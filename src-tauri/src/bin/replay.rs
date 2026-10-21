//! owlfice-replay: fixture를 대상 폴더에 시각 순으로 한 줄씩 append한다 (docs/CONTRACTS.md C12)

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

const USAGE: &str =
    "사용법: owlfice-replay [--speed N] [--shift-to-now] <원본 파일 또는 폴더> <대상 폴더>";

struct Opts {
    speed: f64,
    shift: bool,
    src: PathBuf,
    dst: PathBuf,
}

fn parse_args(args: &[String]) -> Option<Opts> {
    let (mut speed, mut shift, mut pos) = (1.0, false, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--speed" => {
                speed = it
                    .next()?
                    .parse()
                    .ok()
                    .filter(|s: &f64| s.is_finite() && *s > 0.0)?;
            }
            "--shift-to-now" => shift = true,
            _ => pos.push(PathBuf::from(a)),
        }
    }
    let [src, dst] = <[PathBuf; 2]>::try_from(pos).ok()?;
    Some(Opts {
        speed,
        shift,
        src,
        dst,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(opts) = parse_args(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match replay(&opts.src, &opts.dst, opts.speed, opts.shift) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 원본의 파일 하나: 원본 경로, 대상 안 상대 경로, jsonl 이면 줄들 (개행 포함)
struct Src {
    path: PathBuf,
    rel: PathBuf,
    lines: Vec<Vec<u8>>,
}

/// 병합 순서의 한 칸. `ts` 는 파일 안에서 되돌아가지 않게 누적 최댓값으로 맞춘 ms
#[derive(Debug)]
struct Step {
    file: usize,
    line: usize,
    ts: Option<i64>,
}

fn is_subagent(rel: &Path) -> bool {
    rel.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "subagents")
}

fn top_ts(line: &[u8]) -> Option<i64> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let t = DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?).ok()?;
    Some(t.timestamp_millis())
}

/// 시각 순. 동률·시각 없음은 메인 파일 먼저, 그다음 원래 줄 순서.
/// 시각 없는 줄은 같은 파일 앞 줄의 시각을 따른다. 파일 안 순서는 TailReader가 보는 순서라 깨지 않는다.
/// `files` 는 메인이 앞인 순서여야 한다
fn plan(files: &[Src]) -> Vec<Step> {
    let mut steps = Vec::new();
    for (file, f) in files.iter().enumerate() {
        let mut run: Option<i64> = None;
        for (line, bytes) in f.lines.iter().enumerate() {
            run = match (run, top_ts(bytes)) {
                (Some(r), Some(t)) => Some(r.max(t)),
                (r, t) => r.or(t),
            };
            steps.push(Step {
                file,
                line,
                ts: run,
            });
        }
    }
    // None 은 맨 앞
    steps.sort_by_key(|s| (s.ts, s.file, s.line));
    steps
}

fn collect(src: &Path) -> io::Result<Vec<Src>> {
    let (root, found) = if src.is_file() {
        (
            src.parent().unwrap_or(Path::new("")).to_path_buf(),
            vec![src.to_path_buf()],
        )
    } else {
        let mut found = Vec::new();
        let mut stack = vec![src.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in fs::read_dir(&dir)? {
                let p = e?.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    found.push(p);
                }
            }
        }
        (src.to_path_buf(), found)
    };
    let mut files = Vec::new();
    for p in found {
        let rel = p.strip_prefix(&root).unwrap_or(&p).to_path_buf();
        // jsonl 이 아닌 파일은 줄을 안 읽고 replay 가 그대로 복사한다
        let lines = if is_jsonl(&rel) {
            split_lines(&fs::read(&p)?)
        } else {
            Vec::new()
        };
        files.push(Src {
            path: p,
            rel,
            lines,
        });
    }
    files.sort_by_key(|f| (is_subagent(&f.rel), f.rel.clone()));
    Ok(files)
}

fn split_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = bytes
        .split_inclusive(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    // 끝 개행이 없으면 TailReader가 그 줄을 못 읽으니 붙인다
    if let Some(last) = lines.last_mut()
        && last.last() != Some(&b'\n')
    {
        last.push(b'\n');
    }
    lines
}

fn is_jsonl(rel: &Path) -> bool {
    rel.extension().is_some_and(|e| e == "jsonl")
}

fn replay(src: &Path, dst: &Path, speed: f64, shift: bool) -> io::Result<()> {
    // 기존 파일 뒤에 append 하면 두 번 돌릴 때 줄이 겹친다
    if fs::read_dir(dst).is_ok_and(|mut d| d.next().is_some()) {
        return Err(io::Error::other(format!(
            "대상 폴더가 비어 있지 않습니다: {}",
            dst.display()
        )));
    }
    let all = collect(src)?;
    fs::create_dir_all(dst)?;
    let (jsonl, others): (Vec<Src>, Vec<Src>) = all.into_iter().partition(|f| is_jsonl(&f.rel));
    for f in &others {
        let to = dst.join(&f.rel);
        fs::create_dir_all(to.parent().unwrap_or(dst))?;
        fs::copy(&f.path, to)?;
    }
    let steps = plan(&jsonl);
    // ponytail: Codex 파일 이름 속 시각은 옮기지 않는다, 이름의 시각을 읽는 쪽이 생기면 이름도 바꾼다
    let delta = steps
        .iter()
        .find_map(|s| s.ts)
        .map_or(0, |first| Utc::now().timestamp_millis() - first);
    let mut outs: Vec<Option<File>> = jsonl.iter().map(|_| None).collect();
    let mut prev: Option<i64> = None;
    for s in &steps {
        if let (Some(p), Some(t)) = (prev, s.ts) {
            sleep(Duration::from_secs_f64((t - p) as f64 / 1000.0 / speed));
        }
        prev = s.ts.or(prev);
        let f = &jsonl[s.file];
        let out = match &mut outs[s.file] {
            Some(o) => o,
            slot => {
                let to = dst.join(&f.rel);
                fs::create_dir_all(to.parent().unwrap_or(dst))?;
                slot.insert(OpenOptions::new().create(true).append(true).open(to)?)
            }
        };
        let raw = &f.lines[s.line];
        if shift {
            out.write_all(&shifted(raw, delta))?;
        } else {
            out.write_all(raw)?;
        }
    }
    Ok(())
}

/// 파싱되지 않는 줄은 그대로 둔다.
/// 다시 직렬화하니 키 순서가 바뀔 수 있지만 파서는 순서에 기대지 않아 그대로 둔다
fn shifted(line: &[u8], delta_ms: i64) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<Value>(line) else {
        return line.to_vec();
    };
    shift_value(None, &mut v, delta_ms);
    // Value 직렬화는 실패하지 않는다
    let mut out = serde_json::to_vec(&v).expect("Value 직렬화");
    out.push(b'\n');
    out
}

fn shift_value(key: Option<&str>, v: &mut Value, delta_ms: i64) {
    match v {
        Value::String(s) => {
            if let Ok(t) = DateTime::parse_from_rfc3339(s) {
                let digits = s.split_once('.').map_or(0, |(_, r)| {
                    r.chars().take_while(char::is_ascii_digit).count()
                });
                let fmt = match digits {
                    0 => SecondsFormat::Secs,
                    1..=3 => SecondsFormat::Millis,
                    4..=6 => SecondsFormat::Micros,
                    _ => SecondsFormat::Nanos,
                };
                let moved = t.with_timezone(&Utc) + chrono::Duration::milliseconds(delta_ms);
                *s = moved.to_rfc3339_opts(fmt, true);
            }
        }
        Value::Number(n) => {
            let (Some(key), Some(x)) = (key, n.as_i64()) else {
                return;
            };
            if key.ends_with("_ms") && (1_000_000_000_000..10_000_000_000_000).contains(&x) {
                *v = (x + delta_ms).into();
            } else if key.ends_with("_at") && (1_000_000_000..10_000_000_000).contains(&x) {
                *v = (x + delta_ms.div_euclid(1000)).into();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|e| shift_value(None, e, delta_ms)),
        Value::Object(m) => m
            .iter_mut()
            .for_each(|(k, e)| shift_value(Some(k), e, delta_ms)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/claude/async-subagent")
    }

    #[test]
    fn 옮기지_않으면_모든_파일이_원본과_바이트가_같다() {
        let tmp = tempfile::tempdir().unwrap();
        replay(&fixture(), tmp.path(), 1e9, false).unwrap();
        let src = collect(&fixture()).unwrap();
        assert!(src.len() >= 3, "메인 + 서브 + meta.json");
        for f in src {
            let got = fs::read(tmp.path().join(&f.rel)).unwrap();
            assert_eq!(got, fs::read(&f.path).unwrap(), "{}", f.rel.display());
        }
    }

    #[test]
    fn 대상_폴더가_비어_있지_않으면_멈춘다() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("old.txt"), "x").unwrap();
        assert!(replay(&fixture(), tmp.path(), 1e9, false).is_err());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    fn all_ts(root: &Path) -> Vec<i64> {
        let mut v: Vec<i64> = collect(root)
            .unwrap()
            .iter()
            .filter(|f| is_jsonl(&f.rel))
            .flat_map(|f| f.lines.iter().filter_map(|l| top_ts(l)))
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn shift_to_now_는_첫_시각이_지금이고_간격은_그대로다() {
        let tmp = tempfile::tempdir().unwrap();
        replay(&fixture(), tmp.path(), 1e9, true).unwrap();
        let (orig, got) = (all_ts(&fixture()), all_ts(tmp.path()));
        assert_eq!(orig.len(), got.len());
        assert!((got[0] - Utc::now().timestamp_millis()).abs() < 5000);
        let gaps = |v: &[i64]| v.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
        assert_eq!(gaps(&orig), gaps(&got));
    }

    #[test]
    fn shift_to_now_는_started_at_ms_도_같은_만큼_옮긴다() {
        let codex = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/codex/codex-app");
        // 줄 안 어디에 있든 첫 started_at_ms 를 찾는다
        fn dig(v: &Value) -> Option<i64> {
            match v {
                Value::Object(m) => m
                    .get("started_at_ms")
                    .and_then(Value::as_i64)
                    .or_else(|| m.values().find_map(dig)),
                Value::Array(a) => a.iter().find_map(dig),
                _ => None,
            }
        }
        let find = |root: &Path| -> i64 {
            collect(root)
                .unwrap()
                .iter()
                .flat_map(|f| f.lines.iter())
                .find_map(|l| dig(&serde_json::from_slice(l).ok()?))
                .expect("started_at_ms 가 든 줄")
        };
        let tmp = tempfile::tempdir().unwrap();
        replay(&codex, tmp.path(), 1e9, true).unwrap();
        let (o, g) = (all_ts(&codex), all_ts(tmp.path()));
        assert_eq!(find(tmp.path()) - find(&codex), g[0] - o[0]);
    }

    #[test]
    fn 병합은_시각_순이고_동률은_메인_먼저다() {
        let tmp = tempfile::tempdir().unwrap();
        let line = |t: &str, id: &str| {
            format!(
                "{{\"timestamp\":\"{t}\",\"id\":\"{id}\"}}
"
            )
        };
        let sub = tmp.path().join("subagents");
        fs::create_dir(&sub).unwrap();
        // 서브가 이름순으로 앞서도 동률이면 메인이 먼저여야 한다
        fs::write(
            tmp.path().join("main.jsonl"),
            line("2026-01-01T00:00:01Z", "m1") + &line("2026-01-01T00:00:03Z", "m2"),
        )
        .unwrap();
        fs::write(
            sub.join("a.jsonl"),
            line("2026-01-01T00:00:01Z", "s1") + &line("2026-01-01T00:00:02Z", "s2"),
        )
        .unwrap();
        let files = collect(tmp.path()).unwrap();
        let ids: Vec<String> = plan(&files)
            .iter()
            .map(|s| {
                let v: Value = serde_json::from_slice(&files[s.file].lines[s.line]).unwrap();
                v["id"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(ids, ["m1", "s1", "s2", "m2"]);
    }
}
