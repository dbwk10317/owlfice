//! owlfice-probe: 탐색 결과와 이벤트 열을 JSON으로 낸다 (PLAN 4.12)

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use owlfice_lib::diag::summarize;
use owlfice_lib::discover::{Vendor, discover_all, discover_wsl};
use owlfice_lib::parse::{Parser, RawRef};
use owlfice_lib::tail::scheduler::{self, is_subagent};
use owlfice_lib::tail::{MAX_READ_BYTES, TailReader};

const USAGE: &str = "사용법: owlfice-probe discover | owlfice-probe events <파일 또는 폴더>... | owlfice-probe unhandled [경로]";

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
