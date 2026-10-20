//! 미대응 동작 로그 (PLAN 4.11). 앱이 보고도 연출로 바꾸지 못한 것을 구조 정보만으로 모은다.
//! 저장 폴더는 인자로 받는다. 실제 경로 `%APPDATA%\Owlfice\logs` 는 부르는 쪽이 정한다.
//! 시각도 부르는 쪽이 넘긴다 (논리 시계와 같은 축).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::discover::Product;
use crate::parse::{Event, EventKind, ToolKind, ToolOutcome, Unhandled};

const FILE_NAME: &str = "unhandled.jsonl";
const MAX_BYTES: usize = 10 * 1024 * 1024;
const FLUSH_EVERY_SECS: i64 = 60;
/// 인자 모양이 끝없이 길어지지 않게 자른다
const SHAPE_TOKENS: usize = 24;
const RAW_COMMAND_CHARS: usize = 80;
const RAW_RESULT_CHARS: usize = 200;
/// 결과가 안 오는 호출이 쌓여도 메모리가 자라지 않게 하는 상한
const MAX_PENDING_TESTS: usize = 4096;

type Key = (String, String);

/// 한 줄. 같은 (kind, key) 는 합치고 count 만 올린다
#[derive(Serialize, Deserialize)]
struct Entry {
    kind: String,
    key: String,
    first_seen: String,
    last_seen: String,
    count: u64,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

pub struct Diag {
    path: PathBuf,
    /// 설정의 "원문 진단". 꺼져 있으면 원문은 어디에도 들어가지 않는다
    raw: bool,
    max_bytes: usize,
    /// 기존 파일을 읽지 못했으면 덮어쓰지 않는다
    writable: bool,
    entries: BTreeMap<Key, Entry>,
    /// Test 로 분류된 호출의 call_id → command_key. 결과가 올 때 계열을 정하는 재료
    test_calls: HashMap<String, String>,
    last_flush: Option<DateTime<Utc>>,
    dirty: bool,
}

fn parse_entries(bytes: &[u8]) -> impl Iterator<Item = Entry> {
    // 깨진 줄은 건너뛴다. 나머지는 이어 센다
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        .collect::<Vec<_>>()
        .into_iter()
}

fn load(path: &Path) -> (BTreeMap<Key, Entry>, bool) {
    match std::fs::read(path) {
        Ok(b) => (
            parse_entries(&b)
                .map(|e| ((e.kind.clone(), e.key.clone()), e))
                .collect(),
            true,
        ),
        Err(e) => (BTreeMap::new(), e.kind() == io::ErrorKind::NotFound),
    }
}

/// 임시 파일에 쓴 뒤 이름을 바꾼다. 쓰다 끊겨도 기존 파일이 깨지지 않는다
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn product_name(p: Product) -> &'static str {
    match p {
        Product::ClaudeDesktop => "claude_desktop",
        Product::ClaudeCli => "claude_cli",
        Product::CodexApp => "codex_app",
        Product::CodexCli => "codex_cli",
        Product::AntigravityApp => "antigravity_app",
        Product::AntigravityCli => "antigravity_cli",
    }
}

impl Diag {
    pub fn open(dir: &Path, raw: bool) -> Self {
        Self::open_with_limit(dir, raw, MAX_BYTES)
    }

    pub fn open_with_limit(dir: &Path, raw: bool, max_bytes: usize) -> Self {
        let path = dir.join(FILE_NAME);
        let (entries, writable) = load(&path);
        Self {
            path,
            raw,
            max_bytes,
            writable,
            entries,
            test_calls: HashMap::new(),
            last_flush: None,
            dirty: false,
        }
    }

    /// `raw` 는 원문 진단이 꺼져 있으면 부르는 쪽이 None 으로 넘긴다
    fn bump(
        &mut self,
        kind: &str,
        key: String,
        now: DateTime<Utc>,
        fields: Value,
        raw: Option<String>,
    ) {
        let ts = now.to_rfc3339_opts(SecondsFormat::Secs, true);
        let e = self
            .entries
            .entry((kind.to_string(), key.clone()))
            .or_insert_with(|| Entry {
                kind: kind.to_string(),
                key,
                first_seen: ts.clone(),
                last_seen: ts.clone(),
                count: 0,
                fields: Map::new(),
            });
        e.count += 1;
        if ts > e.last_seen {
            e.last_seen = ts;
        }
        if let Value::Object(m) = fields {
            for (k, v) in m {
                e.fields.entry(k).or_insert(v);
            }
        }
        if let Some(r) = raw {
            e.fields.entry("raw").or_insert(Value::String(r));
        }
        self.dirty = true;
        self.last_flush.get_or_insert(now);
    }

    /// 파서가 낸 미대응 재료
    pub fn observe_unhandled(&mut self, product: Product, u: &Unhandled, now: DateTime<Utc>) {
        let p = product_name(product);
        let (kind, key, fields) = match u {
            Unhandled::RecordType {
                type_name,
                keys,
                renamed_turn,
            } => (
                "record_type",
                format!("{p}:{type_name}"),
                json!({"product": p, "type": type_name, "keys": keys, "renamed_turn": renamed_turn}),
            ),
            Unhandled::SystemSubtype { subtype } => (
                "system_subtype",
                format!("{p}:{subtype}"),
                json!({"product": p, "subtype": subtype}),
            ),
            Unhandled::ToolName { name, input_keys } => (
                "tool_name",
                format!("{p}:{name}"),
                json!({"product": p, "name": name, "input_keys": input_keys}),
            ),
            Unhandled::OutcomeUnknown { tool, result_keys } => (
                "outcome_unknown",
                format!("{p}:{tool}"),
                json!({"product": p, "tool": tool, "result_keys": result_keys}),
            ),
            Unhandled::ProductValue { value } => (
                "product_value",
                format!("{p}:{value}"),
                json!({"product": p, "value": value}),
            ),
        };
        self.bump(kind, key, now, fields, None);
    }

    /// 이벤트에서 알 수 있는 kind: command, mcp_server, skill, result_shape
    pub fn observe_event(&mut self, ev: &Event, now: DateTime<Utc>) {
        match &ev.kind {
            EventKind::ToolCall {
                call_id,
                tool,
                summary,
            } => {
                let key = summary.command_key.clone().unwrap_or_default();
                match tool {
                    ToolKind::Shell => {
                        let norm = summary.command_norm.as_deref().unwrap_or_default();
                        if norm.is_empty() {
                            return;
                        }
                        let shape = arg_shape(norm, &key);
                        let raw = self
                            .raw
                            .then(|| norm.chars().take(RAW_COMMAND_CHARS).collect());
                        let fields = json!({"command_key": key, "shape": shape});
                        self.bump("command", shape, now, fields, raw);
                    }
                    ToolKind::Test => {
                        if self.test_calls.len() >= MAX_PENDING_TESTS {
                            self.test_calls.clear();
                        }
                        self.test_calls.insert(call_id.clone(), key);
                    }
                    ToolKind::Mcp { server, tool } => self.bump(
                        "mcp_server",
                        format!("{server}/{tool}"),
                        now,
                        json!({"server": server, "tool": tool}),
                        None,
                    ),
                    ToolKind::Skill { name } => {
                        self.bump("skill", name.clone(), now, json!({"name": name}), None)
                    }
                    _ => {}
                }
            }
            EventKind::ToolResult {
                call_id,
                outcome,
                preview,
                test_result,
                ..
            } => {
                let Some(key) = self.test_calls.remove(call_id) else {
                    return;
                };
                if test_result.is_some() || !matches!(outcome, ToolOutcome::Ok | ToolOutcome::Err) {
                    return;
                }
                let family = test_family(&key);
                let raw = self
                    .raw
                    .then(|| preview.chars().take(RAW_RESULT_CHARS).collect());
                let fields = json!({"command_key": key, "expected": family});
                self.bump("result_shape", format!("{key}|{family}"), now, fields, raw);
            }
            _ => {}
        }
    }

    /// 때가 됐으면(마지막 flush 에서 1분) 쓴다. 바뀐 게 없으면 아무것도 안 한다
    pub fn flush_if_due(&mut self, now: DateTime<Utc>) -> io::Result<()> {
        let due = self
            .last_flush
            .is_none_or(|t| now - t >= Duration::seconds(FLUSH_EVERY_SECS));
        if !self.dirty || !due {
            return Ok(());
        }
        // 실패해도 1분 뒤에 다시 시도한다. 매 호출마다 두드리지 않는다
        self.last_flush = Some(now);
        self.flush()
    }

    /// 통째로 쓴다. 실패하면 메모리 집계는 그대로 두고 에러를 돌려준다
    pub fn flush(&mut self) -> io::Result<()> {
        if !self.writable {
            return Err(io::Error::other(format!(
                "{} 을 읽지 못해 쓰지 않음",
                self.path.display()
            )));
        }
        self.evict();
        let mut out = String::new();
        for e in self.entries.values() {
            out.push_str(&serde_json::to_string(e)?);
            out.push('\n');
        }
        write_atomic(&self.path, out.as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    /// 상한을 넘으면 last_seen 이 가장 오래된 것부터 지운다
    fn evict(&mut self) {
        let mut sizes: Vec<(String, Key, usize)> = self
            .entries
            .iter()
            .map(|(k, e)| {
                let len = serde_json::to_string(e).map_or(0, |s| s.len() + 1);
                (e.last_seen.clone(), k.clone(), len)
            })
            .collect();
        let mut total: usize = sizes.iter().map(|s| s.2).sum();
        if total <= self.max_bytes {
            return;
        }
        sizes.sort();
        for (_, k, len) in sizes {
            if total <= self.max_bytes {
                break;
            }
            self.entries.remove(&k);
            total -= len;
        }
    }
}

/// `owlfice-probe unhandled` 가 쓸 요약: kind 별(총 횟수 순), 안에서 count 순
pub fn summarize(path: &Path) -> io::Result<String> {
    const EMPTY: &str = "미대응 기록 없음\n";
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(EMPTY.into()),
        Err(e) => return Err(e),
    };
    let mut by_kind: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for e in parse_entries(&bytes) {
        by_kind.entry(e.kind.clone()).or_default().push(e);
    }
    if by_kind.is_empty() {
        return Ok(EMPTY.into());
    }
    let mut groups: Vec<(String, u64, Vec<Entry>)> = by_kind
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
            let total = v.iter().map(|e| e.count).sum();
            (k, total, v)
        })
        .collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut out = String::new();
    for (kind, total, items) in groups {
        out.push_str(&format!("{kind}  ({}종, 총 {total}회)\n", items.len()));
        for e in items {
            out.push_str(&format!("  {:>6}  {}\n", e.count, e.key));
        }
    }
    Ok(out)
}

/// 정규식 계열 이름. 파서의 테스트 결과 정규식이 어느 계열을 기대했는지 command_key 로 정한다
fn test_family(command_key: &str) -> &'static str {
    let mut w = command_key.split_whitespace();
    match w.next() {
        Some("cargo") => "cargo",
        Some("pytest") => "pytest",
        Some("jest" | "vitest") => "jest",
        Some("npm" | "pnpm" | "yarn" | "bun") if w.any(|t| t.starts_with("test")) => "jest",
        _ => "none",
    }
}

fn is_ident(t: &str) -> bool {
    t.starts_with(|c: char| c.is_ascii_alphabetic())
        && t.len() <= 24
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `main.rs` 처럼 확장자가 붙은 단어
fn is_file_name(t: &str) -> bool {
    t.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 5)
}

/// 명령의 인자 모양. 세그먼트 첫 토큰·command_key 단어·연산자·플래그 이름만 남기고
/// 나머지 값은 `<arg>`·`<path>`·`<num>` 로 바꾼다.
/// ponytail: `-pSECRET` 처럼 붙여 쓴 단일 대시 플래그를 가를 수 없어 단일 대시는 4글자까지만 남긴다, 필요해지면 도구별 목록을 둔다
fn arg_shape(norm: &str, command_key: &str) -> String {
    let key_words: Vec<&str> = command_key.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut head = true;
    for (i, t) in norm.split_whitespace().enumerate() {
        if i >= SHAPE_TOKENS {
            out.push("…".into());
            break;
        }
        if matches!(t, "&&" | "||" | "|" | "|&" | ";" | "&") {
            out.push(t.into());
            head = true;
            continue;
        }
        let was_head = std::mem::replace(&mut head, false);
        if t.starts_with('-') && t.len() > 1 {
            let (name, has_value) = match t.split_once('=') {
                Some((n, _)) => (n, true),
                None => (t, false),
            };
            let body = name.trim_start_matches('-');
            let plain = !body.is_empty()
                && body
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
            if plain && (name.starts_with("--") || body.len() <= 4) {
                out.push(if has_value {
                    format!("{name}=<arg>")
                } else {
                    name.into()
                });
            } else {
                out.push("<arg>".into());
            }
        } else if is_ident(t) && (was_head || key_words.contains(&t)) {
            out.push(t.into());
        } else if t.parse::<f64>().is_ok() {
            out.push("<num>".into());
        } else if t.contains(['/', '\\']) || t.starts_with(['.', '~']) || is_file_name(t) {
            out.push("<path>".into());
        } else {
            out.push("<arg>".into());
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::TimeZone;

    use super::*;
    use crate::parse::claude::ClaudeParser;
    use crate::parse::codex::CodexParser;
    use crate::parse::{Parser, RawRef};

    const SECRET: &str = "SECRET_PROMPT_7f3a";

    fn t(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + sec, 0).unwrap()
    }

    fn feed(p: &mut dyn Parser, lines: &[String]) -> (Vec<Event>, Vec<Unhandled>) {
        let (mut ev, mut un) = (Vec::new(), Vec::new());
        for (i, l) in lines.iter().enumerate() {
            let raw = RawRef {
                path: PathBuf::from("synthetic.jsonl"),
                generation: 0,
                byte_offset: 0,
                line_no: i as u64 + 1,
            };
            ev.extend(p.parse_line(l.as_bytes(), &raw, &mut un));
        }
        (ev, un)
    }

    fn claude_tool_use(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn claude_result(id: &str, text: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{text}"}}]}}}}"#
        )
    }

    fn claude_lines() -> Vec<String> {
        vec![
            format!(
                r#"{{"type":"user","cwd":"C:\\w","entrypoint":"mystery-ep","sessionId":"00000000-0000-4000-8000-000000000001","message":{{"role":"user","content":"{SECRET}"}}}}"#
            ),
            format!(r#"{{"type":"brand-new","x":"{SECRET}"}}"#),
            r#"{"type":"system","subtype":"brand_new"}"#.into(),
            claude_tool_use(
                "toolu_fake01",
                "FrobTool",
                &format!(r#"{{"a":"{SECRET}"}}"#),
            ),
            claude_result("toolu_fake01", "ok"),
            claude_tool_use(
                "toolu_fake02",
                "Bash",
                &format!(r#"{{"command":"mytool build {SECRET} --fast --out=C:\\x\\y.bin 12"}}"#),
            ),
            claude_result("toolu_fake02", &format!("{SECRET}_RESULT")),
            claude_tool_use("toolu_fake03", "Bash", r#"{"command":"cargo test -p x"}"#),
            claude_result("toolu_fake03", &format!("weird {SECRET}_RESULT output")),
            claude_tool_use(
                "toolu_fake04",
                "mcp__github__create_issue",
                r#"{"title":"t"}"#,
            ),
            claude_tool_use("toolu_fake05", "Skill", r#"{"skill":"brainstorm"}"#),
        ]
    }

    fn codex_lines() -> Vec<String> {
        let rec = |ty: &str, payload: Value| {
            json!({"timestamp": "2026-01-01T00:00:00Z", "type": ty, "payload": payload}).to_string()
        };
        vec![
            rec(
                "session_meta",
                json!({"id": "s1", "cwd": "/w", "originator": "Mystery Desktop", "source": "vscode"}),
            ),
            rec("brand_new_kind", json!({"x": SECRET})),
            rec(
                "response_item",
                json!({"type": "custom_tool_call", "name": "exec", "call_id": "c1",
                       "input": format!("tools.exec_command({{cmd:\"othertool go {SECRET}\"}})")}),
            ),
            rec(
                "response_item",
                json!({"type": "function_call", "name": "update_plan", "call_id": "o1",
                       "arguments": json!({"plan": SECRET}).to_string()}),
            ),
            rec(
                "response_item",
                json!({"type": "function_call_output", "call_id": "o1", "output": SECRET}),
            ),
        ]
    }

    fn collect(d: &mut Diag, product: Product, ev: &[Event], un: &[Unhandled], at: i64) {
        for u in un {
            d.observe_unhandled(product, u, t(at));
        }
        for e in ev {
            d.observe_event(e, t(at));
        }
    }

    fn run_all(d: &mut Diag) {
        let (ev, un) = feed(&mut ClaudeParser::new(0, None), &claude_lines());
        collect(d, Product::ClaudeCli, &ev, &un, 0);
        let (ev, un) = feed(&mut CodexParser::new("s1"), &codex_lines());
        collect(d, Product::CodexCli, &ev, &un, 1);
    }

    fn read(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(FILE_NAME)).unwrap()
    }

    #[test]
    fn real_parser_output_fills_every_kind_without_raw_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), false);
        run_all(&mut d);
        d.flush().unwrap();
        let text = read(dir.path());
        for k in [
            "record_type",
            "system_subtype",
            "tool_name",
            "command",
            "mcp_server",
            "skill",
            "result_shape",
            "outcome_unknown",
            "product_value",
        ] {
            assert!(
                text.contains(&format!(r#""kind":"{k}""#)),
                "{k} 없음:\n{text}"
            );
        }
        assert!(!text.contains("SECRET"), "원문이 저장됨:\n{text}");
        assert!(
            text.contains("mytool <arg> <arg> --fast --out=<arg> <num>"),
            "{text}"
        );
        assert!(text.contains(r#""expected":"cargo""#), "{text}");
        assert!(!text.contains("\"raw\""));
    }

    #[test]
    fn raw_on_adds_command_80_and_result_200() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), true);
        run_all(&mut d);
        d.flush().unwrap();
        let text = read(dir.path());
        assert!(
            text.contains(&format!("mytool build {SECRET} --fast")),
            "{text}"
        );
        assert!(
            text.contains(&format!("weird {SECRET}_RESULT output")),
            "{text}"
        );

        let long = format!("mytool {}", "a".repeat(200));
        let line = claude_tool_use("t1", "Bash", &json!({"command": long}).to_string());
        let (ev, _) = feed(&mut ClaudeParser::new(0, None), &[line]);
        let dir2 = tempfile::tempdir().unwrap();
        let mut d2 = Diag::open(dir2.path(), true);
        collect(&mut d2, Product::ClaudeCli, &ev, &[], 0);
        d2.flush().unwrap();
        let v: Value = serde_json::from_str(read(dir2.path()).trim()).unwrap();
        assert_eq!(v["raw"].as_str().unwrap().chars().count(), 80);
    }

    #[test]
    fn same_key_merges_and_reopen_continues_counting() {
        let dir = tempfile::tempdir().unwrap();
        let u = Unhandled::ToolName {
            name: "Frob".into(),
            input_keys: vec!["a".into()],
        };
        let mut d = Diag::open(dir.path(), false);
        d.observe_unhandled(Product::ClaudeCli, &u, t(0));
        d.observe_unhandled(Product::ClaudeCli, &u, t(5));
        d.flush().unwrap();
        let text = read(dir.path());
        assert_eq!(text.lines().count(), 1);
        let v: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["count"], 2);
        assert_ne!(v["first_seen"], v["last_seen"]);

        let mut d = Diag::open(dir.path(), false);
        d.observe_unhandled(Product::ClaudeCli, &u, t(9));
        d.flush().unwrap();
        let v: Value = serde_json::from_str(read(dir.path()).trim()).unwrap();
        assert_eq!(v["count"], 3);
    }

    #[test]
    fn over_limit_drops_oldest_last_seen_first() {
        let dir = tempfile::tempdir().unwrap();
        let u = |n: &str| Unhandled::ToolName {
            name: n.into(),
            input_keys: vec![],
        };
        let mut d = Diag::open_with_limit(dir.path(), false, 600);
        for (i, n) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            d.observe_unhandled(Product::ClaudeCli, &u(n), t(i as i64));
        }
        // 가장 오래된 a 를 다시 보면 최신이 된다
        d.observe_unhandled(Product::ClaudeCli, &u("a"), t(10));
        d.flush().unwrap();
        let text = read(dir.path());
        assert!(text.len() <= 600, "{}", text.len());
        assert!(text.contains(r#""name":"a""#), "{text}");
        assert!(!text.contains(r#""name":"b""#), "{text}");
        assert!(text.contains(r#""name":"f""#), "{text}");
    }

    #[test]
    fn write_failure_keeps_memory_and_flush_waits_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        let mut d = Diag::open(&blocker.join("sub"), false);
        let u = Unhandled::SystemSubtype {
            subtype: "s".into(),
        };
        d.observe_unhandled(Product::ClaudeCli, &u, t(0));
        assert!(d.flush_if_due(t(30)).is_ok(), "1분 전에는 쓰지 않는다");
        assert!(d.flush_if_due(t(60)).is_err());
        assert_eq!(d.entries.len(), 1);
        assert!(d.dirty);
    }

    #[test]
    fn summary_groups_by_kind_then_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), false);
        let u = |n: &str| Unhandled::ToolName {
            name: n.into(),
            input_keys: vec![],
        };
        d.observe_unhandled(Product::ClaudeCli, &u("rare"), t(0));
        for _ in 0..3 {
            d.observe_unhandled(Product::ClaudeCli, &u("common"), t(1));
        }
        let s = Unhandled::SystemSubtype {
            subtype: "s".into(),
        };
        d.observe_unhandled(Product::ClaudeCli, &s, t(2));
        d.flush().unwrap();
        let s = summarize(&dir.path().join(FILE_NAME)).unwrap();
        assert!(s.starts_with("tool_name  (2종, 총 4회)\n"), "{s}");
        assert!(s.find("claude_cli:common").unwrap() < s.find("claude_cli:rare").unwrap());
        assert_eq!(
            summarize(&dir.path().join("none")).unwrap(),
            "미대응 기록 없음\n"
        );
    }
}
