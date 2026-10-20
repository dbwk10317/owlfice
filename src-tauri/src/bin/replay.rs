//! owlfice-replay: fixture를 대상 폴더에 시각 순으로 한 줄씩 append한다 (PLAN 4.12)

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

/// 원본의 파일 하나: 대상 안 상대 경로와 줄들 (개행 포함)
struct Src {
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
        files.push(Src {
            rel,
            lines: split_lines(&fs::read(&p)?),
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
    let all = collect(src)?;
    fs::create_dir_all(dst)?;
    let (jsonl, others): (Vec<Src>, Vec<Src>) = all.into_iter().partition(|f| is_jsonl(&f.rel));
    for f in &others {
        let to = dst.join(&f.rel);
        fs::create_dir_all(to.parent().unwrap_or(dst))?;
        fs::write(to, f.lines.concat())?;
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
        out.flush()?;
    }
    Ok(())
}

/// 파싱되지 않는 줄은 그대로 둔다
fn shifted(line: &[u8], delta_ms: i64) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<Value>(line) else {
        return line.to_vec();
    };
    shift_value(None, &mut v, delta_ms);
    let mut out = serde_json::to_vec(&v).unwrap_or_else(|_| line.to_vec());
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
            assert_eq!(got, f.lines.concat(), "{}", f.rel.display());
        }
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
    fn 병합은_시각_순이고_동률은_메인_먼저다() {
        let files: Vec<Src> = collect(&fixture())
            .unwrap()
            .into_iter()
            .filter(|f| is_jsonl(&f.rel))
            .collect();
        assert!(!is_subagent(&files[0].rel), "메인이 앞 번호");
        assert!(files.iter().any(|f| is_subagent(&f.rel)));
        let steps = plan(&files);
        assert_eq!(
            steps.len(),
            files.iter().map(|f| f.lines.len()).sum::<usize>()
        );
        for w in steps.windows(2) {
            assert!(
                (w[0].ts, w[0].file, w[0].line) <= (w[1].ts, w[1].file, w[1].line),
                "{w:?}"
            );
        }
        // 서브 줄이 메인 줄 사이에 섞여야 병합이다
        let first_sub = steps.iter().position(|s| s.file != 0).unwrap();
        assert!(steps[first_sub..].iter().any(|s| s.file == 0));
    }
}
