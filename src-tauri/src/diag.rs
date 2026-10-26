//! 미대응 동작 로그 (docs/CONTRACTS.md C11). 앱이 보고도 연출로 바꾸지 못한 것을 구조 정보만으로 모은다.
//! 저장 폴더는 인자로 받는다. 실제 경로 `%APPDATA%\Owlfice\logs` 는 부르는 쪽이 정한다.
//! 시각도 부르는 쪽이 넘긴다 (논리 시계와 같은 축).

use std::collections::btree_map::Entry::{Occupied, Vacant};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::discover::HostKind;
use crate::discover::Product;
use crate::parse::{Event, EventKind, ToolKind, ToolOutcome, Unhandled};
use crate::runtime::Diagnostics;

const FILE_NAME: &str = "unhandled.jsonl";
const MAX_BYTES: usize = 10 * 1024 * 1024;
const FLUSH_EVERY_SECS: i64 = 60;
/// 인자 모양이 끝없이 길어지지 않게 자른다
const SHAPE_TOKENS: usize = 24;
/// 낱말·플래그 이름으로 남기는 길이 상한. 붙여 쓴 비밀이 이름으로 남지 않게 한다
const IDENT_MAX: usize = 24;
const RAW_COMMAND_CHARS: usize = 80;
const RAW_RESULT_CHARS: usize = 200;
/// 결과가 안 오는 호출이 쌓여도 메모리가 자라지 않게 하는 상한
const MAX_PENDING_TESTS: usize = 4096;

fn hash8(text: &str) -> String {
    format!("{:016x}", crate::office::names::fnv1a64(text))[..8].to_owned()
}

/// 원문 로그를 섞지 않고 요청으로 모은 구조 정보만 내보낸다.
pub fn diagnostic_text(data: &Diagnostics, home: Option<&Path>, projects: &[PathBuf]) -> String {
    use std::fmt::Write;
    let roots: Vec<_> = data
        .candidates
        .iter()
        .filter_map(|c| c.root.as_deref())
        .chain(
            data.hosts
                .iter()
                .flat_map(|h| h.sites.iter().map(|s| s.root.as_path())),
        )
        .collect();
    let project_names: Vec<_> = projects
        .iter()
        .filter_map(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_lowercase())
        .collect();
    let path = |path: &Path| {
        let (prefix, tail) = if let Some(tail) = home.and_then(|h| path.strip_prefix(h).ok()) {
            ("<home>".to_owned(), tail)
        } else if let Some(root) = roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.as_os_str().len())
        {
            (
                format!("<root:{}>", hash8(&root.to_string_lossy())),
                path.strip_prefix(root).unwrap(),
            )
        } else {
            // 알 수 없는 접두어를 홈으로 추정하면 수동 경로의 사용자명이 남을 수 있다.
            return format!("<path:{}>", hash8(&path.to_string_lossy()));
        };
        let mut out = prefix;
        let mut project = false;
        for part in tail.components() {
            let part = part.as_os_str().to_string_lossy();
            let known = matches!(
                part.as_ref(),
                ".claude" | ".codex" | "projects" | "sessions" | "subagents"
            );
            let safe = known && !project && !project_names.contains(&part.to_lowercase());
            out.push('/');
            if safe {
                out.push_str(&part);
            } else {
                out.push_str(&hash8(&part));
                if part.ends_with(".jsonl") {
                    out.push_str(".jsonl");
                }
            }
            project = part == "projects";
        }
        out
    };
    let host = |host: &crate::discover::Host| match &host.kind {
        HostKind::Windows => "windows".to_owned(),
        HostKind::Wsl { distro } => format!("wsl:{}", hash8(distro)),
    };
    let mut out = String::from("Owlfice diagnostics\n");
    for candidate in &data.candidates {
        let root = candidate
            .root
            .as_deref()
            .map(path)
            .unwrap_or_else(|| "<unknown>".into());
        let _ = writeln!(
            out,
            "candidate {} {} {:?} {root}",
            host(&candidate.host),
            candidate.vendor.label(),
            candidate.result
        );
    }
    for snapshot in &data.hosts {
        let id = host(&snapshot.host);
        let ticks = &snapshot.ticks;
        let average = ticks
            .total_us
            .checked_div(u128::from(ticks.count))
            .unwrap_or(0);
        let _ = writeln!(
            out,
            "host {id} suspect={} ticks={} average_us={average} max_us={} last_us={} path_ops={}",
            snapshot.suspect, ticks.count, ticks.max_us, ticks.last_us, snapshot.path_ops
        );
        for site in &snapshot.sites {
            let _ = writeln!(
                out,
                "site {id} {} {}",
                site.vendor.label(),
                path(&site.root)
            );
        }
        for file in &snapshot.files {
            let _ = writeln!(
                out,
                "file {id} {} team={} {} offset={} generation={}",
                file.team.vendor.label(),
                hash8(&file.team.session),
                path(&file.path),
                file.offset,
                file.generation
            );
        }
    }
    for id in &data.unavailable {
        let id = if id == "windows" {
            id.clone()
        } else {
            format!("wsl:{}", hash8(id.strip_prefix("wsl:").unwrap_or(id)))
        };
        let _ = writeln!(out, "host {id} unavailable");
    }
    out
}

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

/// 같은 (kind, key) 가 여러 줄이면 합친다. raw 를 끄고 열면 예전 raw 도 버린다.
/// 돌려주는 값: (집계, 쓸 수 있는가, 파일에 있던 raw 를 뗐는가)
fn load(path: &Path, raw: bool) -> (BTreeMap<Key, Entry>, bool, bool) {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return (BTreeMap::new(), e.kind() == io::ErrorKind::NotFound, false),
    };
    let mut map: BTreeMap<Key, Entry> = BTreeMap::new();
    let mut stripped = false;
    for mut e in parse_entries(&b) {
        if !raw {
            stripped |= e.fields.remove("raw").is_some();
        }
        merge_entry(&mut map, e);
    }
    (map, true, stripped)
}

fn merge_entry(map: &mut BTreeMap<Key, Entry>, e: Entry) {
    match map.entry((e.kind.clone(), e.key.clone())) {
        Occupied(mut o) => {
            let m = o.get_mut();
            m.count = m.count.saturating_add(e.count);
            if e.first_seen < m.first_seen {
                m.first_seen = e.first_seen;
            }
            if e.last_seen > m.last_seen {
                m.last_seen = e.last_seen;
            }
            for (k, v) in e.fields {
                m.fields.entry(k).or_insert(v);
            }
        }
        Vacant(v) => {
            v.insert(e);
        }
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

    /// 끈 뒤 다음 flush가 예전에 모은 원문을 다시 쓰지 않도록 메모리에서도 지운다.
    pub fn set_raw(&mut self, raw: bool) -> io::Result<()> {
        self.raw = raw;
        if !raw {
            for entry in self.entries.values_mut() {
                self.dirty |= entry.fields.remove("raw").is_some();
            }
            // 다음 정기 저장 전에 앱이 꺼져도 파일에 원문이 남지 않게 바로 쓴다
            if self.dirty || !self.writable {
                self.purge_now()?;
            }
        }
        Ok(())
    }

    /// 실패해도 dirty 는 그대로라 다음 flush_if_due 가 다시 시도한다
    fn purge_now(&mut self) -> io::Result<()> {
        if !self.writable {
            let (mut entries, writable, _) = load(&self.path, false);
            if writable {
                // 시작 때 못 읽은 원장을 복구한 뒤, 그동안 메모리에 모은 집계도 보존한다.
                for entry in std::mem::take(&mut self.entries).into_values() {
                    merge_entry(&mut entries, entry);
                }
                self.entries = entries;
                self.writable = true;
                self.dirty = true;
            }
        }
        self.flush().inspect_err(|e| {
            crate::oplog_once!("원문 진단 제거 저장 실패 {}: {e}", self.path.display());
        })
    }

    fn open_with_limit(dir: &Path, raw: bool, max_bytes: usize) -> Self {
        let path = dir.join(FILE_NAME);
        let (entries, writable, stripped) = load(&path, raw);
        let mut d = Self {
            path,
            raw,
            max_bytes,
            writable,
            entries,
            test_calls: HashMap::new(),
            last_flush: None,
            dirty: stripped,
        };
        // 껐는데 파일에 원문이 남아 있으면 재시작 때 바로 지운다
        if stripped {
            let _ = d.purge_now();
        }
        d
    }

    /// 원문 진단이 꺼져 있으면 원문이 어디에도 안 들어가도록 부르는 쪽이 `raw` 를 None 으로 넘긴다
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

    /// 파서·상태 기계가 낸 미대응 재료
    pub fn observe_unhandled(&mut self, product: Product, u: &Unhandled, now: DateTime<Utc>) {
        let p = product_name(product);
        let (kind, key, fields) = match u {
            Unhandled::InvalidJson => ("invalid_json", p.to_owned(), json!({"product": p})),
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
            Unhandled::StateConflict { winner, masked } => (
                "state_conflict",
                format!("{p}:{winner}>{masked}"),
                json!({"product": p, "winner": winner, "masked": masked}),
            ),
            Unhandled::ScenarioGap {
                team,
                from,
                to,
                via,
            } => (
                "scenario_gap",
                format!("{}{from}>{to}@{via}", if *team { "team:" } else { "" }),
                json!({"from": from, "to": to, "via": via}),
            ),
        };
        self.bump(kind, key, now, fields, None);
    }

    pub fn observe_event(&mut self, ev: &Event, now: DateTime<Utc>) {
        match &ev.kind {
            EventKind::ToolCall {
                call_id,
                tool,
                summary,
            } => {
                let key = summary
                    .command_key
                    .as_deref()
                    .map(safe_key)
                    .unwrap_or_default();
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
                        if self.test_calls.len() < MAX_PENDING_TESTS {
                            self.test_calls.insert(call_id.clone(), key);
                        }
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

    /// 통째로 쓴다. 쓰기에 실패하면 상한 초과분도 메모리에 남겨 다음 flush 에서 다시 시도한다
    pub fn flush(&mut self) -> io::Result<()> {
        let (out, dropped) = self.render()?;
        if !self.writable {
            // 못 읽은 파일은 덮어쓰지 않지만 메모리 집계가 끝없이 자라면 안 된다
            self.forget(dropped);
            return Err(io::Error::other(format!(
                "{} 을 읽지 못해 쓰지 않음",
                self.path.display()
            )));
        }
        write_atomic(&self.path, out.as_bytes())?;
        self.forget(dropped);
        self.dirty = false;
        Ok(())
    }

    fn forget(&mut self, keys: Vec<Key>) {
        for k in keys {
            self.entries.remove(&k);
        }
    }

    /// 항목마다 한 번만 직렬화해 상한 계산과 출력에 함께 쓴다.
    /// 상한을 넘으면 last_seen 이 가장 오래된 것부터 빼고, 뺀 키를 함께 돌려준다
    fn render(&self) -> io::Result<(String, Vec<Key>)> {
        let mut rows: Vec<(&str, &Key, String)> = Vec::with_capacity(self.entries.len());
        for (k, e) in &self.entries {
            rows.push((&e.last_seen, k, serde_json::to_string(e)?));
        }
        let mut total: usize = rows.iter().map(|r| r.2.len() + 1).sum();
        let mut dropped = Vec::new();
        if total > self.max_bytes {
            rows.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));
            let mut cut = 0;
            while cut < rows.len() && total > self.max_bytes {
                total -= rows[cut].2.len() + 1;
                dropped.push(rows[cut].1.clone());
                cut += 1;
            }
            rows.drain(..cut);
            rows.sort_by(|a, b| a.1.cmp(b.1));
        }
        let mut out = String::with_capacity(total);
        for (_, _, line) in rows {
            out.push_str(&line);
            out.push('\n');
        }
        Ok((out, dropped))
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

/// Test 호출로 확인된 키만 받는다. 패키지 스크립트 이름으로 내부 실행기를 추측하지 않는다.
fn test_family(command_key: &str) -> &'static str {
    let mut w = command_key.split_whitespace();
    match w.next() {
        Some("cargo") => "cargo",
        Some("pytest") => "pytest",
        Some("jest" | "vitest") => "jest",
        Some("node") => "node",
        Some("npm" | "pnpm" | "yarn" | "bun") if w.any(|t| t.starts_with("test")) => "test-summary",
        _ => "none",
    }
}

fn is_ident(t: &str) -> bool {
    t.starts_with(|c: char| c.is_ascii_alphabetic())
        && t.len() <= IDENT_MAX
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `main.rs` 처럼 확장자가 붙은 단어
fn is_file_name(t: &str) -> bool {
    t.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 5)
}

/// 파서가 준 command_key 에 공백 든 경로 조각 등이 섞였을 수 있어 낱말 모양이 아닌 것은 가린다
fn safe_key(key: &str) -> String {
    key.split_whitespace()
        .map(|w| if is_ident(w) { w } else { "<arg>" })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 따옴표 안의 공백·연산자가 토큰 경계나 연산자로 읽히지 않게 따옴표를 따라가며 나눈다.
/// 짝 없는 따옴표는 끝까지 한 토큰으로 삼는다 (가리는 쪽으로 틀린다). 둘째 값은 따옴표가 든 토큰인지
fn split_shell(s: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let (mut cur, mut quoted, mut open) = (String::new(), false, None::<char>);
    for c in s.chars() {
        match open {
            Some(close) => {
                cur.push(c);
                if c == close {
                    open = None;
                }
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), quoted));
                    quoted = false;
                }
            }
            None => {
                if matches!(c, '"' | '\'') {
                    open = Some(c);
                    quoted = true;
                }
                cur.push(c);
            }
        }
    }
    if !cur.is_empty() {
        out.push((cur, quoted));
    }
    out
}

/// 명령의 인자 모양. 세그먼트 첫 토큰·command_key 단어·연산자·플래그 이름만 남기고
/// 나머지 값은 `<arg>`·`<path>`·`<num>` 로 바꾼다.
/// ponytail: `-pSECRET` 처럼 붙여 쓴 단일 대시 플래그를 가를 수 없어 단일 대시는 4글자까지만 남긴다, 필요해지면 도구별 목록을 둔다
fn arg_shape(norm: &str, command_key: &str) -> String {
    let key_words: Vec<&str> = command_key.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut head = true;
    for (i, (t, quoted)) in split_shell(norm).iter().enumerate() {
        let (t, quoted) = (t.as_str(), *quoted);
        if i >= SHAPE_TOKENS {
            out.push("…".into());
            break;
        }
        if !quoted && matches!(t, "&&" | "||" | "|" | "|&" | ";" | "&") {
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
            let max = if name.starts_with("--") { IDENT_MAX } else { 4 };
            if plain && body.len() <= max {
                out.push(if has_value {
                    format!("{name}=<arg>")
                } else {
                    name.into()
                });
            } else {
                out.push("<arg>".into());
            }
        } else if quoted {
            out.push("<arg>".into());
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

    #[test]
    fn copied_diagnostics_hide_home_projects_and_unknown_manual_prefixes() {
        use crate::discover::{
            Candidate, CandidateResult, Host, ReadMode, Site, Vendor, windows_host,
        };
        use crate::runtime::{FileDiagnostic, HostDiagnostic, TickStats};
        let home = PathBuf::from(r"C:\Users\private-user");
        let root = home.join(".claude/projects");
        let manual = PathBuf::from(r"D:\secret-prefix\private-project\records");
        let remote = Host {
            id: "wsl:private-distro".into(),
            kind: HostKind::Wsl {
                distro: "private-distro".into(),
            },
            label: "private-distro".into(),
        };
        let site = Site {
            host: windows_host(),
            vendor: Vendor::Claude,
            root: root.clone(),
            read_mode: ReadMode::Append,
        };
        let data = Diagnostics {
            candidates: vec![
                Candidate {
                    host: windows_host(),
                    vendor: Vendor::Claude,
                    root: Some(root.clone()),
                    result: CandidateResult::Found,
                },
                Candidate {
                    host: windows_host(),
                    vendor: Vendor::Codex,
                    root: Some(manual.clone()),
                    result: CandidateResult::Found,
                },
                Candidate {
                    host: remote,
                    vendor: Vendor::Claude,
                    root: Some(PathBuf::from(
                        r"\\wsl$\private-distro\home\private-user\.claude\projects",
                    )),
                    result: CandidateResult::Offline,
                },
            ],
            hosts: vec![HostDiagnostic {
                host: windows_host(),
                sites: vec![site],
                files: vec![FileDiagnostic {
                    team: crate::office::world::TeamKey {
                        host: "windows".into(),
                        vendor: Vendor::Claude,
                        session: "secret-session".into(),
                    },
                    path: root.join("private-project/secret-session.jsonl"),
                    offset: 1234,
                    generation: 2,
                }],
                ticks: TickStats {
                    count: 2,
                    total_us: 60,
                    last_us: 40,
                    max_us: 40,
                },
                path_ops: 3,
                suspect: false,
            }],
            unavailable: vec!["wsl:private-distro".into()],
        };
        let text = diagnostic_text(&data, Some(&home), &[PathBuf::from("D:/private-project")]);
        for private in [
            "private-user",
            "private-project",
            "secret-prefix",
            "private-distro",
            "secret-session",
        ] {
            assert!(!text.contains(private), "{text}");
        }
        assert!(text.contains("<home>/.claude/projects"));
        assert!(text.contains(&hash8("private-project")));
        assert!(text.contains("<root:"));
        assert!(text.contains("offset=1234 generation=2"));
        assert!(text.contains("ticks=2 average_us=30 max_us=40 last_us=40"));
        assert!(text.contains("Offline"));
        assert!(text.contains("unavailable"));
    }

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
    fn indirect_node_test_summaries_only_diagnose_incomplete_results() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), false);
        let mut parser = ClaudeParser::new(0, None);
        let complete =
            r"ℹ tests 2\nℹ suites 0\nℹ pass 1\nℹ fail 1\nℹ cancelled 0\nℹ skipped 0\nℹ todo 0";
        let (events, _) = feed(
            &mut parser,
            &[
                claude_tool_use("complete", "Bash", r#"{"command":"npm test"}"#),
                claude_result("complete", complete),
            ],
        );
        for event in &events {
            d.observe_event(event, t(0));
        }
        d.flush().unwrap();
        assert!(read(dir.path()).is_empty());

        for (id, command) in [
            ("indirect", "npm test"),
            ("direct", "node --test sample.test.js"),
            ("script", "node -e '1 + 1'"),
        ] {
            let (events, _) = feed(
                &mut parser,
                &[
                    claude_tool_use(id, "Bash", &json!({"command": command}).to_string()),
                    claude_result(id, r"✔ example one\n▶ example group"),
                ],
            );
            for event in &events {
                d.observe_event(event, t(1));
            }
        }
        d.flush().unwrap();
        let rows: Vec<Value> = read(dir.path())
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2, "{rows:?}");
        for (key, expected) in [("npm test", "test-summary"), ("node", "node")] {
            let row = rows.iter().find(|row| row["command_key"] == key).unwrap();
            assert_eq!(row["kind"], "result_shape");
            assert_eq!(row["expected"], expected);
            assert_eq!(row["count"], 1);
            assert!(row.get("raw").is_none());
        }
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

    /// 끄기만 해도 다음 정기 저장에서 원문이 사라지고 집계는 이어진다.
    #[test]
    fn runtime_raw_toggle_removes_raw_and_keeps_counts_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), false);
        d.set_raw(true).unwrap();
        run_all(&mut d);
        d.flush().unwrap();
        let before = read(dir.path());
        assert!(before.contains(SECRET));
        let mut expected: Vec<Value> = before
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for entry in &mut expected {
            entry.as_object_mut().unwrap().remove("raw");
        }
        let count: u64 = expected
            .iter()
            .map(|entry| entry["count"].as_u64().unwrap())
            .sum();
        d.set_raw(false).unwrap();
        d.flush_if_due(t(61)).unwrap();
        let after = read(dir.path());
        let actual: Vec<Value> = after
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(actual, expected);
        run_all(&mut d);
        d.flush().unwrap();
        drop(d);

        // 켠 상태로 다시 열어도 디스크에서 원문이 되살아나지 않아야 한다.
        let mut reopened = Diag::open(dir.path(), true);
        reopened.flush().unwrap();
        let after = read(dir.path());
        assert!(!after.contains(SECRET));
        assert!(!after.contains("\"raw\""));
        let actual_count: u64 = after
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["count"]
                    .as_u64()
                    .unwrap()
            })
            .sum();
        assert_eq!(actual_count, count * 2);
    }

    /// 끄는 즉시 파일에서 원문이 사라지고 집계는 그대로다 (flush_if_due 없이).
    #[test]
    fn raw_off_purges_file_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), false);
        d.set_raw(true).unwrap();
        run_all(&mut d);
        d.flush().unwrap();
        let before = read(dir.path());
        assert!(before.contains(SECRET));
        let count = |text: &str| -> u64 {
            text.lines()
                .map(|l| {
                    serde_json::from_str::<Value>(l).unwrap()["count"]
                        .as_u64()
                        .unwrap()
                })
                .sum()
        };
        d.set_raw(false).unwrap();
        let after = read(dir.path());
        assert!(!after.contains(SECRET));
        assert!(!after.contains("\"raw\""));
        assert_eq!(count(&after), count(&before));
    }

    /// 원문이 든 파일을 끈 채 열기만 해도 파일에서 원문이 지워진다.
    #[test]
    fn open_with_raw_off_purges_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"{"kind":"skill","key":"x","first_seen":"2026-01-01T00:00:00Z","last_seen":"2026-01-02T00:00:00Z","count":4,"raw":"LEAK"}
"#;
        std::fs::write(dir.path().join(FILE_NAME), text).unwrap();
        let _d = Diag::open(dir.path(), false);
        let out = read(dir.path());
        assert!(!out.contains("LEAK"));
        assert!(!out.contains("\"raw\""));
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["count"], 4);
    }

    /// 디스크 삭제 실패를 알려도 수집은 멈추고, 잠금이 풀린 뒤 다시 끄면 원문을 제거한다.
    #[test]
    fn failed_purge_stops_collection_and_can_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), true);
        run_all(&mut d);
        d.flush().unwrap();
        let blocked = d.path.with_extension("jsonl.tmp");
        std::fs::create_dir(&blocked).unwrap();

        assert!(d.set_raw(false).is_err());
        assert!(read(dir.path()).contains(SECRET));
        run_all(&mut d);
        assert!(!d.render().unwrap().0.contains(SECRET));

        std::fs::remove_dir(blocked).unwrap();
        d.set_raw(false).unwrap();
        assert!(!read(dir.path()).contains(SECRET));
    }

    #[test]
    fn unreadable_existing_diagnostics_do_not_report_purge_success() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
        let mut d = Diag::open(dir.path(), true);
        assert!(d.set_raw(false).is_err());
        assert!(!d.raw);
    }

    #[test]
    fn purge_retries_an_initially_locked_file_without_losing_counts() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut original = Diag::open(dir.path(), true);
        original.bump("skill", "demo".into(), t(0), json!({}), Some(SECRET.into()));
        original.flush().unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let mut retry = Diag::open(dir.path(), false);
        retry.bump("skill", "demo".into(), t(1), json!({}), None);
        assert!(retry.set_raw(false).is_err());
        drop(locked);
        retry.set_raw(false).unwrap();
        let text = read(dir.path());
        assert!(!text.contains(SECRET));
        assert!(!text.contains("\"raw\""));
        let row: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(row["count"], 2);
        retry.set_raw(false).unwrap();
        assert_eq!(read(dir.path()), text, "재시도로 집계를 중복하지 않는다");
    }

    #[test]
    fn invalid_json_diagnostic_has_no_raw_even_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Diag::open(dir.path(), true);
        d.observe_unhandled(Product::CodexCli, &Unhandled::InvalidJson, t(0));
        d.flush().unwrap();
        let entry: Value = serde_json::from_str(read(dir.path()).trim()).unwrap();
        assert_eq!(entry["kind"], "invalid_json");
        assert!(entry.get("raw").is_none());
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
    fn write_failure_keeps_records_and_flush_waits_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        // 임시 파일 자리를 폴더로 막아 쓰기만 실패시킨다
        let block = dir.path().join("unhandled.jsonl.tmp");
        std::fs::create_dir(&block).unwrap();
        let mut d = Diag::open(dir.path(), false);
        let u = Unhandled::SystemSubtype {
            subtype: "s".into(),
        };
        d.observe_unhandled(Product::ClaudeCli, &u, t(0));
        assert!(d.flush_if_due(t(30)).is_ok(), "1분 전에는 쓰지 않는다");
        assert!(d.flush_if_due(t(60)).is_err());
        std::fs::remove_dir(&block).unwrap();
        d.flush_if_due(t(120)).unwrap();
        assert!(read(dir.path()).contains(r#""subtype":"s""#));
    }

    #[test]
    fn write_failure_does_not_lose_over_limit_entries() {
        let dir = tempfile::tempdir().unwrap();
        let block = dir.path().join("unhandled.jsonl.tmp");
        std::fs::create_dir(&block).unwrap();
        let u = |n: &str| Unhandled::ToolName {
            name: n.into(),
            input_keys: vec![],
        };
        let mut d = Diag::open_with_limit(dir.path(), false, 300);
        for (i, n) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            d.observe_unhandled(Product::ClaudeCli, &u(n), t(i as i64));
        }
        assert!(d.flush().is_err());
        std::fs::remove_dir(&block).unwrap();
        d.flush().unwrap();
        let text = read(dir.path());
        assert!(text.len() <= 300, "{}", text.len());
        assert!(text.contains(r#""name":"e""#), "{text}");
    }

    #[test]
    fn unreadable_existing_file_is_not_overwritten_and_memory_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        // 파일 자리가 폴더라 읽기가 NotFound 가 아닌 에러로 끝난다
        let existing = dir.path().join(FILE_NAME);
        std::fs::create_dir(&existing).unwrap();
        let mut d = Diag::open_with_limit(dir.path(), false, 300);
        for i in 0..20 {
            let u = Unhandled::ToolName {
                name: format!("n{i}"),
                input_keys: vec![],
            };
            d.observe_unhandled(Product::ClaudeCli, &u, t(i));
        }
        assert!(d.flush().is_err());
        assert!(existing.is_dir());
        assert!(d.entries.len() < 20, "상한 정리가 안 됨");
    }

    #[test]
    fn quoted_operators_do_not_leak_next_word() {
        for (cmd, key, want) in [
            (r#"echo "a | SecretWord b""#, "echo", "echo <arg>"),
            (
                r#"git commit -m "x ; leakedword""#,
                "git commit",
                "git commit -m <arg>",
            ),
            (r#"python -c "a && token""#, "python", "python -c <arg>"),
            ("echo 'a & Tail b' | wc", "echo", "echo <arg> | wc"),
        ] {
            assert_eq!(arg_shape(cmd, key), want, "{cmd}");
        }
    }

    #[test]
    fn long_flag_name_and_odd_key_words_are_masked() {
        assert_eq!(
            arg_shape("tool --sk-live-abcdefghijklmnopqrstuvwxyz", "tool"),
            "tool <arg>"
        );
        assert_eq!(arg_shape("tool --verbose", "tool"), "tool --verbose");
        assert_eq!(safe_key(r"cargo C:\Users\bob"), "cargo <arg>");
        assert_eq!(safe_key(""), "");
    }

    #[test]
    fn duplicate_lines_merge_and_old_raw_is_dropped_when_raw_off() {
        let dir = tempfile::tempdir().unwrap();
        let line = |count: u32, first: &str, last: &str| {
            format!(
                r#"{{"kind":"skill","key":"x","first_seen":"{first}","last_seen":"{last}","count":{count},"raw":"LEAK"}}"#
            )
        };
        let text = format!(
            "{}\n{}\n",
            line(2, "2026-01-02T00:00:00Z", "2026-01-03T00:00:00Z"),
            line(3, "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"),
        );
        std::fs::write(dir.path().join(FILE_NAME), text).unwrap();
        let mut d = Diag::open(dir.path(), false);
        d.bump("skill", "x".into(), t(0), json!({}), None);
        d.flush().unwrap();
        let out = read(dir.path());
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(!out.contains("LEAK"), "{out}");
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["count"], 6);
        assert_eq!(v["first_seen"], "2026-01-01T00:00:00Z");
        assert_ne!(v["last_seen"], "2026-01-03T00:00:00Z");
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
