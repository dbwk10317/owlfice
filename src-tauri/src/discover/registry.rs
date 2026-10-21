//! Claude 세션 등록부 (PLAN 4.4.1)

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, FILETIME, GetLastError, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::Product;

/// 프로세스 생성 시각이 등록부 startedAt 보다 이만큼 넘게 늦으면 pid 를 물려받은 다른 프로세스다 (4.4.1 쓰임 1)
const REUSE_SLACK_MS: i64 = 5_000;

/// `<home>\sessions\<pid>.json` 한 개. 알 수 없는 필드는 무시한다.
/// 필수는 pid, sessionId 뿐이다. 선택 필드는 타입이 달라도 그 필드만 None 이 된다.
/// updatedAt 은 epoch ms 숫자다 (개발 PC 파일에서 확인됨)
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub pid: u32,
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub entrypoint: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub name_source: Option<String>,
    /// epoch ms
    #[serde(default, deserialize_with = "lenient")]
    pub started_at: Option<i64>,
    /// `idle`, `busy` 확인됨. 허가 대기 값은 미확인이라 문자열로 둔다
    #[serde(default, deserialize_with = "lenient")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub updated_at: Option<i64>,
}

impl RegistryEntry {
    /// 사용자가 붙인 이름만. Claude Code 는 이름을 직접 붙일 때 "user" 를 쓰고 자동·충돌 이름은 auto·derived·collision 등으로 따로 적는다
    pub fn user_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .filter(|n| !n.is_empty() && self.name_source.as_deref() == Some("user"))
    }

    /// 동 보조 (4.4.1 쓰임 3)
    pub fn product(&self) -> Option<Product> {
        self.entrypoint
            .as_deref()
            .map(crate::parse::claude::product_of)
    }
}

/// 타입이 맞지 않는 값은 에러 대신 None
fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// 등록부를 한 번 읽은 결과. 없는 것과 못 본 것을 나눈다 (4.5)
#[derive(Debug, Default)]
pub struct RegistryRead {
    pub entries: Vec<RegistryEntry>,
    /// 있지만 읽거나 해석하지 못한 파일의 이름 줄기 (`<pid>`). 쓰는 중일 수 있다
    pub unread: Vec<String>,
}

/// `<home>\sessions\*.json` 을 모두 읽는다. 폴더를 읽지 못하면(NotFound 아님) None.
/// 같은 폴더의 `.key` 파일은 용도를 확인하지 않았고 비밀일 수 있어 열지 않는다 (`.json` 만 연다)
pub fn read_registry(home: &Path) -> Option<RegistryRead> {
    let dir = match fs::read_dir(home.join("sessions")) {
        Ok(d) => d,
        // 폴더가 없는 건 정상이다 (등록부를 안 쓰는 설치)
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(RegistryRead::default()),
        Err(e) => {
            // Scan 마다 돌아서 같은 줄은 한 번만 남긴다
            crate::oplog::log_once(&format!("등록부 폴더 읽기 실패 {}: {e}", home.display()));
            return None;
        }
    };
    let mut out = RegistryRead::default();
    for p in dir.filter_map(Result::ok).map(|e| e.path()) {
        if !p.extension().is_some_and(|x| x == "json") {
            continue;
        }
        // 오류 문구에 파일 내용이 섞일 수 있어 파일 이름과 종류만 남긴다
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let stem = p
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes = match fs::read(&p) {
            Ok(b) => b,
            // 목록을 읽은 뒤 세션이 끝나 지워진 건 정상이다
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                crate::oplog::log_once(&format!("등록부 파일 읽기 실패 {name}: {}", e.kind()));
                out.unread.push(stem);
                continue;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(e) => out.entries.push(e),
            Err(_) => {
                crate::oplog::log_once(&format!("등록부 파일 해석 실패 {name}"));
                out.unread.push(stem);
            }
        }
    }
    Some(out)
}

/// 등록부 중 그 세션의 프로세스가 살아 있는 것만, 세션 id 하나에 한 행.
/// 읽지 못한 파일은 `prev`(직전 결과)의 같은 pid 행을 대신 쓴다. 폴더를 읽지 못하면 None 이라 부르는 쪽이 이번 반영을 건너뛴다
pub fn live_sessions(
    home: &Path,
    prev: &HashMap<String, RegistryEntry>,
) -> Option<HashMap<String, RegistryEntry>> {
    live_with(home, prev, is_alive, created_ms)
}

fn live_with(
    home: &Path,
    prev: &HashMap<String, RegistryEntry>,
    alive: impl Fn(u32) -> bool,
    created: impl Fn(u32) -> Option<i64>,
) -> Option<HashMap<String, RegistryEntry>> {
    let read = read_registry(home)?;
    let kept = prev
        .values()
        .filter(|e| read.unread.contains(&e.pid.to_string()))
        .cloned();
    let mut out: HashMap<String, RegistryEntry> = HashMap::new();
    for e in read.entries.into_iter().chain(kept) {
        if !alive(e.pid) || reused(&e, &created) {
            continue;
        }
        // 살아 있는 행 둘이 같은 세션이면 read_dir 순서에 맡기지 않는다
        match out.get(&e.session_id) {
            Some(o) if rank(o) >= rank(&e) => {}
            _ => {
                out.insert(e.session_id.clone(), e);
            }
        }
    }
    Some(out)
}

/// 같은 세션 행 중 고르는 순서: 사용자가 이름을 붙인 쪽, 늦게 시작한 쪽, 큰 pid
fn rank(e: &RegistryEntry) -> (bool, Option<i64>, u32) {
    (
        e.name_source.as_deref() == Some("user"),
        e.started_at,
        e.pid,
    )
}

/// 죽은 세션의 pid 를 다른 프로세스가 받았나. 생성 시각이나 startedAt 을 모르면 pid 생존만 믿는다
fn reused(e: &RegistryEntry, created: impl Fn(u32) -> Option<i64>) -> bool {
    e.started_at
        .zip(created(e.pid))
        .is_some_and(|(started, created)| created - started > REUSE_SLACK_MS)
}

/// 프로세스 생성 시각 (epoch ms). 열 수 없으면 None
fn created_ms(pid: u32) -> Option<i64> {
    // FILETIME 은 1601-01-01 부터의 100ns 단위다
    const EPOCH_DIFF_MS: i64 = 11_644_473_600_000;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: 핸들은 이 함수 안에서 열고 닫는다
    let ok = unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let ok = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(h);
        ok
    };
    let ft = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    ok.then(|| (ft / 10_000) as i64 - EPOCH_DIFF_MS)
}

pub fn is_alive(pid: u32) -> bool {
    // SAFETY: 핸들은 이 함수 안에서 열고 닫는다
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // 권한이 높은 프로세스는 열 수 없어도 살아 있다
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn home_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        fs::create_dir(&dir).unwrap();
        for (name, body) in files {
            fs::write(dir.join(name), body).unwrap();
        }
        home
    }

    #[test]
    fn reads_good_files_and_skips_bad_ones() {
        let home = home_with(&[
            (
                "1.json",
                r#"{"pid":1,"sessionId":"s1","cwd":"C:\\x","name":"n","status":"busy","updatedAt":2000,"extra":{"a":1}}"#,
            ),
            ("2.json", r#"{"pid":2,"sessionId":"#), // 쓰는 중에 깨짐
            ("3.json", r#"{"pid":3}"#),             // 필수 필드 누락
            ("4.json", r#"{"pid":"4","sessionId":"s4"}"#), // 타입 다름
            ("5.json", r#"{"pid":5,"sessionId":"s5"}"#),
            // 선택 필드 타입이 달라도 파일은 살고 그 필드만 None
            (
                "6.json",
                r#"{"pid":6,"sessionId":"s6","updatedAt":"x","status":1,"name":"n6"}"#,
            ),
        ]);
        let read = read_registry(home.path()).unwrap();
        let mut unread = read.unread;
        unread.sort();
        assert_eq!(unread, ["2", "3", "4"]);
        let mut got = read.entries;
        got.sort_by_key(|e| e.pid);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].session_id, "s1");
        assert_eq!(got[0].status.as_deref(), Some("busy"));
        assert_eq!(got[0].updated_at, Some(2000));
        assert_eq!(got[1].pid, 5);
        assert_eq!(got[1].name, None);
        assert_eq!(got[2].pid, 6);
        assert_eq!(got[2].updated_at, None);
        assert_eq!(got[2].status, None);
        assert_eq!(got[2].name.as_deref(), Some("n6"));
    }

    #[test]
    fn missing_sessions_dir_gives_empty_list() {
        let home = tempfile::tempdir().unwrap();
        assert!(read_registry(home.path()).unwrap().entries.is_empty());
    }

    #[test]
    fn key_files_are_not_read() {
        // .key 가 유효한 JSON이어도 결과에 들어오면 안 된다
        let home = home_with(&[("7.abcdef.key", r#"{"pid":7,"sessionId":"secret"}"#)]);
        let read = read_registry(home.path()).unwrap();
        assert!(read.entries.is_empty() && read.unread.is_empty());
    }

    #[test]
    fn own_pid_is_alive_and_exited_child_is_dead() {
        assert!(is_alive(std::process::id()));
        let mut child = Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!is_alive(pid));
    }

    #[test]
    fn live_sessions_drops_dead_pids() {
        let me = std::process::id();
        let home = home_with(&[
            ("a.json", &format!(r#"{{"pid":{me},"sessionId":"live"}}"#)),
            ("b.json", r#"{"pid":4294967294,"sessionId":"dead"}"#),
        ]);
        let got = live_sessions(home.path(), &HashMap::new()).unwrap();
        assert_eq!(got.keys().collect::<Vec<_>>(), ["live"]);
    }

    #[test]
    fn pid_created_more_than_5s_after_start_is_another_process() {
        let home = home_with(&[
            (
                "a.json",
                r#"{"pid":1,"sessionId":"reused","startedAt":10000}"#,
            ),
            (
                "b.json",
                r#"{"pid":2,"sessionId":"same","startedAt":10000}"#,
            ),
            (
                "c.json",
                r#"{"pid":3,"sessionId":"unknown","startedAt":10000}"#,
            ),
            ("d.json", r#"{"pid":4,"sessionId":"no-start"}"#),
        ]);
        let created = |pid| match pid {
            1 => Some(15_001),
            2 => Some(15_000),
            4 => Some(99_999),
            _ => None, // 권한 등으로 못 얻음
        };
        let mut got: Vec<String> = live_with(home.path(), &HashMap::new(), |_| true, created)
            .unwrap()
            .into_keys()
            .collect();
        got.sort();
        assert_eq!(got, ["no-start", "same", "unknown"]);
        // 생성 시각을 못 얻어도 죽은 pid 는 죽은 것이다
        assert!(
            live_with(home.path(), &HashMap::new(), |_| false, |_| None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unreadable_file_keeps_its_previous_row_and_unreadable_dir_skips() {
        let home = home_with(&[
            ("1.json", r#"{"pid":1,"sessionId":"s1"}"#),
            ("2.json", r#"{"pid":2,"sessionId":"#), // 쓰는 중
        ]);
        let prev: HashMap<String, RegistryEntry> = [
            (r#"{"pid":2,"sessionId":"s2","name":"n"}"#),
            // 파일이 정말 사라진 행은 따라오지 않는다
            (r#"{"pid":3,"sessionId":"s3"}"#),
        ]
        .iter()
        .map(|j| serde_json::from_str::<RegistryEntry>(j).unwrap())
        .map(|e| (e.session_id.clone(), e))
        .collect();
        let got = live_with(home.path(), &prev, |_| true, |_| None).unwrap();
        let mut ids: Vec<&String> = got.keys().collect();
        ids.sort();
        assert_eq!(ids, ["s1", "s2"]);
        assert_eq!(got["s2"].name.as_deref(), Some("n"));
        // 이어받은 행도 프로세스가 죽었으면 빠진다
        let got = live_with(home.path(), &prev, |pid| pid != 2, |_| None).unwrap();
        assert!(!got.contains_key("s2"));
        // 폴더 자리에 파일: NotFound 가 아닌 폴더 실패는 "못 봄"이다
        let odd = tempfile::tempdir().unwrap();
        fs::write(odd.path().join("sessions"), "").unwrap();
        assert!(read_registry(odd.path()).is_none());
        assert!(live_with(odd.path(), &prev, |_| true, |_| None).is_none());
    }

    #[test]
    fn same_session_twice_prefers_user_name_then_later_start() {
        let pick = |a: &str, b: &str| {
            let home = home_with(&[("1.json", a), ("2.json", b)]);
            let got = live_with(home.path(), &HashMap::new(), |_| true, |_| None).unwrap();
            assert_eq!(got.len(), 1);
            got["s"].pid
        };
        let user = r#"{"pid":1,"sessionId":"s","nameSource":"user","startedAt":1}"#;
        let later = r#"{"pid":2,"sessionId":"s","nameSource":"auto","startedAt":9}"#;
        // 파일 순서를 바꿔도 같다
        assert_eq!([pick(user, later), pick(later, user)], [1, 1]);
        let early = r#"{"pid":1,"sessionId":"s","startedAt":1}"#;
        assert_eq!([pick(early, later), pick(later, early)], [2, 2]);
    }

    #[test]
    fn own_creation_time_is_before_now() {
        let now = chrono::Utc::now().timestamp_millis();
        let created = created_ms(std::process::id()).unwrap();
        assert!(created <= now && now - created < 600_000, "{created} {now}");
    }

    #[test]
    fn only_user_given_name_counts() {
        let entry = |extra: &str| -> RegistryEntry {
            serde_json::from_str(&format!(r#"{{"pid":1,"sessionId":"s"{extra}}}"#)).unwrap()
        };
        let user = entry(r#","name":"n","nameSource":"user""#);
        assert_eq!(user.user_name(), Some("n"));
        assert_eq!(
            entry(r#","name":"n","nameSource":"auto""#).user_name(),
            None
        );
        assert_eq!(entry(r#","name":"n""#).user_name(), None);
        assert_eq!(entry(r#","name":"","nameSource":"user""#).user_name(), None);
    }
}
