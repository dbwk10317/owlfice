//! Claude Code 기록 파서 (PLAN 4.6.1)

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    Compound, Confidence, Event, EventKind, Parser, RawRef, TestResult, TodoItem, TodoStatus,
    ToolKind, ToolOutcome, ToolSummary, Unhandled, classify_command, mcp_kind, parse_test_result,
};
use crate::discover::Product;

/// 옛 버전은 `toolDenialKind` 없이 이 문구로만 반려를 알렸다
const REJECTED_TEXT: &str = "The user doesn't want to proceed with this tool use";
// ponytail: 팝업이 쓸 길이를 아직 모른다, T27에서 정한다
const PREVIEW_CHARS: usize = 200;

/// 파일 하나(메인 또는 서브 기록)의 파서. Reset 때 새로 만든다
pub struct ClaudeParser {
    depth: u8,
    parent_agent: Option<String>,
    /// `sessionId` 가 없는 레코드(드묾)는 직전 값을 쓴다
    session_id: String,
    model: Option<String>,
    meta: Option<Meta>,
    /// 결과를 아직 못 본 호출. 결과 해석에 호출 쪽 정보가 필요하다
    calls: HashMap<String, Call>,
    /// 내부 배관이라 버린 호출. 결과도 버린다
    dropped: HashSet<String>,
    /// TaskStop 호출 id → 멈출 작업 id
    stops: HashMap<String, String>,
    /// backgroundTaskId → 원래 호출 id. TaskStop 이 작업 id로만 가리킨다
    bg_tasks: HashMap<String, String>,
    /// 비동기로 뜬 서브 id. 완료 알림이 서브 것인지 백그라운드 명령 것인지 가른다
    agents: HashSet<String>,
    custom_title: bool,
    /// 서브 파일에서 agentId 없는 줄이 쓸 직전 값
    last_agent: Option<String>,
    /// 한 응답의 블록마다 같은 usage가 반복된다
    last_usage: Option<(Option<String>, u64, u64)>,
}

#[derive(PartialEq)]
struct Meta {
    cwd: String,
    model: Option<String>,
    product: Product,
    branch: Option<String>,
}

#[derive(Default)]
struct Call {
    name: String,
    test: bool,
    /// 단독 `&` 로 띄워 셸이 즉시 0을 돌려준 명령 (4.6.4)
    async_unknown: bool,
    /// `run_in_background: true` Bash·PowerShell. 서브 파일의 결과에는 backgroundTaskId 가 없다
    background: bool,
    spawn: Option<SpawnInput>,
}

struct SpawnInput {
    prompt: Option<String>,
    description: Option<String>,
    model: Option<String>,
}

struct Ctx<'a> {
    ts: Option<DateTime<Utc>>,
    session_id: String,
    agent_id: String,
    parent: Option<String>,
    cwd: Option<String>,
    raw: &'a RawRef,
}

impl Ctx<'_> {
    fn ev(&self, kind: EventKind) -> Event {
        Event {
            ts: self.ts,
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            parent_agent: self.parent.clone(),
            kind,
            raw_ref: self.raw.clone(),
        }
    }
}

impl ClaudeParser {
    /// `depth` 는 이 파일 주인의 깊이(메인 0, 서브는 meta.json `spawnDepth`), `parent_agent` 는 그 상사의 agent_id
    pub fn new(depth: u8, parent_agent: Option<String>) -> Self {
        Self {
            depth,
            parent_agent,
            session_id: String::new(),
            model: None,
            meta: None,
            calls: HashMap::new(),
            dropped: HashSet::new(),
            stops: HashMap::new(),
            bg_tasks: HashMap::new(),
            agents: HashSet::new(),
            custom_title: false,
            last_agent: None,
            last_usage: None,
        }
    }

    /// 메인 파일에서만 낸다. 값이 바뀔 때마다 다시 낸다 (모델 갱신)
    fn session_meta(
        &mut self,
        rec: &Value,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        let Some(cwd) = &ctx.cwd else { return };
        let entry = str_at(rec, "/entrypoint");
        let product = match entry {
            Some("claude-desktop") => Product::ClaudeDesktop,
            Some(_) => Product::ClaudeCli,
            None => self.meta.as_ref().map_or(Product::ClaudeCli, |m| m.product),
        };
        let meta = Meta {
            cwd: cwd.clone(),
            model: self.model.clone(),
            product,
            branch: str_at(rec, "/gitBranch")
                .map(str::to_string)
                .or_else(|| self.meta.as_ref().and_then(|m| m.branch.clone())),
        };
        if self.meta.as_ref() == Some(&meta) {
            return;
        }
        if let Some(v) = entry
            && !matches!(v, "claude-desktop" | "cli")
        {
            unhandled.push(Unhandled::ProductValue {
                value: v.to_string(),
            });
        }
        out.push(ctx.ev(EventKind::SessionMeta {
            cwd: PathBuf::from(&meta.cwd),
            model: meta.model.clone(),
            title: None,
            product: meta.product,
            git_branch: meta.branch.clone(),
        }));
        self.meta = Some(meta);
    }

    fn user(
        &mut self,
        rec: &Value,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        let origin = str_at(rec, "/origin/kind")
            .map(str::to_string)
            .or_else(|| str_at(rec, "/turnOrigin").map(|s| s.replace('_', "-")));
        // 다른 세션·에이전트가 보낸 메시지. 사장님 방문이 아니다
        if origin.as_deref() == Some("peer") {
            return;
        }
        match rec.pointer("/message/content") {
            Some(Value::String(s)) => {
                if origin.as_deref() == Some("task-notification")
                    || s.trim_start().starts_with("<task-notification>")
                {
                    self.notification(s, ctx, out, unhandled);
                } else if !flag(rec, "/isMeta")
                    && !flag(rec, "/isCompactSummary")
                    && !is_injected(s)
                {
                    out.push(ctx.ev(EventKind::UserMessage { text: s.clone() }));
                }
            }
            // text 블록만 있는 배열(거부 직후의 중단 표식 등)은 사람 입력이 아니다
            Some(Value::Array(blocks)) => {
                // image 블록이 최상위에 있으면 사람이 그림을 붙인 입력이다
                if !flag(rec, "/isMeta")
                    && blocks.iter().any(|b| str_at(b, "/type") == Some("image"))
                {
                    let text = blocks
                        .iter()
                        .filter(|b| str_at(b, "/type") == Some("text"))
                        .filter_map(|b| str_at(b, "/text"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    out.push(ctx.ev(EventKind::UserMessage { text }));
                }
                for b in blocks {
                    if str_at(b, "/type") == Some("tool_result") {
                        self.tool_result(b, rec, ctx, out, unhandled);
                    }
                }
            }
            _ => {}
        }
    }

    fn assistant(
        &mut self,
        rec: &Value,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        if let Some(Value::Array(blocks)) = rec.pointer("/message/content") {
            for b in blocks {
                match str_at(b, "/type") {
                    Some("text") => {
                        let s = str_at(b, "/text").unwrap_or_default();
                        if !s.trim().is_empty() {
                            out.push(ctx.ev(EventKind::AssistantText {
                                text: s.to_string(),
                            }));
                        }
                    }
                    Some("thinking") => out.push(ctx.ev(EventKind::Thinking)),
                    Some("tool_use") => self.tool_use(b, ctx, out, unhandled),
                    _ => {}
                }
            }
        }
        self.usage(rec, ctx, out);
    }

    fn usage(&mut self, rec: &Value, ctx: &Ctx, out: &mut Vec<Event>) {
        let Some(u) = rec.pointer("/message/usage") else {
            return;
        };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let (input, created, read, output) = (
            n("input_tokens"),
            n("cache_creation_input_tokens"),
            n("cache_read_input_tokens"),
            n("output_tokens"),
        );
        // 모두 0은 오류·중단 때 하네스가 넣는 합성 레코드다. 사용량이 아니다
        if [input, created, read, output].iter().all(|&x| x == 0) {
            return;
        }
        let input_total = input.saturating_add(created).saturating_add(read);
        let key = (
            str_at(rec, "/message/id").map(str::to_string),
            input_total,
            output,
        );
        if self.last_usage.as_ref() == Some(&key) {
            return;
        }
        self.last_usage = Some(key);
        out.push(ctx.ev(EventKind::TokenUsage {
            input_total,
            cache_read: read,
            output,
            context_window: None,
        }));
    }

    fn tool_use(
        &mut self,
        b: &Value,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        let (Some(id), Some(name)) = (str_at(b, "/id"), str_at(b, "/name")) else {
            return;
        };
        let input = b.get("input").unwrap_or(&Value::Null);
        let s = |k: &str| input.get(k).and_then(Value::as_str);
        let rel = |p: Option<&str>| p.map(|p| rel_path(p, ctx.cwd.as_deref()));
        let mut summary = ToolSummary {
            path: None,
            range: None,
            pattern: None,
            command: None,
            command_key: None,
            command_norm: None,
            compound: Compound::Single,
            background: false,
            lines_changed: None,
            escalated: false,
            commit_message: None,
        };
        let mut call = Call {
            name: name.to_string(),
            ..Call::default()
        };
        let mut extra = None;
        // 순서대로 첫 매치 (4.6.1 매핑표)
        let kind = match name {
            "TodoWrite" | "mcp__ccd_turn__update_plan" => {
                extra = Some(EventKind::TodoChanged {
                    items: todo_items(input),
                });
                ToolKind::Todo
            }
            "AskUserQuestion" | "ExitPlanMode" => {
                let question = input
                    .pointer("/questions/0/question")
                    .and_then(Value::as_str)
                    .or_else(|| s("question"))
                    .or_else(|| s("plan"))
                    .unwrap_or_default();
                extra = Some(EventKind::PermissionRequest {
                    question: question.to_string(),
                    plan: name == "ExitPlanMode",
                });
                ToolKind::Ask
            }
            "Read" => {
                summary.path = rel(s("file_path"));
                let num = |k: &str| {
                    input
                        .get(k)
                        .and_then(Value::as_u64)
                        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
                };
                // ponytail: offset 만 있으면 끝 줄을 몰라 범위 없음으로 둔다, 문장에 필요해지면 올린다
                if let Some(limit) = num("limit") {
                    let start = num("offset").unwrap_or(1);
                    summary.range = Some((start, start.saturating_add(limit.saturating_sub(1))));
                }
                ToolKind::FileRead
            }
            "Grep" | "Glob" => {
                summary.pattern = s("pattern").map(str::to_string);
                summary.path = rel(s("path"));
                ToolKind::FileSearch
            }
            "Write" => {
                summary.path = rel(s("file_path"));
                ToolKind::FileWrite
            }
            "Edit" | "NotebookEdit" => {
                summary.path = rel(s("file_path").or_else(|| s("notebook_path")));
                let lines = ["old_string", "new_string", "new_source"]
                    .iter()
                    .filter_map(|k| s(k))
                    .map(|t| if t.is_empty() { 0 } else { t.lines().count() })
                    .max();
                summary.lines_changed = lines.map(|n| u32::try_from(n).unwrap_or(u32::MAX));
                ToolKind::FileEdit
            }
            "Bash" | "PowerShell" => {
                summary.background =
                    input.get("run_in_background").and_then(Value::as_bool) == Some(true);
                match s("command") {
                    Some(cmd) => {
                        let c = classify_command(cmd);
                        summary.path = rel(c.path.as_deref());
                        summary.command = Some(c.command);
                        summary.command_key = Some(c.command_key);
                        summary.command_norm = Some(c.command_norm);
                        summary.commit_message = c.commit_message;
                        summary.compound = c.compound;
                        call.test = c.kind == ToolKind::Test;
                        call.async_unknown = c.async_unknown;
                        call.background = summary.background;
                        c.kind
                    }
                    None => ToolKind::Shell,
                }
            }
            "Agent" | "Task" => {
                call.spawn = Some(SpawnInput {
                    prompt: s("prompt").map(str::to_string),
                    description: s("description").map(str::to_string),
                    model: s("model").map(str::to_string),
                });
                ToolKind::Spawn
            }
            "Skill" => ToolKind::Skill {
                name: s("skill").unwrap_or_default().to_string(),
            },
            "WebSearch" => {
                summary.pattern = s("query").map(str::to_string);
                ToolKind::WebSearch
            }
            "WebFetch" => {
                summary.pattern = s("url").map(domain);
                ToolKind::WebFetch
            }
            // 호출은 작업 id와 연결만 한다. 결과가 Ok일 때 그 작업의 호출을 닫는다
            "TaskStop" => {
                match s("task_id") {
                    Some(task) => {
                        self.stops.insert(id.to_string(), task.to_string());
                    }
                    None => {
                        self.dropped.insert(id.to_string());
                    }
                }
                return;
            }
            n if is_internal(n) => {
                self.dropped.insert(id.to_string());
                return;
            }
            n if n.starts_with("mcp__") => {
                let kind = mcp_kind(n);
                match kind {
                    ToolKind::WebSearch => summary.pattern = s("query").map(str::to_string),
                    ToolKind::WebFetch => summary.pattern = s("url").map(domain),
                    _ => {}
                }
                kind
            }
            _ => {
                unhandled.push(Unhandled::ToolName {
                    name: name.to_string(),
                    input_keys: keys(input),
                });
                ToolKind::Other(name.to_string())
            }
        };
        self.calls.insert(id.to_string(), call);
        out.push(ctx.ev(EventKind::ToolCall {
            call_id: id.to_string(),
            tool: kind,
            summary,
        }));
        if let Some(k) = extra {
            out.push(ctx.ev(k));
        }
    }

    fn tool_result(
        &mut self,
        b: &Value,
        rec: &Value,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        let Some(call_id) = str_at(b, "/tool_use_id") else {
            return;
        };
        // 내부 도구의 실패가 사고로 세지지 않게 결과도 버린다
        if self.dropped.remove(call_id) {
            return;
        }
        let text = content_text(b.get("content"));
        let is_error = b.get("is_error").and_then(Value::as_bool) == Some(true);
        let rejected = rec.get("toolDenialKind").is_some_and(|v| !v.is_null())
            || (is_error && text.starts_with(REJECTED_TEXT));
        if let Some(task) = self.stops.remove(call_id) {
            // 중단이 실패하면 작업은 계속 돈다. 작업표를 유지한다
            if !rejected
                && !is_error
                && let Some(bg) = self.bg_tasks.remove(&task)
            {
                self.calls.remove(&bg);
                out.push(ctx.ev(result(bg, ToolOutcome::Unknown, String::new(), None)));
            }
            return;
        }
        let call = self.calls.remove(call_id).unwrap_or_default();
        let tur = rec.get("toolUseResult").unwrap_or(&Value::Null);
        let bg_task = str_at(tur, "/backgroundTaskId");
        let outcome = if rejected {
            ToolOutcome::Rejected
        } else if is_error {
            ToolOutcome::Err
        } else if bg_task.is_some() || call.background {
            if let Some(task) = bg_task {
                self.bg_tasks.insert(task.to_string(), call_id.to_string());
            }
            self.calls.insert(
                call_id.to_string(),
                Call {
                    name: call.name.clone(),
                    ..Call::default()
                },
            );
            ToolOutcome::Started
        } else if call.async_unknown {
            unhandled.push(Unhandled::OutcomeUnknown {
                tool: call.name.clone(),
                result_keys: keys(tur),
            });
            ToolOutcome::Unknown
        } else {
            ToolOutcome::Ok
        };
        let test_result = (call.test && matches!(outcome, ToolOutcome::Ok | ToolOutcome::Err))
            .then(|| parse_test_result(&text))
            .flatten();
        out.push(ctx.ev(result(
            call_id.to_string(),
            outcome,
            preview(&text),
            test_result,
        )));
        if outcome != ToolOutcome::Ok {
            return;
        }
        let (Some(child), Some(status)) = (str_at(tur, "/agentId"), str_at(tur, "/status")) else {
            return;
        };
        let async_launched = status == "async_launched";
        if !async_launched && status != "completed" {
            return;
        }
        let input = call.spawn.as_ref();
        let pick = |key: &str, from_input: Option<&String>| {
            str_at(tur, key)
                .map(str::to_string)
                .or_else(|| from_input.cloned())
        };
        out.push(ctx.ev(EventKind::SubagentSpawn {
            child_id: child.to_string(),
            prompt: pick("/prompt", input.and_then(|i| i.prompt.as_ref())).unwrap_or_default(),
            // 동기 결과에는 resolvedModel이 없어 호출의 별칭으로 둔다 (4.7.4가 별칭을 받는다)
            model: pick("/resolvedModel", input.and_then(|i| i.model.as_ref())),
            description: pick("/description", input.and_then(|i| i.description.as_ref())),
            effort: None,
            depth: self.depth.saturating_add(1),
        }));
        if async_launched {
            self.agents.insert(child.to_string());
        } else {
            // 동기 실행은 결과가 곧 완료다
            out.push(ctx.ev(EventKind::SubagentDone {
                child_id: child.to_string(),
            }));
        }
    }

    /// `<task-notification>`: 비동기 서브의 완료 또는 백그라운드 명령의 종료
    fn notification(
        &mut self,
        s: &str,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        let task = tag(s, "task-id");
        // 띄운 걸 못 본 중첩 서브의 알림은 `<usage>` 로 서브임을 안다
        if let Some(t) = task
            && (self.agents.contains(t) || tag(s, "usage").is_some())
        {
            out.push(ctx.ev(EventKind::SubagentDone {
                child_id: t.to_string(),
            }));
            return;
        }
        let Some(call_id) = tag(s, "tool-use-id") else {
            return;
        };
        if let Some(t) = task {
            self.bg_tasks.remove(t);
        }
        let call = self.calls.remove(call_id).unwrap_or_default();
        let summary = tag(s, "summary").unwrap_or_default();
        // 종료 코드는 하네스가 쓰는 summary 문구 안에만 있다
        let outcome = match exit_code(summary) {
            Some(0) => ToolOutcome::Ok,
            Some(_) => ToolOutcome::Err,
            None => {
                unhandled.push(Unhandled::OutcomeUnknown {
                    tool: call.name,
                    result_keys: tag_names(s),
                });
                ToolOutcome::Unknown
            }
        };
        out.push(ctx.ev(result(call_id.to_string(), outcome, preview(summary), None)));
    }

    fn attachment(
        &mut self,
        rec: &Value,
        ctx: &Ctx,
        out: &mut Vec<Event>,
        unhandled: &mut Vec<Unhandled>,
    ) {
        if str_at(rec, "/attachment/type") != Some("queued_command") {
            return;
        }
        // `commandMode` 는 믿지 않고 앞머리로 가른다
        let Some(prompt) = str_at(rec, "/attachment/prompt") else {
            return;
        };
        let t = prompt.trim_start();
        if t.starts_with("<task-notification>") {
            self.notification(prompt, ctx, out, unhandled);
        } else if !t.is_empty() && !t.starts_with('<') {
            // 턴 중에 사장님이 친 입력
            out.push(ctx.ev(EventKind::UserMessage {
                text: prompt.to_string(),
            }));
        }
    }

    fn title(&mut self, rec: &Value, key: &str, custom: bool, ctx: &Ctx, out: &mut Vec<Event>) {
        if !custom && self.custom_title {
            return;
        }
        if let Some(t) = str_at(rec, key) {
            self.custom_title |= custom;
            out.push(ctx.ev(EventKind::TitleChanged {
                title: t.to_string(),
            }));
        }
    }
}

impl Parser for ClaudeParser {
    fn parse_line(
        &mut self,
        bytes: &[u8],
        raw_ref: &RawRef,
        unhandled: &mut Vec<Unhandled>,
    ) -> Vec<Event> {
        let Ok(rec) = serde_json::from_slice::<Value>(bytes) else {
            return Vec::new();
        };
        if !rec.is_object() {
            return Vec::new();
        }
        if let Some(s) = str_at(&rec, "/sessionId") {
            self.session_id = s.to_string();
        }
        let sub = str_at(&rec, "/agentId");
        if let (Some(a), Some(_)) = (sub, &self.parent_agent) {
            self.last_agent = Some(a.to_string());
        }
        let ctx = Ctx {
            ts: str_at(&rec, "/timestamp").and_then(parse_ts),
            session_id: self.session_id.clone(),
            agent_id: sub
                .map(str::to_string)
                .or_else(|| self.last_agent.clone())
                .unwrap_or_else(|| self.session_id.clone()),
            parent: self.parent_agent.clone(),
            cwd: str_at(&rec, "/cwd").map(str::to_string),
            raw: raw_ref,
        };
        let typ = str_at(&rec, "/type").unwrap_or_default();
        if typ == "assistant"
            && let Some(m) = str_at(&rec, "/message/model")
            && m != "<synthetic>"
        {
            self.model = Some(m.to_string());
        }
        let mut out = Vec::new();
        if sub.is_none() && self.parent_agent.is_none() {
            self.session_meta(&rec, &ctx, &mut out, unhandled);
        }
        match typ {
            "user" => self.user(&rec, &ctx, &mut out, unhandled),
            "assistant" => self.assistant(&rec, &ctx, &mut out, unhandled),
            "attachment" => self.attachment(&rec, &ctx, &mut out, unhandled),
            "system" => match str_at(&rec, "/subtype") {
                Some("compact_boundary") => {
                    let n = |k: &str| {
                        rec.pointer("/compactMetadata")
                            .and_then(|m| m.get(k))
                            .and_then(Value::as_u64)
                    };
                    out.push(ctx.ev(EventKind::ContextCompaction {
                        pre_tokens: n("preTokens"),
                        post_tokens: n("postTokens"),
                    }));
                }
                Some("stop_hook_summary" | "turn_duration" | "away_summary" | "informational")
                | None => {}
                Some(other) => unhandled.push(Unhandled::SystemSubtype {
                    subtype: other.to_string(),
                }),
            },
            "custom-title" => self.title(&rec, "/customTitle", true, &ctx, &mut out),
            "agent-name" => self.title(&rec, "/agentName", false, &ctx, &mut out),
            // ponytail: ai-title 의 키는 표본이 없어 짐작이다, CLI 표본이 생기면 확인한다
            "ai-title" => self.title(&rec, "/aiTitle", false, &ctx, &mut out),
            "pr-link" => {
                let number = rec
                    .get("prNumber")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok());
                if let (Some(number), Some(url)) = (number, str_at(&rec, "/prUrl")) {
                    out.push(ctx.ev(EventKind::PrLinked {
                        number,
                        url: url.to_string(),
                    }));
                }
            }
            t if is_ignored_type(t) => {}
            t => {
                let renamed_turn = rec.get("timestamp").is_some()
                    && rec.pointer("/message/role").is_some()
                    && rec.pointer("/message/content").is_some();
                unhandled.push(Unhandled::RecordType {
                    type_name: t.to_string(),
                    keys: keys(&rec),
                    renamed_turn,
                });
            }
        }
        out
    }
}

fn result(
    call_id: String,
    outcome: ToolOutcome,
    preview: String,
    test_result: Option<TestResult>,
) -> EventKind {
    EventKind::ToolResult {
        call_id,
        outcome,
        outcome_confidence: Confidence::Confirmed,
        preview,
        test_result,
    }
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

fn flag(v: &Value, ptr: &str) -> bool {
    v.pointer(ptr).and_then(Value::as_bool) == Some(true)
}

fn keys(v: &Value) -> Vec<String> {
    v.as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn is_ignored_type(t: &str) -> bool {
    matches!(
        t,
        "queue-operation"
            | "file-history-snapshot"
            | "file-history-delta"
            | "last-prompt"
            | "mode"
            | "permission-mode"
            | "atis-latch"
            | "cost-state"
            | "bridge-session"
            | "frame-link"
            | "artifact-comment-monitor"
            | "artifact-autoreact-ledger"
            | "summary"
            | "progress"
    ) || t.starts_with("api-request")
}

/// 앱 내부 배관. pending·실적·미대응 로그 어디에도 넣지 않는다
fn is_internal(name: &str) -> bool {
    [
        "mcp__ccd_",
        "mcp__terminal__",
        "mcp__Claude_Browser__",
        "mcp__claude-in-chrome__",
        "Artifact",
    ]
    .iter()
    .any(|p| name.starts_with(p))
        || matches!(
            name,
            "ToolSearch" | "Monitor" | "SendMessage" | "ScheduleWakeup" | "Workflow"
        )
}

/// 문자열 user 레코드 중 사람이 친 것이 아닌 것 (시스템 주입, 슬래시 명령·`!` 셸 흔적)
fn is_injected(s: &str) -> bool {
    let t = s.trim_start();
    [
        "<artifact-view-context>",
        "<command-name>",
        "<command-message>",
        "<local-command-stdout>",
        "<local-command-caveat>",
        "<bash-input>",
        "<bash-stdout>",
        "<bash-stderr>",
        "[Request interrupted by user",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

fn todo_items(input: &Value) -> Vec<TodoItem> {
    let (list, text_key) = match (input.get("todos"), input.get("plan")) {
        (Some(Value::Array(a)), _) => (a, "content"),
        (_, Some(Value::Array(a))) => (a, "step"),
        _ => return Vec::new(),
    };
    list.iter()
        .map(|t| TodoItem {
            content: str_at(t, &format!("/{text_key}"))
                .unwrap_or_default()
                .to_string(),
            status: match str_at(t, "/status") {
                Some("in_progress") => TodoStatus::InProgress,
                Some("completed") => TodoStatus::Done,
                _ => TodoStatus::Pending,
            },
        })
        .collect()
}

/// `tool_result.content` 는 문자열 또는 블록 배열
pub(crate) fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| str_at(b, "/text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn preview(s: &str) -> String {
    s.chars().take(PREVIEW_CHARS).collect()
}

/// cwd 아래면 상대 경로로. Windows 경로라 구분자와 대소문자를 가리지 않는다
fn rel_path(path: &str, cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.map(|c| c.trim_end_matches(['/', '\\'])) else {
        return path.to_string();
    };
    let norm = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    match path.get(..cwd.len()) {
        Some(head)
            if !cwd.is_empty()
                && norm(head) == norm(cwd)
                && matches!(path.as_bytes().get(cwd.len()), Some(b'/' | b'\\')) =>
        {
            path[cwd.len() + 1..].to_string()
        }
        _ => path.to_string(),
    }
}

fn domain(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = s.find(&open)? + open.len();
    let end = start + s[start..].find(&format!("</{name}>"))?;
    Some(s[start..end].trim())
}

/// 알림에서 하네스가 쓰는 태그. 이 밖의 낱말은 원문 조각일 수 있어 로그에 안 남긴다
const KNOWN_TAGS: [&str; 8] = [
    "task-id",
    "tool-use-id",
    "output-file",
    "status",
    "summary",
    "note",
    "result",
    "usage",
];

/// 알림 안의 태그 이름. 미대응 로그에 원문 대신 남기는 구조 정보
fn tag_names(s: &str) -> Vec<String> {
    s.split('<')
        .skip(1)
        .filter_map(|p| {
            let name = p.split(['>', ' ']).next()?;
            KNOWN_TAGS.contains(&name).then(|| name.to_string())
        })
        .collect()
}

/// summary 끝의 `(exit code 0)`, `failed with exit code 2`. 끝에 오는 것만 받아 따옴표 안 설명문의 말은 거른다
fn exit_code(summary: &str) -> Option<i64> {
    const KEY: &str = "exit code ";
    let s = summary.trim_end_matches(|c: char| c.is_whitespace() || c == '.');
    let at = s.rfind(KEY)?;
    let (head, rest) = (&s[..at], &s[at + KEY.len()..]);
    // 앞말은 따지지 않는다: 표본은 익명화로 "with" 가 다른 낱말이 되어 있다
    let n = match rest.strip_suffix(')') {
        Some(n) if head.ends_with('(') => n,
        None => rest,
        _ => return None,
    };
    n.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    struct Run {
        events: Vec<Event>,
        unhandled: Vec<Unhandled>,
    }

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/claude")
            .join(rel)
    }

    /// TailReader처럼 `\n` 으로 자르고 `\r` 을 뗀 줄을 넣는다
    fn run_bytes(bytes: &[u8], path: &Path, p: &mut ClaudeParser) -> Run {
        let (mut events, mut unhandled, mut offset) = (Vec::new(), Vec::new(), 0u64);
        for (i, line) in bytes.split(|&b| b == b'\n').enumerate() {
            let raw = RawRef {
                path: path.to_path_buf(),
                generation: 0,
                byte_offset: offset,
                line_no: i as u64 + 1,
            };
            offset += line.len() as u64 + 1;
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            events.extend(p.parse_line(line, &raw, &mut unhandled));
        }
        Run { events, unhandled }
    }

    fn run_file(path: &Path, p: &mut ClaudeParser) -> Run {
        run_bytes(&std::fs::read(path).unwrap(), path, p)
    }

    fn main_file(rel: &str) -> Run {
        run_file(&fixture(rel), &mut ClaudeParser::new(0, None))
    }

    fn feed<S: AsRef<str>>(lines: &[S]) -> Run {
        let text = lines
            .iter()
            .map(|l| l.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        run_bytes(
            text.as_bytes(),
            Path::new("synthetic.jsonl"),
            &mut ClaudeParser::new(0, None),
        )
    }

    fn at(r: &Run, line: u64) -> Vec<EventKind> {
        r.events
            .iter()
            .filter(|e| e.raw_ref.line_no == line)
            .map(|e| e.kind.clone())
            .collect()
    }

    fn results(r: &Run) -> Vec<(String, ToolOutcome)> {
        r.events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult {
                    call_id,
                    outcome,
                    outcome_confidence,
                    ..
                } => {
                    assert_eq!(*outcome_confidence, Confidence::Confirmed);
                    Some((call_id.clone(), *outcome))
                }
                _ => None,
            })
            .collect()
    }

    fn user_lines(r: &Run) -> Vec<u64> {
        r.events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::UserMessage { .. }))
            .map(|e| e.raw_ref.line_no)
            .collect()
    }

    fn call_at(r: &Run, line: u64) -> (String, ToolKind, ToolSummary) {
        at(r, line)
            .into_iter()
            .find_map(|k| match k {
                EventKind::ToolCall {
                    call_id,
                    tool,
                    summary,
                } => Some((call_id, tool, summary)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{line}행에 ToolCall 없음"))
    }

    fn is_result(k: &EventKind, id: &str, outcome: ToolOutcome, test: Option<(u32, u32)>) -> bool {
        let want = test.map(|(passed, failed)| TestResult { passed, failed });
        matches!(k, EventKind::ToolResult {
            call_id,
            outcome: o,
            outcome_confidence: Confidence::Confirmed,
            test_result,
            ..
        } if call_id == id && *o == outcome && *test_result == want)
    }

    fn tool_use(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    /// `top` 은 레코드 최상위에 더할 필드(끝에 쉼표), `block` 은 tool_result 블록에 더할 필드(앞에 쉼표)
    fn tool_result(id: &str, block: &str, top: &str) -> String {
        format!(
            r#"{{"type":"user",{top}"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}"{block}}}]}}}}"#
        )
    }

    fn notification(task: &str, call: &str, summary: &str) -> String {
        format!(
            r#"{{"type":"user","origin":{{"kind":"task-notification"}},"message":{{"role":"user","content":"<task-notification>\n<task-id>{task}</task-id>\n<tool-use-id>{call}</tool-use-id>\n<status>completed</status>\n<summary>{summary}</summary>\n</task-notification>"}}}}"#
        )
    }

    #[test]
    fn every_fixture_line_parses_and_every_type_is_known() {
        let mut dirs = vec![fixture("")];
        let mut files = 0;
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "jsonl") {
                    files += 1;
                    let r = run_file(&p, &mut ClaudeParser::new(0, None));
                    let unknown: Vec<_> = r
                        .unhandled
                        .iter()
                        .filter(|u| {
                            matches!(
                                u,
                                Unhandled::RecordType { .. } | Unhandled::SystemSubtype { .. }
                            )
                        })
                        .collect();
                    assert!(unknown.is_empty(), "{p:?}: {unknown:?}");
                }
            }
        }
        // 순회가 비어 통과하는 걸 막는다
        assert!(files > 0);
    }

    #[test]
    fn async_subagent_parent_file() {
        let r = main_file("async-subagent/302222d1-a790-f2ee-6a9b-2212d4de135d.jsonl");
        let sid = "302222d1-a790-f2ee-6a9b-2212d4de135d";
        assert!(
            r.events
                .iter()
                .all(|e| e.session_id == sid && e.agent_id == sid && e.parent_agent.is_none())
        );
        assert_eq!(
            at(&r, 1),
            vec![
                EventKind::SessionMeta {
                    cwd: PathBuf::from("C:\\dir1\\dir2"),
                    model: Some("claude-opus-5-5".into()),
                    title: None,
                    product: Product::ClaudeDesktop,
                    git_branch: Some("jwnqs/treksu-lppol-39585740".into()),
                },
                EventKind::Thinking,
                EventKind::TokenUsage {
                    input_total: 2 + 868 + 476_287,
                    cache_read: 476_287,
                    output: 1751,
                    context_window: None,
                },
            ]
        );
        // 같은 응답의 다음 블록은 usage를 다시 내지 않는다
        assert!(matches!(at(&r, 2)[..], [EventKind::AssistantText { .. }]));

        let (id, tool, _) = call_at(&r, 3);
        assert_eq!(
            (id.as_str(), tool),
            ("toolu_803iLgcqY6KulyMzeXEBzOwh", ToolKind::Spawn)
        );
        let line4 = at(&r, 4);
        assert_eq!(line4.len(), 2);
        assert!(is_result(&line4[0], &id, ToolOutcome::Ok, None));
        match &line4[1] {
            EventKind::SubagentSpawn {
                child_id,
                prompt,
                model,
                description,
                effort,
                depth,
            } => {
                assert_eq!(child_id, "a4fa9a3e07b34f715");
                assert!(prompt.starts_with("Npy ska pl"));
                assert_eq!(model.as_deref(), Some("claude-opus-5-5"));
                assert_eq!(
                    description.as_deref(),
                    Some("Tqj xzzdv RQZ tvptrdxbxf-nsceqzm kroh")
                );
                assert_eq!(*effort, None);
                assert_eq!(*depth, 1);
            }
            k => panic!("{k:?}"),
        }

        // 백그라운드 Bash: 시작 응답 → 완료 알림의 exit code 0
        let (id, _, summary) = call_at(&r, 8);
        assert_eq!(id, "toolu_526yXRSUGhPGXNDdoBPqHTCn");
        assert!(summary.background);
        assert!(is_result(&at(&r, 9)[0], &id, ToolOutcome::Started, None));
        let line25 = at(&r, 25);
        assert_eq!(line25.len(), 1);
        assert!(is_result(&line25[0], &id, ToolOutcome::Ok, None));

        // 파일 밖 경로는 절대 경로 그대로
        let (_, tool, summary) = call_at(&r, 18);
        assert_eq!(tool, ToolKind::FileRead);
        assert!(summary.path.unwrap().starts_with("C:\\dir3\\"));

        // custom-title 은 시각이 없고, 뒤따른 agent-name 은 내지 않는다
        let title: Vec<_> = r
            .events
            .iter()
            .filter(|e| e.raw_ref.line_no == 12)
            .collect();
        assert!(matches!(
            title[..],
            [Event {
                ts: None,
                kind: EventKind::TitleChanged { .. },
                ..
            }]
        ));
        assert!(at(&r, 13).is_empty());

        assert!(user_lines(&r).is_empty());
        // 이 서브는 사장님이 멈췄다 (meta.json stoppedByUser). 완료 알림이 없다
        assert!(
            !r.events
                .iter()
                .any(|e| matches!(e.kind, EventKind::SubagentDone { .. }))
        );
        assert!(r.unhandled.is_empty(), "{:?}", r.unhandled);
    }

    #[test]
    fn async_subagent_child_file() {
        let sid = "302222d1-a790-f2ee-6a9b-2212d4de135d";
        let r = run_file(
            &fixture(&format!(
                "async-subagent/{sid}/subagents/agent-a4fa9a3e07b34f715.jsonl"
            )),
            &mut ClaudeParser::new(1, Some(sid.into())),
        );
        assert!(r.events.iter().all(|e| e.session_id == sid
            && e.agent_id == "a4fa9a3e07b34f715"
            && e.parent_agent.as_deref() == Some(sid)));
        assert!(
            !r.events
                .iter()
                .any(|e| matches!(e.kind, EventKind::SessionMeta { .. }))
        );
        // 첫 레코드는 프롬프트 전문. isMeta 주입(3행)은 아니다
        assert_eq!(user_lines(&r), vec![1]);
        assert!(at(&r, 3).is_empty());
        let err = "toolu_92CmpPF6BPm7n0BUabUPiaJl";
        assert!(is_result(&at(&r, 37)[0], err, ToolOutcome::Err, None));
        let rej = "toolu_818LWOeNnoOamXVqS26fMhcz";
        assert!(is_result(&at(&r, 75)[0], rej, ToolOutcome::Rejected, None));
        assert!(at(&r, 76).is_empty());
        // 서브 파일의 백그라운드 결과에는 toolUseResult 가 없다
        let bg = "toolu_97K7pFRvh9lEbVPj0LZZgGtY";
        assert!(is_result(&at(&r, 57)[0], bg, ToolOutcome::Started, None));
    }

    #[test]
    fn sync_subagent_spawns_and_finishes_at_once() {
        let r = main_file("sync-subagent/ed46b23c-eb2a-4459-1e37-cb69d8ecad93.jsonl");
        let line4 = at(&r, 4);
        assert_eq!(line4.len(), 3);
        let id = "toolu_803iLgcqY6KulyMzeXEBzOwh";
        assert!(is_result(&line4[0], id, ToolOutcome::Ok, None));
        match &line4[1] {
            EventKind::SubagentSpawn {
                child_id,
                model,
                description,
                depth,
                ..
            } => {
                assert_eq!(child_id, "ac0c27afe76bccd54");
                // 동기 결과에는 resolvedModel이 없다. 호출의 별칭
                assert_eq!(model.as_deref(), Some("opus"));
                assert_eq!(
                    description.as_deref(),
                    Some("Tqj xzzdv RQZ tvptrdxbxf-nsceqzm kroh")
                );
                assert_eq!(*depth, 1);
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(
            line4[2],
            EventKind::SubagentDone {
                child_id: "ac0c27afe76bccd54".into()
            }
        );
    }

    #[test]
    fn test_fail_fix_pass() {
        let r = main_file("test-fail-fix-pass/015373c5-6b19-d6c2-c7e1-86385bee9237.jsonl");
        assert_eq!(user_lines(&r), vec![1]);
        let (id, tool, s) = call_at(&r, 3);
        assert_eq!(tool, ToolKind::Test);
        assert_eq!(s.command_key.as_deref(), Some("cargo test"));
        assert_eq!(s.compound, Compound::Single);
        assert!(is_result(
            &at(&r, 4)[0],
            &id,
            ToolOutcome::Err,
            Some((3, 1))
        ));

        let (id, tool, s) = call_at(&r, 6);
        assert_eq!(tool, ToolKind::FileEdit);
        assert_eq!(s.path.as_deref(), Some("file3.rs"));
        assert_eq!(s.lines_changed, Some(1));
        assert!(is_result(&at(&r, 7)[0], &id, ToolOutcome::Ok, None));

        let (id, tool, _) = call_at(&r, 8);
        assert_eq!(tool, ToolKind::Test);
        assert!(is_result(&at(&r, 9)[0], &id, ToolOutcome::Ok, Some((4, 0))));
    }

    #[test]
    fn parallel_calls_with_reversed_results() {
        let r = main_file("parallel-reversed/bb5d9bb5-fb0b-cbf9-a342-7a82e4d82553.jsonl");
        let (a, tool_a, sa) = call_at(&r, 9);
        let (b, tool_b, sb) = call_at(&r, 10);
        assert_eq!((tool_a, tool_b), (ToolKind::FileRead, ToolKind::FileRead));
        assert_eq!(sa.path.as_deref(), Some("dir3/file4.md"));
        assert_eq!(sb.path.as_deref(), Some("dir3/file5.md"));
        assert_eq!(
            results(&r),
            vec![(b, ToolOutcome::Ok), (a, ToolOutcome::Ok)]
        );
        assert_eq!(user_lines(&r), vec![3]);
    }

    #[test]
    fn compaction_and_its_injections() {
        let r = main_file("compaction/2d416c44-e80b-dfec-6627-ef150260a525.jsonl");
        assert_eq!(
            at(&r, 16),
            vec![EventKind::ContextCompaction {
                pre_tokens: Some(966_723),
                post_tokens: Some(13_591),
            }]
        );
        // 요약(20행)과 queued_command·attachment 재주입은 사람 입력이 아니다
        assert!(user_lines(&r).is_empty());
        assert!(at(&r, 20).is_empty());
    }

    #[test]
    fn rejected_tool_and_interrupt_marker() {
        let r = main_file("reject/13f4d27c-e1d7-fa23-0b0e-05ad838d1b68.jsonl");
        let (id, _, _) = call_at(&r, 2);
        assert_eq!(results(&r), vec![(id, ToolOutcome::Rejected)]);
        assert!(at(&r, 9).is_empty());
        assert_eq!(user_lines(&r), vec![11]);
    }

    #[test]
    fn internal_tool_calls_and_failures_are_silent() {
        let r = main_file("internal-tool-failure/e52d1abf-7516-aeb5-e401-2f94e956374d.jsonl");
        assert!(!r.events.iter().any(|e| matches!(
            e.kind,
            EventKind::ToolCall { .. } | EventKind::ToolResult { .. }
        )));
        assert!(r.unhandled.is_empty());
    }

    #[test]
    fn failed_background_rejection_and_manual_compact() {
        let r = main_file("bg-fail-reject-compact/dcc68dd7-8ab5-ede5-fb28-c09de28115be.jsonl");
        let (bg, _, s) = call_at(&r, 23);
        assert!(s.background);
        let (rej, tool, _) = call_at(&r, 45);
        assert_eq!(tool, ToolKind::Shell);
        assert_eq!(
            results(&r),
            vec![
                (bg.clone(), ToolOutcome::Started),
                (bg.clone(), ToolOutcome::Err),
                (rej, ToolOutcome::Rejected),
            ]
        );
        assert!(is_result(&at(&r, 42)[0], &bg, ToolOutcome::Err, None));
        assert_eq!(
            at(&r, 54),
            vec![EventKind::ContextCompaction {
                pre_tokens: Some(90_941),
                post_tokens: Some(11_963),
            }]
        );
        // 55~58행: 압축 요약, caveat(isMeta), <command-name>, <local-command-stdout>
        assert_eq!(user_lines(&r), vec![3, 70]);
    }

    #[test]
    fn product_from_entrypoint() {
        let rec = |e: &str| {
            format!(
                r#"{{"type":"user","cwd":"C:\\w","entrypoint":"{e}","sessionId":"00000000-0000-4000-8000-000000000001","message":{{"role":"user","content":"hi"}}}}"#
            )
        };
        let product = |r: &Run| {
            r.events.iter().find_map(|e| match e.kind {
                EventKind::SessionMeta { product, .. } => Some(product),
                _ => None,
            })
        };
        let r = feed(&[rec("cli")]);
        assert_eq!(product(&r), Some(Product::ClaudeCli));
        assert!(r.unhandled.is_empty());
        let r = feed(&[rec("sdk-cli")]);
        assert_eq!(product(&r), Some(Product::ClaudeCli));
        assert_eq!(
            r.unhandled,
            vec![Unhandled::ProductValue {
                value: "sdk-cli".into()
            }]
        );
    }

    #[test]
    fn unknown_record_type_and_system_subtype() {
        let r = feed(&[
            r#"{"type":"brand-new","x":1}"#,
            r#"{"type":"turn-v2","timestamp":"2026-03-02T09:00:00Z","message":{"role":"user","content":"x"}}"#,
            r#"{"type":"system","subtype":"brand_new"}"#,
            r#"{"type":"system","subtype":"turn_duration"}"#,
        ]);
        assert!(r.events.is_empty());
        let mut got = r.unhandled.clone();
        for u in &mut got {
            if let Unhandled::RecordType { keys, .. } = u {
                keys.sort();
            }
        }
        assert_eq!(
            got,
            vec![
                Unhandled::RecordType {
                    type_name: "brand-new".into(),
                    keys: vec!["type".into(), "x".into()],
                    renamed_turn: false,
                },
                Unhandled::RecordType {
                    type_name: "turn-v2".into(),
                    keys: vec!["message".into(), "timestamp".into(), "type".into()],
                    renamed_turn: true,
                },
                Unhandled::SystemSubtype {
                    subtype: "brand_new".into()
                },
            ]
        );
    }

    #[test]
    fn tool_name_mapping_order() {
        let r = feed(&[
            tool_use("toolu_fake01", "FrobTool", r#"{"a":1}"#),
            tool_use(
                "toolu_fake02",
                "mcp__ccd_turn__update_plan",
                r#"{"plan":[{"step":"s","status":"in_progress"}]}"#,
            ),
            tool_use(
                "toolu_fake03",
                "TodoWrite",
                r#"{"todos":[{"content":"c","status":"completed"}]}"#,
            ),
            tool_use(
                "toolu_fake04",
                "AskUserQuestion",
                r#"{"questions":[{"question":"Q?"}]}"#,
            ),
            tool_use(
                "toolu_fake05",
                "mcp__00000000-0000-4000-8000-000000000001__asset_add_file",
                "{}",
            ),
            tool_use("toolu_fake06", "mcp__github__create_issue", "{}"),
            tool_use(
                "toolu_fake07",
                "mcp__fc__firecrawl_search",
                r#"{"query":"q"}"#,
            ),
            tool_use(
                "toolu_fake08",
                "mcp__fc__firecrawl_scrape",
                r#"{"url":"https://example.invalid/a"}"#,
            ),
            tool_use("toolu_fake09", "mcp__terminal__run", "{}"),
            tool_result("toolu_fake09", r#","is_error":true"#, ""),
            tool_use(
                "toolu_fake10",
                "Read",
                r#"{"file_path":"/w/a.rs","offset":10,"limit":5}"#,
            ),
        ]);
        let calls: Vec<_> = r
            .events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolCall { tool, .. } => Some(tool.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                ToolKind::Other("FrobTool".into()),
                ToolKind::Todo,
                ToolKind::Todo,
                ToolKind::Ask,
                ToolKind::Mcp {
                    server: "asset".into(),
                    tool: "asset_add_file".into()
                },
                ToolKind::Mcp {
                    server: "github".into(),
                    tool: "create_issue".into()
                },
                ToolKind::WebSearch,
                ToolKind::WebFetch,
                ToolKind::FileRead,
            ]
        );
        assert_eq!(
            at(&r, 2)[1],
            EventKind::TodoChanged {
                items: vec![TodoItem {
                    content: "s".into(),
                    status: TodoStatus::InProgress
                }]
            }
        );
        assert_eq!(
            at(&r, 3)[1],
            EventKind::TodoChanged {
                items: vec![TodoItem {
                    content: "c".into(),
                    status: TodoStatus::Done
                }]
            }
        );
        assert_eq!(
            at(&r, 4)[1],
            EventKind::PermissionRequest {
                question: "Q?".into(),
                plan: false,
            }
        );
        let (_, _, s) = call_at(&r, 8);
        assert_eq!(s.pattern.as_deref(), Some("example.invalid"));
        let (_, _, s) = call_at(&r, 11);
        assert_eq!(s.range, Some((10, 14)));
        assert!(results(&r).is_empty());
        assert_eq!(
            r.unhandled,
            vec![Unhandled::ToolName {
                name: "FrobTool".into(),
                input_keys: vec!["a".into()]
            }]
        );
    }

    #[test]
    fn exit_plan_mode_is_a_plan_request() {
        let r = feed(&[tool_use("toolu_fake01", "ExitPlanMode", r#"{"plan":"P"}"#)]);
        assert_eq!(
            at(&r, 1)[1],
            EventKind::PermissionRequest {
                question: "P".into(),
                plan: true,
            }
        );
    }

    #[test]
    fn task_stop_closes_background_job_only_when_it_succeeds() {
        let start = [
            tool_use(
                "toolu_fake01",
                "Bash",
                r#"{"command":"npm run dev","run_in_background":true}"#,
            ),
            tool_result(
                "toolu_fake01",
                "",
                r#""toolUseResult":{"backgroundTaskId":"task01"},"#,
            ),
            tool_use("toolu_fake02", "TaskStop", r#"{"task_id":"task01"}"#),
        ];
        let stopped = tool_result("toolu_fake02", r#","content":"stopped""#, "");
        let r = feed(&[&start[..], &[stopped]].concat());
        assert_eq!(
            results(&r),
            vec![
                ("toolu_fake01".into(), ToolOutcome::Started),
                ("toolu_fake01".into(), ToolOutcome::Unknown),
            ]
        );
        assert!(r.unhandled.is_empty());

        // 중단 실패면 작업은 남고, 뒤의 완료 알림이 그대로 닫는다
        let failed = tool_result("toolu_fake02", r#","is_error":true"#, "");
        let done = notification(
            "task01",
            "toolu_fake01",
            "Background command completed (exit code 0)",
        );
        let r = feed(&[&start[..], &[failed, done]].concat());
        assert_eq!(
            results(&r),
            vec![
                ("toolu_fake01".into(), ToolOutcome::Started),
                ("toolu_fake01".into(), ToolOutcome::Ok),
            ]
        );
    }

    #[test]
    fn notification_shapes() {
        let bg = [
            tool_use(
                "toolu_fake01",
                "Bash",
                r#"{"command":"make","run_in_background":true}"#,
            ),
            tool_result(
                "toolu_fake01",
                "",
                r#""toolUseResult":{"backgroundTaskId":"task01"},"#,
            ),
        ];
        // exit code 가 없으면 판별 불가
        let stopped = notification("task01", "toolu_fake01", "Background command stopped");
        let r = feed(&[&bg[..], &[stopped]].concat());
        assert_eq!(
            results(&r)[1],
            ("toolu_fake01".into(), ToolOutcome::Unknown)
        );
        assert_eq!(
            r.unhandled,
            vec![Unhandled::OutcomeUnknown {
                tool: "Bash".into(),
                result_keys: vec![
                    "task-id".into(),
                    "tool-use-id".into(),
                    "status".into(),
                    "summary".into()
                ],
            }]
        );

        // 턴 중에는 queued_command 로 온다
        let queued = r#"{"type":"attachment","attachment":{"type":"queued_command","commandMode":"task-notification","prompt":"<task-notification>\n<task-id>task01</task-id>\n<tool-use-id>toolu_fake01</tool-use-id>\n<summary>failed with exit code 2</summary>\n</task-notification>"}}"#;
        let r = feed(&[&bg[..], &[queued.to_string()]].concat());
        assert_eq!(results(&r)[1], ("toolu_fake01".into(), ToolOutcome::Err));

        // 비동기 서브의 완료
        let r = feed(&[
            tool_use(
                "toolu_fake02",
                "Agent",
                r#"{"description":"d","prompt":"p","run_in_background":true}"#,
            ),
            tool_result(
                "toolu_fake02",
                "",
                r#""toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"a0000000000000001","resolvedModel":"claude-fake-1"},"#,
            ),
            notification("a0000000000000001", "toolu_fake02", "Agent completed"),
        ]);
        assert_eq!(
            at(&r, 3),
            vec![EventKind::SubagentDone {
                child_id: "a0000000000000001".into()
            }]
        );
        assert_eq!(results(&r).len(), 1);
    }

    #[test]
    fn who_is_a_user_message() {
        let user = |extra: &str, content: &str| {
            format!(r#"{{"type":"user",{extra}"message":{{"role":"user","content":{content}}}}}"#)
        };
        let queued = |prompt: &str| {
            format!(
                r#"{{"type":"attachment","attachment":{{"type":"queued_command","prompt":"{prompt}"}}}}"#
            )
        };
        let r = feed(&[
            user(r#""origin":{"kind":"peer"},"#, r#""hello""#),
            user("", r#""[Request interrupted by user for tool use]""#),
            user(
                "",
                r#"[{"type":"text","text":"[Request interrupted by user for tool use]"}]"#,
            ),
            user("", r#""<bash-stdout>x</bash-stdout>""#),
            user("", r#""<artifact-view-context>x""#),
            user(r#""isMeta":true,"#, r#""note""#),
            queued(r#"<agent-message from=\"x\">hi</agent-message>"#),
            queued("<system-reminder>x</system-reminder>"),
            queued("also fix the header"),
            user(r#""origin":{"kind":"human"},"#, r#""[Image #1] look""#),
        ]);
        assert_eq!(user_lines(&r), vec![9, 10]);
        assert!(r.unhandled.is_empty());
    }

    #[test]
    fn result_outcome_shapes() {
        let r = feed(&[
            tool_use("toolu_fake01", "Bash", r#"{"command":"git push &"}"#),
            tool_result("toolu_fake01", r#","content":"""#, ""),
            tool_use("toolu_fake02", "Bash", r#"{"command":"ls"}"#),
            tool_result(
                "toolu_fake02",
                r#","is_error":true,"content":"The user doesn't want to proceed with this tool use. x""#,
                "",
            ),
            tool_use("toolu_fake03", "Bash", r#"{"command":"ls"}"#),
            tool_result(
                "toolu_fake03",
                r#","is_error":true"#,
                r#""toolDenialKind":"automode-blocked","#,
            ),
        ]);
        assert_eq!(
            results(&r),
            vec![
                ("toolu_fake01".into(), ToolOutcome::Unknown),
                ("toolu_fake02".into(), ToolOutcome::Rejected),
                ("toolu_fake03".into(), ToolOutcome::Rejected),
            ]
        );
        assert!(
            matches!(&r.unhandled[..], [Unhandled::OutcomeUnknown { tool, .. }] if tool == "Bash")
        );
    }

    #[test]
    fn usage_and_model_skip_synthetic_zero_records() {
        let r = feed(&[
            r#"{"type":"assistant","cwd":"C:\\w","entrypoint":"cli","message":{"id":"msg_fake","model":"<synthetic>","content":[{"type":"text","text":"API Error"}],"usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}"#,
        ]);
        let kinds: Vec<_> = r.events.iter().map(|e| &e.kind).collect();
        assert!(matches!(
            kinds[..],
            [
                EventKind::SessionMeta { model: None, .. },
                EventKind::AssistantText { .. }
            ]
        ));
    }

    #[test]
    fn malformed_lines_do_not_drop_the_record_or_panic() {
        let r = feed(&[
            "not json",
            "[1,2]",
            "",
            r#"{"type":"user","timestamp":5,"message":{"content":"hi"}}"#,
            r#"{"type":"user","timestamp":"2026-03-02T18:00:00+09:00","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":7,"name":"Read"},{"type":"text","text":5}],"usage":"x"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_none","content":{"weird":1}}]},"toolUseResult":"Error: x"}"#,
        ]);
        let ts: Vec<_> = r.events.iter().map(|e| (e.raw_ref.line_no, e.ts)).collect();
        assert_eq!(
            ts,
            vec![
                (4, None),
                (5, Some("2026-03-02T09:00:00Z".parse().unwrap())),
                (7, None),
            ]
        );
        assert!(r.unhandled.is_empty());
    }

    #[test]
    fn background_without_task_id_and_agent_call_stays_ok() {
        let r = feed(&[
            tool_use(
                "toolu_fake01",
                "PowerShell",
                r#"{"command":"make","run_in_background":true}"#,
            ),
            tool_result("toolu_fake01", "", ""),
            tool_use(
                "toolu_fake02",
                "Agent",
                r#"{"description":"d","prompt":"p","run_in_background":true}"#,
            ),
            tool_result("toolu_fake02", "", ""),
        ]);
        assert_eq!(
            results(&r),
            vec![
                ("toolu_fake01".into(), ToolOutcome::Started),
                ("toolu_fake02".into(), ToolOutcome::Ok),
            ]
        );
    }

    #[test]
    fn notification_leaks_no_prose_and_reads_exit_code_only_at_the_end() {
        let bg = [
            tool_use(
                "toolu_fake01",
                "Bash",
                r#"{"command":"make","run_in_background":true}"#,
            ),
            tool_result(
                "toolu_fake01",
                "",
                r#""toolUseResult":{"backgroundTaskId":"task01"},"#,
            ),
        ];
        let prose = notification(
            "task01",
            "toolu_fake01",
            r#"Background command \"check exit code 0\" was stopped"#,
        )
        .replace("</summary>", "</summary>\\n<secret.txt>");
        let r = feed(&[&bg[..], &[prose]].concat());
        assert_eq!(results(&r)[1].1, ToolOutcome::Unknown);
        let [Unhandled::OutcomeUnknown { result_keys, .. }] = &r.unhandled[..] else {
            panic!("{:?}", r.unhandled)
        };
        assert_eq!(
            result_keys,
            &["task-id", "tool-use-id", "status", "summary"]
        );

        assert_eq!(exit_code("done (exit code 0). "), Some(0));
        assert_eq!(exit_code("failed with exit code 2"), Some(2));
        assert_eq!(
            exit_code(r#"Background command "exit code 0" stopped"#),
            None
        );
    }

    #[test]
    fn rejection_text_needs_is_error() {
        let r = feed(&[
            tool_use("toolu_fake01", "Bash", r#"{"command":"ls"}"#),
            tool_result(
                "toolu_fake01",
                r#","content":"The user doesn't want to proceed with this tool use. x""#,
                "",
            ),
        ]);
        assert_eq!(results(&r), vec![("toolu_fake01".into(), ToolOutcome::Ok)]);
    }

    #[test]
    fn sub_parser_never_emits_meta_and_keeps_last_agent_id() {
        let mut p = ClaudeParser::new(1, Some("parent".into()));
        let text = [
            r#"{"type":"user","agentId":"a1","sessionId":"s1","cwd":"C:\\w","message":{"content":"hi"}}"#,
            r#"{"type":"user","sessionId":"s1","cwd":"C:\\w","message":{"content":"again"}}"#,
        ]
        .join("\n");
        let r = run_bytes(text.as_bytes(), Path::new("sub.jsonl"), &mut p);
        assert_eq!(r.events.len(), 2);
        assert!(
            r.events
                .iter()
                .all(|e| e.agent_id == "a1" && matches!(e.kind, EventKind::UserMessage { .. }))
        );
    }

    #[test]
    fn missing_git_branch_keeps_the_previous_one() {
        let rec = |b: &str| {
            format!(
                r#"{{"type":"assistant","cwd":"C:\\w","entrypoint":"cli",{b}"sessionId":"s1","message":{{"content":[]}}}}"#
            )
        };
        let r = feed(&[rec(r#""gitBranch":"main","#), rec("")]);
        assert_eq!(
            r.events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::SessionMeta { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn nested_subagent_notification_with_usage_is_done() {
        let n = notification("a9999999999999999", "toolu_fake01", "Agent completed")
            .replace("</summary>", "</summary>\\n<usage>total_tokens: 1</usage>");
        let r = feed(&[n]);
        assert_eq!(
            at(&r, 1),
            vec![EventKind::SubagentDone {
                child_id: "a9999999999999999".into()
            }]
        );
    }

    #[test]
    fn pasted_image_makes_a_user_message() {
        let user = |content: &str| {
            format!(r#"{{"type":"user","message":{{"role":"user","content":{content}}}}}"#)
        };
        let r = feed(&[
            user(
                r#"[{"type":"text","text":"look"},{"type":"image","source":{}},{"type":"text","text":"here"}]"#,
            ),
            user(r#"[{"type":"text","text":"plain"}]"#),
        ]);
        assert_eq!(user_lines(&r), vec![1]);
        assert!(
            matches!(&at(&r, 1)[..], [EventKind::UserMessage { text }] if text == "look\nhere")
        );
    }

    #[test]
    fn commit_message_is_carried_to_summary() {
        let r = feed(&[tool_use(
            "toolu_c1",
            "Bash",
            r#"{"command":"git commit -m \"fix: x\""}"#,
        )]);
        let (_, tool, summary) = call_at(&r, 1);
        assert_eq!(tool, ToolKind::GitCommit);
        assert_eq!(summary.commit_message.as_deref(), Some("fix: x"));
        assert!(!summary.escalated);
    }
}
