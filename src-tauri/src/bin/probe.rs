//! owlfice-probe: 탐색 결과와 이벤트 열을 JSON으로 낸다 (PLAN 4.12)

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use owlfice_lib::discover::{discover_all, discover_wsl};
use owlfice_lib::parse::claude::ClaudeParser;
use owlfice_lib::parse::codex::CodexParser;
use owlfice_lib::parse::{Parser, RawRef};
use owlfice_lib::tail::TailReader;

const USAGE: &str = "사용법: owlfice-probe discover | owlfice-probe events <파일 또는 폴더>...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = io::stdout().lock();
    let result = match args.first().map(String::as_str) {
        Some("discover") if args.len() == 1 => discover(&mut out),
        Some("events") if args.len() > 1 => {
            let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
            events(&paths, &mut out)
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

fn is_subagent(p: &Path) -> bool {
    p.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "subagents")
}

fn make_parser(path: &Path) -> Box<dyn Parser> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem.starts_with("rollout-") {
        // 파일명 끝 UUID (36자)
        let id = stem.get(stem.len().saturating_sub(36)..).unwrap_or(stem);
        return Box::new(CodexParser::new(id));
    }
    if is_subagent(path) {
        let meta = path.with_extension("meta.json");
        let depth = fs::read_to_string(meta)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["spawnDepth"].as_u64())
            .and_then(|d| u8::try_from(d).ok())
            .unwrap_or(1);
        let boss = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned());
        return Box::new(ClaudeParser::new(depth, boss));
    }
    Box::new(ClaudeParser::new(0, None))
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
    loop {
        let pending = reader.read_new_lines(8 << 20)?;
        if pending.lines.is_empty() {
            reader.commit(pending);
            return Ok(());
        }
        for line in &pending.lines {
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
