//! 운영 로그 (PLAN 4.11). `%APPDATA%\Owlfice\logs\owlfice-YYYY-MM-DD.log` 에 한 줄씩 남긴다.
//! 세션 내용(프롬프트·명령·결과)은 부르는 쪽이 넘기지 않는다. 구조 정보와 경로만 쓴다.
//! 초기화 전(단위 테스트, probe·replay bin)에는 stderr 로 쓴다.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDate, SecondsFormat};

const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 오늘 포함 이만큼의 파일을 둔다
const KEEP_DAYS: i64 = 7;
const PREFIX: &str = "owlfice-";
const SUFFIX: &str = ".log";
/// 한 번만 남기는 줄의 기억 상한. 넘으면 새 줄은 기억하지 않고 그냥 쓴다 (스팸은 10MB 상한이 막는다)
const ONCE_MAX: usize = 256;

struct OpLog {
    dir: PathBuf,
    max_bytes: u64,
    day: Option<NaiveDate>,
    file: Option<File>,
    written: u64,
    capped: bool,
    warned: bool,
}

impl OpLog {
    fn new(dir: &Path) -> Self {
        Self::with_max(dir, MAX_BYTES)
    }

    fn with_max(dir: &Path, max_bytes: u64) -> Self {
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
            day: None,
            file: None,
            written: 0,
            capped: false,
            warned: false,
        }
    }

    fn file_name(day: NaiveDate) -> String {
        format!("{PREFIX}{day}{SUFFIX}")
    }

    /// 쓰기 실패는 앱을 멈추지 않는다. 한 번만 stderr 로 알린다
    fn warn(&mut self, what: &str, e: &std::io::Error) {
        if !std::mem::replace(&mut self.warned, true) {
            eprintln!("운영 로그 {what} 실패 {}: {e}", self.dir.display());
        }
    }

    /// 오늘 파일을 열고 7일 지난 파일을 지운다. 열린 파일의 이름은 날짜가 바뀔 때만 바꾼다
    fn roll(&mut self, now: DateTime<FixedOffset>) {
        let today = now.date_naive();
        self.day = Some(today);
        self.file = None;
        self.written = 0;
        self.capped = false;
        if let Err(e) = fs::create_dir_all(&self.dir) {
            return self.warn("폴더 만들기", &e);
        }
        let path = self.dir.join(Self::file_name(today));
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                self.warned = false;
                self.written = f.metadata().map_or(0, |m| m.len());
                self.capped = self.written >= self.max_bytes;
                self.file = Some(f);
            }
            Err(e) => self.warn("파일 열기", &e),
        }
        self.prune(today);
    }

    fn prune(&self, today: NaiveDate) {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in rd.filter_map(Result::ok) {
            let name = entry.file_name();
            // 이름이 이 형식인 파일만 지운다. 다른 파일은 건드리지 않는다
            let Some(day) = name.to_str().and_then(|n| {
                n.strip_prefix(PREFIX)?
                    .strip_suffix(SUFFIX)?
                    .parse::<NaiveDate>()
                    .ok()
            }) else {
                continue;
            };
            if today - day >= Duration::days(KEEP_DAYS) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    fn write(&mut self, now: DateTime<FixedOffset>, msg: &str) {
        // 파일이 없으면(폴더가 막혔다 풀렸을 수 있다) 날짜가 같아도 다시 연다
        if self.day != Some(now.date_naive()) || self.file.is_none() {
            self.roll(now);
        }
        if self.capped {
            return;
        }
        let stamp = now.to_rfc3339_opts(SecondsFormat::Secs, false);
        let mut out = format!("{stamp} {}\n", msg.replace(['\r', '\n'], " "));
        // 넘친 줄까지 쓰고 상한 줄을 붙여 파일이 늘 상한 이상이 되게 한다. 다시 열어도 그날은 이어 쓰지 않는다
        if self.written + out.len() as u64 > self.max_bytes {
            self.capped = true;
            out += &format!("{stamp} 상한 도달: 오늘은 더 쓰지 않는다\n");
        }
        let Some(f) = self.file.as_mut() else {
            // 파일을 못 쓰는 동안은 줄이 사라지지 않게 stderr 로 넘긴다
            eprint!("{out}");
            return;
        };
        match f.write_all(out.as_bytes()) {
            Ok(()) => {
                self.written += out.len() as u64;
                self.warned = false;
            }
            Err(e) => {
                self.file = None;
                self.warn("쓰기", &e);
                eprint!("{out}");
            }
        }
    }
}

static LOG: OnceLock<Mutex<OpLog>> = OnceLock::new();
static ONCE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// 프로세스에 하나. 앱 시작 때 한 번 부른다
pub fn init(dir: &Path) {
    let mut log = OpLog::new(dir);
    log.roll(Local::now().fixed_offset());
    let _ = LOG.set(Mutex::new(log));
}

pub fn log(msg: &str) {
    match LOG.get() {
        Some(m) => m
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write(Local::now().fixed_offset(), msg),
        None => eprintln!("{msg}"),
    }
}

/// 같은 줄은 한 번만 남긴다. 주기적으로 도는 곳의 실패용
pub fn log_once(msg: &str) {
    let first = {
        let mut seen = ONCE.lock().unwrap_or_else(|e| e.into_inner());
        first_time(seen.get_or_insert_with(HashSet::new), msg)
    };
    if first {
        log(msg);
    }
}

fn first_time(set: &mut HashSet<String>, msg: &str) -> bool {
    if set.contains(msg) {
        return false;
    }
    // 기억이 찼으면 새 줄은 기억 없이 쓴다
    if set.len() < ONCE_MAX {
        set.insert(msg.to_string());
    }
    true
}

#[macro_export]
macro_rules! oplog {
    ($($a:tt)*) => { $crate::oplog::log(&format!($($a)*)) };
}

#[macro_export]
macro_rules! oplog_once {
    ($($a:tt)*) => { $crate::oplog::log_once(&format!($($a)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn read(dir: &Path, day: &str) -> String {
        fs::read_to_string(dir.join(format!("owlfice-{day}.log"))).unwrap()
    }

    #[test]
    fn 기록은_그날_파일에_한_줄로_남는다() {
        let d = tempfile::tempdir().unwrap();
        let mut l = OpLog::new(d.path());
        l.write(at("2026-03-05T09:00:00+09:00"), "첫째\n둘째");
        l.write(at("2026-03-05T23:59:59+09:00"), "셋째");
        assert_eq!(
            read(d.path(), "2026-03-05"),
            "2026-03-05T09:00:00+09:00 첫째 둘째\n2026-03-05T23:59:59+09:00 셋째\n"
        );
    }

    #[test]
    fn 날짜가_바뀌면_새_파일() {
        let d = tempfile::tempdir().unwrap();
        let mut l = OpLog::new(d.path());
        l.write(at("2026-03-05T23:59:59+09:00"), "어제");
        l.write(at("2026-03-06T00:00:01+09:00"), "오늘");
        assert!(read(d.path(), "2026-03-05").ends_with("어제\n"));
        assert!(read(d.path(), "2026-03-06").ends_with("오늘\n"));
    }

    #[test]
    fn 칠일_지난_파일은_지우고_그_안은_남긴다() {
        let d = tempfile::tempdir().unwrap();
        for n in ["2026-02-26", "2026-02-27", "2026-02-28", "2026-03-04"] {
            fs::write(d.path().join(format!("owlfice-{n}.log")), "x").unwrap();
        }
        fs::write(d.path().join("unhandled.jsonl"), "x").unwrap();
        let mut l = OpLog::new(d.path());
        l.write(at("2026-03-06T00:00:00+09:00"), "a");
        assert!(!d.path().join("owlfice-2026-02-26.log").exists());
        assert!(!d.path().join("owlfice-2026-02-27.log").exists());
        assert!(d.path().join("owlfice-2026-02-28.log").exists());
        assert!(d.path().join("owlfice-2026-03-04.log").exists());
        assert!(d.path().join("unhandled.jsonl").exists());
    }

    #[test]
    fn 상한_뒤에는_더_쓰지_않는다() {
        let d = tempfile::tempdir().unwrap();
        let mut l = OpLog::with_max(d.path(), 200);
        for i in 0..50 {
            l.write(at("2026-03-05T09:00:00+09:00"), &format!("줄 {i}"));
        }
        let text = read(d.path(), "2026-03-05");
        assert_eq!(text.matches("상한 도달").count(), 1);
        assert!(text.lines().last().unwrap().contains("상한 도달"));
        assert!(text.len() >= 200);
    }

    #[test]
    fn 상한_줄보다_긴_줄로_넘겨도_파일은_상한_이상이고_다시_열어도_이어_쓰지_않는다() {
        let d = tempfile::tempdir().unwrap();
        let mut l = OpLog::with_max(d.path(), 200);
        l.write(at("2026-03-05T09:00:00+09:00"), &"긴".repeat(100));
        let text = read(d.path(), "2026-03-05");
        assert!(text.len() >= 200, "{}", text.len());
        assert!(text.lines().last().unwrap().contains("상한 도달"));
        let before = text.len();
        let mut again = OpLog::with_max(d.path(), 200);
        again.write(at("2026-03-05T10:00:00+09:00"), "다시");
        assert_eq!(read(d.path(), "2026-03-05").len(), before);
    }

    #[test]
    fn log_once_는_기억이_차도_새_줄을_쓴다() {
        // 전역 기억은 이 프로세스의 다른 테스트와 겹치지 않는 줄로만 본다
        for i in 0..ONCE_MAX + 5 {
            log_once(&format!("기억 테스트 {i}"));
        }
        let seen = ONCE.lock().unwrap();
        assert_eq!(seen.as_ref().unwrap().len(), ONCE_MAX);
    }

    #[test]
    fn log_once_는_기억이_찬_뒤에도_기억한_줄은_다시_안_쓰고_새_줄은_쓴다() {
        let mut set = HashSet::new();
        for i in 0..ONCE_MAX {
            assert!(first_time(&mut set, &format!("줄 {i}")));
        }
        assert!(!first_time(&mut set, "줄 0"));
        assert!(first_time(&mut set, "새 줄"));
        assert!(first_time(&mut set, "새 줄"), "기억 못 한 줄은 다시 쓴다");
    }

    #[test]
    fn 폴더를_쓸_수_없어도_패닉_없이_계속한다() {
        let d = tempfile::tempdir().unwrap();
        // 폴더 자리에 파일이 있어 만들 수 없다
        let blocked = d.path().join("logs");
        fs::write(&blocked, "x").unwrap();
        let mut l = OpLog::new(&blocked);
        l.write(at("2026-03-05T09:00:00+09:00"), "a");
        l.write(at("2026-03-05T09:00:01+09:00"), "b");
        // 풀리면 이어서 쓴다
        fs::remove_file(&blocked).unwrap();
        l.write(at("2026-03-05T09:00:02+09:00"), "c");
        assert!(read(&blocked, "2026-03-05").ends_with("c\n"));
    }
}
