//! owlfice-probe: 탐색 결과와 이벤트 열을 JSON으로 낸다 (PLAN 4.12)

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use owlfice_lib::diag::summarize;
use owlfice_lib::discover::{Host, HostKind, ReadMode, Site, Vendor, discover_all, discover_wsl};
use owlfice_lib::parse::{Parser, RawRef};
use owlfice_lib::tail::scheduler::{self, is_subagent};
use owlfice_lib::tail::{MAX_READ_BYTES, TailReader};

const USAGE: &str = "사용법: owlfice-probe discover | owlfice-probe events <파일 또는 폴더>... | owlfice-probe unhandled [경로] | owlfice-probe bench <폴더> [--json]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = io::stdout().lock();
    let result = match args.first().map(String::as_str) {
        Some("discover") if args.len() == 1 => discover(&mut out),
        Some("events") if args.len() > 1 => {
            let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
            events(&paths, &mut out)
        }
        Some("unhandled") if args.len() <= 2 => unhandled(args.get(1).map(PathBuf::from), &mut out),
        Some("bench") => {
            let json = args[1..].iter().any(|a| a == "--json");
            let dirs: Vec<&String> = args[1..].iter().filter(|a| *a != "--json").collect();
            let unknown_flag = dirs.iter().any(|a| a.starts_with("--"));
            match dirs[..] {
                [dir] if !unknown_flag => bench_cmd(Path::new(dir), json, &mut out),
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result.and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::FAILURE
        }
    }
}

fn discover(out: &mut impl Write) -> io::Result<()> {
    let v = serde_json::json!({ "sites": discover_all(), "wsl": discover_wsl() });
    writeln!(out, "{v}")
}

/// 경로를 안 주면 앱이 쓰는 `%APPDATA%\Owlfice\logs\unhandled.jsonl`
fn unhandled(path: Option<PathBuf>, out: &mut impl Write) -> io::Result<()> {
    let path = match path {
        Some(p) => p,
        None => PathBuf::from(
            std::env::var_os("APPDATA")
                .ok_or_else(|| io::Error::other("APPDATA 가 없어 경로를 정할 수 없습니다"))?,
        )
        .join("Owlfice/logs/unhandled.jsonl"),
    };
    write!(out, "{}", summarize(&path)?)
}

/// 폴더면 아래 .jsonl 전부. 메인 파일 먼저, 그다음 subagents
fn jsonl_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let p = entry?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "jsonl") {
                found.push(p);
            }
        }
    }
    found.sort_by_key(|p| (is_subagent(p), p.clone()));
    Ok(found)
}

/// 파일명으로 제조사를 정해 앱과 같은 파서를 쓴다
fn make_parser(path: &Path) -> Box<dyn Parser + Send> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let vendor = if stem.starts_with("rollout-") {
        Vendor::Codex
    } else {
        Vendor::Claude
    };
    scheduler::make_parser(path, vendor)
}

fn events(paths: &[PathBuf], out: &mut impl Write) -> io::Result<()> {
    for arg in paths {
        for path in jsonl_files(arg)? {
            read_file(&path, out)?;
        }
    }
    Ok(())
}

fn read_file(path: &Path, out: &mut impl Write) -> io::Result<()> {
    let mut parser = make_parser(path);
    let mut reader = TailReader::open(path)?;
    let file = path.display().to_string();
    // 마지막으로 읽은 줄의 시작 바이트. 없으면 파일 처음부터가 안 읽은 구간이다
    let mut last_line: Option<u64> = None;
    loop {
        let pending = reader.read_new_lines(MAX_READ_BYTES)?;
        if pending.lines.is_empty() {
            reader.commit(pending);
            // 끝 개행이 없는 마지막 줄은 TailReader 규칙대로 읽지 않는다. 진단 도구라 알린다
            let rest = unread_bytes(path, last_line)?;
            if rest > 0 {
                eprintln!(
                    "{}: 끝 개행 없는 {rest}바이트는 읽지 않았습니다",
                    path.display()
                );
            }
            return Ok(());
        }
        for line in &pending.lines {
            last_line = Some(line.offset);
            let raw = RawRef {
                path: path.to_path_buf(),
                generation: u64::from(reader.generation()),
                byte_offset: line.offset,
                line_no: line.line_no,
            };
            let mut unhandled = Vec::new();
            for ev in parser.parse_line(&line.bytes, &raw, &mut unhandled) {
                writeln!(out, "{}", serde_json::json!({ "event": ev }))?;
            }
            for u in unhandled {
                writeln!(
                    out,
                    "{}",
                    serde_json::json!({ "unhandled": u, "file": file })
                )?;
            }
        }
        reader.commit(pending);
    }
}

/// 마지막으로 읽은 줄 뒤에 개행 없이 남은 바이트 수
fn unread_bytes(path: &Path, last_line: Option<u64>) -> io::Result<usize> {
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(last_line.unwrap_or(0)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(match last_line {
        None => buf.len(),
        Some(_) => buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(0, |i| buf.len() - i - 1),
    })
}

// ponytail: 4.10 은 60초 동안 팀 4개(5MB + 2MB×4)를 재지만 여기는 횟수만 채우고 파일은 100KB 하나다.
// 나열은 N=50 이라 p99 가 최대와 같다. 실측값을 4.10 예산과 직접 견주게 되면 그때 올린다
const BENCH_N: [usize; 4] = [1000, 100, 200, 50];
/// 읽기 한 번에 붙이는 양은 이 둘의 곱(100KB). 4.10 "Hot 틱 (읽기 포함, 100KB)"
const LINE_BYTES: usize = 1024;
const LINES_PER_READ: usize = 100;
/// 4.10 "Scan 1회 (로컬 프로젝트 폴더 25개)"
const PROJECTS: usize = 25;

struct Stat {
    key: &'static str,
    label: &'static str,
    n: usize,
    mean: f64,
    p50: f64,
    p99: f64,
    max: f64,
}

fn stat(key: &'static str, label: &'static str, mut v: Vec<Duration>) -> Stat {
    v.sort();
    let ms: Vec<f64> = v.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    let n = ms.len();
    // 순위 기준 백분위 (보간하지 않는다)
    let rank = |p: f64| ms[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
    Stat {
        key,
        label,
        n,
        mean: ms.iter().sum::<f64>() / n as f64,
        p50: rank(0.5),
        p99: rank(0.99),
        max: ms[n - 1],
    }
}

fn bench_cmd(dir: &Path, json: bool, out: &mut impl Write) -> io::Result<()> {
    let stats = bench(dir, BENCH_N)?;
    if json {
        let items: Vec<_> = stats
            .iter()
            .map(|s| {
                serde_json::json!({
                    "key": s.key, "n": s.n, "mean_ms": s.mean,
                    "p50_ms": s.p50, "p99_ms": s.p99, "max_ms": s.max,
                })
            })
            .collect();
        return writeln!(out, "{}", serde_json::json!({ "dir": dir, "items": items }));
    }
    writeln!(out, "{}", dir.display())?;
    writeln!(
        out,
        "{:<10} {:>5} {:>9} {:>9} {:>9} {:>9}  (ms)",
        "항목", "N", "평균", "p50", "p99", "최대"
    )?;
    for s in &stats {
        writeln!(
            out,
            "{:<10} {:>5} {:>9.3} {:>9.3} {:>9.3} {:>9.3}",
            s.label, s.n, s.mean, s.p50, s.p99, s.max
        )?;
    }
    Ok(())
}

/// `dir` 아래에 새 폴더를 만들어 재고 지운다. 이미 있는 폴더는 남의 것일 수 있어 쓰지 않는다
fn bench(dir: &Path, n: [usize; 4]) -> io::Result<Vec<Stat>> {
    in_scratch(dir, |work| measure(work, n))
}

fn in_scratch<T>(dir: &Path, f: impl FnOnce(&Path) -> io::Result<T>) -> io::Result<T> {
    let work = dir.join(format!("owlfice-bench-{}", std::process::id()));
    fs::create_dir(&work)?;
    let r = f(&work);
    // 열린 핸들은 f 가 끝나며 닫혔다. 지우기 실패도 알려야 임시 파일이 남은 걸 안다
    match (r, fs::remove_dir_all(&work)) {
        (Ok(v), Ok(())) => Ok(v),
        (Ok(_), Err(c)) => Err(c),
        (Err(e), Ok(())) => Err(e),
        (Err(e), Err(c)) => {
            eprintln!("임시 폴더 지우기도 실패 {}: {c}", work.display());
            Err(e)
        }
    }
}

fn measure(work: &Path, [n_stat, n_read, n_verify, n_list]: [usize; 4]) -> io::Result<Vec<Stat>> {
    let line = [vec![b'x'; LINE_BYTES - 1], b"\n".to_vec()].concat();
    let chunk = line.repeat(LINES_PER_READ);
    let path = work.join("s.jsonl");
    fs::write(&path, &chunk)?;
    let mut reader = TailReader::open(&path)?;
    while reader.changed()? {
        let p = reader.read_new_lines(MAX_READ_BYTES)?;
        reader.commit(p);
    }

    let mut v = Vec::with_capacity(n_stat);
    for _ in 0..n_stat {
        let t = Instant::now();
        let changed = reader.changed()?;
        v.push(t.elapsed());
        if changed {
            return Err(io::Error::other("안 바뀐 파일인데 changed() 가 true 다"));
        }
    }
    let s_stat = stat("handle_stat", "핸들 stat", v);

    let mut writer = fs::OpenOptions::new().append(true).open(&path)?;
    let mut v = Vec::with_capacity(n_read);
    for _ in 0..n_read {
        writer.write_all(&chunk)?;
        let t = Instant::now();
        let p = reader.read_new_lines(MAX_READ_BYTES)?;
        let (lines, reset) = (p.lines.len(), p.reset);
        reader.commit(p);
        v.push(t.elapsed());
        if reset || lines != LINES_PER_READ {
            return Err(io::Error::other(format!(
                "읽기가 어긋났다: 줄 {lines}, reset {reset}"
            )));
        }
    }
    drop(writer);
    let s_read = stat("handle_read", "핸들 읽기", v);

    let mut v = Vec::with_capacity(n_verify);
    for _ in 0..n_verify {
        let t = Instant::now();
        reader.verify_path()?;
        v.push(t.elapsed());
    }
    // 같은 파일이니 갈아끼울 핸들이 없어야 한다
    if reader.changed()? {
        return Err(io::Error::other("경로 대조가 같은 파일을 다른 파일로 봤다"));
    }
    drop(reader);
    let s_verify = stat("verify_path", "경로 대조", v);

    let site = work.join("site");
    for i in 0..PROJECTS {
        let project = site.join(format!("project-{i:02}"));
        fs::create_dir_all(&project)?;
        fs::write(project.join(format!("session-{i:02}.jsonl")), &line)?;
    }
    let site = Site {
        host: Host {
            id: "bench".into(),
            kind: HostKind::Windows,
            label: "bench".into(),
        },
        vendor: Vendor::Claude,
        root: site,
        read_mode: ReadMode::Append,
    };
    let mut v = Vec::with_capacity(n_list);
    for _ in 0..n_list {
        let t = Instant::now();
        let found = scheduler::list_site(&site, &mut 0)?.files.len();
        v.push(t.elapsed());
        if found != PROJECTS {
            return Err(io::Error::other(format!("나열에서 {found}개를 찾았다")));
        }
    }
    let s_list = stat("list", "나열", v);
    Ok(vec![s_stat, s_read, s_verify, s_list])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rel: &str) -> Vec<serde_json::Value> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(rel);
        let mut out = Vec::new();
        events(&[root], &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn has_tool_call(lines: &[serde_json::Value]) -> bool {
        lines
            .iter()
            .any(|v| v["event"]["kind"].get("ToolCall").is_some())
    }

    #[test]
    fn unhandled_는_합성_파일을_요약한다() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("unhandled.jsonl");
        let e = r#"{"kind":"tool","key":"Foo","first_seen":"2026-01-01T00:00:00Z","last_seen":"2026-01-01T00:00:00Z","count":3}"#;
        fs::write(
            &p,
            format!(
                "{e}
"
            ),
        )
        .unwrap();
        let mut out = Vec::new();
        unhandled(Some(p), &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Foo") && s.contains("총 3회"), "{s}");
    }

    #[test]
    fn 끝_개행_없는_바이트를_센다() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.jsonl");
        fs::write(
            &p,
            "{}
{}
abc",
        )
        .unwrap();
        assert_eq!(unread_bytes(&p, Some(3)).unwrap(), 3);
        assert_eq!(unread_bytes(&p, Some(6)).unwrap(), 0);
        fs::write(&p, "abc").unwrap();
        assert_eq!(unread_bytes(&p, None).unwrap(), 3);
    }

    #[test]
    fn bench_는_네_항목을_재고_임시_폴더를_지운다() {
        let tmp = tempfile::tempdir().unwrap();
        let stats = bench(tmp.path(), [5, 3, 4, 2]).unwrap();
        let got: Vec<_> = stats.iter().map(|s| (s.key, s.n)).collect();
        assert_eq!(
            got,
            [
                ("handle_stat", 5),
                ("handle_read", 3),
                ("verify_path", 4),
                ("list", 2)
            ]
        );
        for s in &stats {
            assert!(
                s.mean.is_finite() && s.p50 <= s.p99 && s.p99 <= s.max,
                "{}",
                s.key
            );
        }
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn 측정이_실패해도_임시_폴더를_지운다() {
        let tmp = tempfile::tempdir().unwrap();
        let r = in_scratch(tmp.path(), |work| {
            fs::write(work.join("x"), b"x")?;
            Err::<(), _>(io::Error::other("측정 실패"))
        });
        assert_eq!(r.unwrap_err().to_string(), "측정 실패");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn 이미_있는_측정_폴더_이름이면_거부하고_건드리지_않는다() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp
            .path()
            .join(format!("owlfice-bench-{}", std::process::id()));
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("남의것"), b"x").unwrap();
        let r = in_scratch(tmp.path(), |_| Ok(()));
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(existing.join("남의것").is_file());
    }

    #[test]
    fn claude_reject_는_줄마다_json이고_도구_호출이_있다() {
        let lines = run("claude/reject");
        assert!(has_tool_call(&lines), "{:?}", lines.first());
    }

    #[test]
    fn codex_app_은_줄마다_json이고_도구_호출이_있다() {
        let lines = run("codex/codex-app");
        assert!(has_tool_call(&lines), "{:?}", lines.first());
    }
}
