//! Codex 기록 파서 (PLAN 4.6.2)

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::{
    Classified, Compound, Confidence, Event, EventKind, Parser, RawRef, ToolKind, ToolOutcome,
    ToolSummary, Unhandled, classify_command, mcp_kind, parse_test_result, rank,
};
use crate::discover::Product;

/// 승인 창 거부 때 하네스가 쓰는 문구. 결과 text 안에서는 따옴표가 `\"` 로 이스케이프돼 있다
const REJECTED: &str = r#"Rejected(\"rejected by user\")"#;
const QUESTION_REPLY: &str = "<send_user_message_question_reply>";
// ponytail: 미리보기 길이는 PLAN에 정한 값이 없다, 팝업(3.7)을 붙일 때 맞춘다
const PREVIEW_CHARS: usize = 200;

/// rollout 파일 하나에 하나. 파일이 Reset되면 새로 만든다
pub struct CodexParser {
    /// 파일명 끝 UUID. 정본 meta를 보기 전까지 agent_id로 쓴다
    file_id: String,
    /// 첫 번째 비어 있지 않은 `session_meta.payload.id`
    canonical: Option<String>,
    /// 자식 rollout이면 부모 thread id
    parent: Option<String>,
    depth: u8,
    /// 자식 rollout 앞의 부모 이력 복제 구간 끝
    history_start: Option<u64>,
    meta: Option<Meta>,
    model: Option<String>,
    /// 결과를 기다리는 호출. 호출 순서대로
    pending: Vec<Pending>,
    /// 백그라운드로 남은 exec_command 의 session_id → 원래 호출의 call_id
    background: HashMap<String, String>,
    /// 이벤트를 내지 않는 호출(`sleep`, `send_message`). 뒤따르는 출력도 버린다
    ignored: HashSet<String>,
    /// 내부 에이전트의 rollout. 파일 전체에서 이벤트를 내지 않는다
    silent: bool,
    /// 마지막 token_count 의 누적 사용량 (입력, 캐시, 출력). 누적이 그대로인 기록은 같은 사용량을 다시 쓴 것이다
    last_total: Option<(u64, u64, u64)>,
}

struct Meta {
    cwd: PathBuf,
    product: Product,
    git_branch: Option<String>,
}

struct Pending {
    call_id: String,
    /// `OutcomeUnknown.tool`
    tool: String,
    wait: Wait,
}

enum Wait {
    Exec(ExecWait),
    /// `ask` 면 `{accepted: true}` 를 받은 뒤에도 질문 답을 기다린다
    Function {
        ask: bool,
    },
}

#[derive(Default)]
struct ExecWait {
    /// 이 호출이 부른 `tools.<이름>`. item 은 맞는 종류를 부른 exec 에만 붙는다
    tools: Vec<String>,
    /// 단독 `&` 로 띄운 명령. 셸이 즉시 0을 돌려준다 (4.6.4)
    async_unknown: bool,
    /// Test 로 분류된 호출. 결과 본문에서 도장 숫자를 뽑는다
    test: bool,
    /// `write_stdin` 으로 `\u0003` 을 보낸 백그라운드 session_id
    stop_session: Option<String>,
    /// 중단을 기다리는 동안 온 그 세션의 item. 중단이 실패하면 이것으로 닫는다
    held: Option<Value>,
    items: u32,
    completed: u32,
    failed: u32,
}

impl ExecWait {
    fn calls(&self, f: impl Fn(&str) -> bool) -> bool {
        self.tools.iter().any(|t| f(t))
    }

    /// `write_stdin` 폴링만 한 호출은 결과에 판정 근거가 없는 게 정상이다
    fn polling_only(&self) -> bool {
        !self.tools.is_empty() && self.tools.iter().all(|t| t == "write_stdin")
    }
}

impl CodexParser {
    pub fn new(file_id: impl Into<String>) -> Self {
        Self {
            file_id: file_id.into(),
            canonical: None,
            parent: None,
            depth: 0,
            history_start: None,
            meta: None,
            model: None,
            pending: Vec::new(),
            background: HashMap::new(),
            ignored: HashSet::new(),
            silent: false,
            last_total: None,
        }
    }

    fn session_meta(&mut self, p: &Value, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
        // 정본 뒤의 meta는 부모 이력과 함께 복제된 남의 것이다
        if self.canonical.is_some() {
            return;
        }
        let Some(id) = str_at(p, &["id"]).filter(|s| !s.is_empty()) else {
            return;
        };
        self.canonical = Some(id.to_string());
        let internal_source = match p.pointer("/source/subagent") {
            Some(Value::String(_)) => true,
            Some(v) => v.get("other").is_some(),
            None => false,
        };
        if internal_source
            || matches!(
                str_at(p, &["thread_source"]),
                Some("guardian_review" | "roborev")
            )
        {
            self.silent = true;
            return;
        }
        let spawn = p.pointer("/source/subagent/thread_spawn");
        self.parent = str_at(p, &["parent_thread_id"])
            .or_else(|| spawn.and_then(|s| str_at(s, &["parent_thread_id"])))
            .map(str::to_string);
        self.depth = spawn
            .and_then(|s| s.get("depth"))
            .and_then(Value::as_u64)
            .map(|d| u8::try_from(d).unwrap_or(u8::MAX))
            .unwrap_or(u8::from(self.parent.is_some()));
        self.history_start = p
            .get("subagent_history_start_ordinal")
            .and_then(Value::as_u64);
        let product = match str_at(p, &["originator"]) {
            Some("Codex Desktop") => Product::CodexApp,
            Some("codex-tui") => Product::CodexCli,
            other => {
                if let Some(v) = other {
                    un.push(Unhandled::ProductValue {
                        value: v.to_string(),
                    });
                }
                Product::CodexCli
            }
        };
        self.meta = Some(Meta {
            cwd: PathBuf::from(str_at(p, &["cwd"]).unwrap_or_default()),
            product,
            git_branch: str_at(p, &["git", "branch"]).map(str::to_string),
        });
        out.extend(self.meta_event());
    }

    fn meta_event(&self) -> Option<EventKind> {
        let m = self.meta.as_ref()?;
        Some(EventKind::SessionMeta {
            cwd: m.cwd.clone(),
            model: self.model.clone(),
            title: None,
            product: m.product,
            git_branch: m.git_branch.clone(),
        })
    }

    fn turn_context(&mut self, p: &Value, out: &mut Vec<EventKind>) {
        let Some(model) = str_at(p, &["model"]) else {
            return;
        };
        // turn_context 는 턴마다 온다. 바뀔 때만 알린다
        if self.model.as_deref() == Some(model) {
            return;
        }
        self.model = Some(model.to_string());
        out.extend(self.meta_event());
    }

    fn response_item(
        &mut self,
        record: &Value,
        p: &Value,
        raw_ref: &RawRef,
        out: &mut Vec<EventKind>,
        un: &mut Vec<Unhandled>,
    ) {
        match str_at(p, &["type"]).unwrap_or_default() {
            "message" => self.message(record, p, out, un),
            "reasoning" => out.push(EventKind::Thinking),
            "custom_tool_call" => self.custom_tool_call(p, out, un),
            "custom_tool_call_output" => match str_at(p, &["call_id"]) {
                Some(call_id) => self.exec_output(call_id, p.get("output"), out, un),
                None => un.push(Unhandled::OutcomeUnknown {
                    tool: "exec".into(),
                    result_keys: keys(p),
                }),
            },
            "function_call" => self.function_call(p, out, un),
            // call_id 없는 출력은 호스트가 보낸 것이라 정상 모양이다
            "function_call_output" => {
                if let Some(call_id) = str_at(p, &["call_id"]) {
                    self.function_output(call_id, p.get("output"), out, un);
                }
            }
            "web_search_call" => {
                let call_id = str_at(p, &["call_id"])
                    .or_else(|| str_at(p, &["id"]))
                    .map_or_else(|| format!("ws-{}", raw_ref.byte_offset), str::to_string);
                out.push(EventKind::ToolCall {
                    call_id: call_id.clone(),
                    tool: ToolKind::WebSearch,
                    summary: empty_summary(),
                });
                // 따로 오는 출력 레코드가 없어 같은 레코드의 status 로 닫는다
                let outcome = match str_at(p, &["status"]) {
                    Some("completed") => ToolOutcome::Ok,
                    Some("failed") => ToolOutcome::Err,
                    _ => ToolOutcome::Unknown,
                };
                out.push(result(call_id, outcome, String::new()));
            }
            "additional_tools" | "agent_message" => {}
            other => un.push(Unhandled::RecordType {
                type_name: other.to_string(),
                keys: keys(p),
                renamed_turn: false,
            }),
        }
    }

    fn message(
        &mut self,
        record: &Value,
        p: &Value,
        out: &mut Vec<EventKind>,
        un: &mut Vec<Unhandled>,
    ) {
        let text = output_texts(p.get("content")).concat();
        match str_at(p, &["role"]) {
            Some("user") => {
                // 주입 문맥이 섞이므로 사람이 친 입력 표식이 있는 것만
                if record.pointer("/metadata/user_input_order").is_none() {
                    return;
                }
                let head = text.trim_start();
                if head.starts_with(QUESTION_REPLY) {
                    self.question_reply(&text, out);
                } else if let Some(body) = head.strip_prefix("<subagent_notification>") {
                    // 표본이 없는 모양이라 이벤트는 내지 않고 미대응 로그만 남긴다
                    let json = body
                        .split("</subagent_notification>")
                        .next()
                        .unwrap_or_default();
                    let body = serde_json::from_str(json.trim()).unwrap_or(Value::Null);
                    un.push(Unhandled::RecordType {
                        type_name: "subagent_notification".into(),
                        keys: keys(&body),
                        renamed_turn: false,
                    });
                } else {
                    out.push(EventKind::UserMessage { text });
                }
            }
            Some("assistant") if !text.is_empty() => out.push(EventKind::AssistantText { text }),
            _ => {}
        }
    }

    /// 답 JSON의 `questionItemId` 안에 질문 호출의 call_id가 들어 있다
    fn question_reply(&mut self, text: &str, out: &mut Vec<EventKind>) {
        let mut i = 0;
        while i < self.pending.len() {
            let p = &self.pending[i];
            if matches!(p.wait, Wait::Function { ask: true })
                && !p.call_id.is_empty()
                && text.contains(&p.call_id)
            {
                let p = self.pending.remove(i);
                out.push(result(p.call_id, ToolOutcome::Ok, String::new()));
            } else {
                i += 1;
            }
        }
    }

    fn custom_tool_call(&mut self, p: &Value, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
        let call_id = str_at(p, &["call_id"]).unwrap_or_default().to_string();
        let name = str_at(p, &["name"]).unwrap_or_default();
        if name != "exec" {
            un.push(Unhandled::ToolName {
                name: name.to_string(),
                input_keys: keys(p.get("input").unwrap_or(&Value::Null)),
            });
            out.push(EventKind::ToolCall {
                call_id: call_id.clone(),
                tool: ToolKind::Other(name.to_string()),
                summary: empty_summary(),
            });
            self.pending.push(Pending {
                call_id,
                tool: name.to_string(),
                wait: Wait::Exec(ExecWait::default()),
            });
            return;
        }
        let scan = scan_exec(str_at(p, &["input"]).unwrap_or_default());
        let (tool, mut summary, async_unknown) = exec_tool(&scan);
        summary.escalated = scan.escalated;
        let ask = tool == ToolKind::Ask;
        let test = tool == ToolKind::Test;
        out.push(EventKind::ToolCall {
            call_id: call_id.clone(),
            tool,
            summary,
        });
        if ask {
            out.push(EventKind::PermissionRequest {
                question: scan.question.clone().unwrap_or_default(),
                plan: false,
            });
        }
        let stop_session = scan
            .stdin
            .iter()
            .find(|(s, chars)| {
                chars.as_deref() == Some("\u{3}")
                    && s.as_ref().is_some_and(|s| self.background.contains_key(s))
            })
            .and_then(|(s, _)| s.clone());
        self.pending.push(Pending {
            call_id,
            tool: "exec".into(),
            wait: Wait::Exec(ExecWait {
                tools: scan.tools,
                async_unknown,
                test,
                stop_session,
                ..ExecWait::default()
            }),
        });
    }

    fn exec_output(
        &mut self,
        call_id: &str,
        output: Option<&Value>,
        out: &mut Vec<EventKind>,
        un: &mut Vec<Unhandled>,
    ) {
        let Some(i) = self
            .pending
            .iter()
            .position(|p| p.call_id == call_id && matches!(p.wait, Wait::Exec(_)))
        else {
            return;
        };
        let p = self.pending.remove(i);
        let Wait::Exec(w) = p.wait else {
            unreachable!("위에서 Exec 만 골랐다")
        };
        let texts = output_texts(output);
        let first = texts.first().map(String::as_str).unwrap_or_default();
        // 둘째 text부터는 모델이 찍은 값이라 모양이 정해져 있지 않다. 백그라운드 판별에만 쓴다
        let objs: Vec<Map<String, Value>> = texts
            .iter()
            .skip(1)
            .filter_map(|t| match serde_json::from_str(t) {
                Ok(Value::Object(m)) => Some(m),
                _ => None,
            })
            .collect();
        // 이미 백그라운드인 세션의 폴링 응답은 새 작업이 아니다. 원래 호출의 짝을 덮어쓰면 안 된다
        let started = objs
            .iter()
            .find(|m| m.contains_key("session_id"))
            .filter(|m| !m.contains_key("exit_code"))
            .and_then(|m| m.get("session_id"))
            .and_then(id_string)
            .filter(|s| w.calls(|t| t == "exec_command") && !self.background.contains_key(s));
        // 거부 문구는 item 없이 `Script failed` 로 끝난 호출에서만 본다. 성공한 호출이 text() 로 찍은 값에도 있을 수 있다
        let rejected = w.items == 0
            && first.starts_with("Script failed")
            && texts.iter().any(|t| t.contains(REJECTED));
        let mut outcome = if rejected {
            ToolOutcome::Rejected
        } else if w.failed > 0 {
            ToolOutcome::Err
        } else if started.is_some() {
            ToolOutcome::Started
        } else if w.items > 0 && w.completed == w.items {
            ToolOutcome::Ok
        } else if first.starts_with("Script failed") {
            ToolOutcome::Err
        } else {
            ToolOutcome::Unknown
        };
        if outcome == ToolOutcome::Ok && w.async_unknown {
            outcome = ToolOutcome::Unknown;
        }
        if outcome == ToolOutcome::Started
            && let Some(s) = started
        {
            self.background.insert(s, p.call_id.clone());
        }
        let rest = texts.get(1..).unwrap_or_default().join(
            "
",
        );
        let preview = if rest.is_empty() { first } else { &rest };
        let mut ev = result(p.call_id, outcome, preview.to_string());
        if w.test
            && matches!(outcome, ToolOutcome::Ok | ToolOutcome::Err)
            && let EventKind::ToolResult { test_result, .. } = &mut ev
        {
            *test_result = parse_test_result(&texts.join(
                "
",
            ));
        }
        out.push(ev);
        if outcome == ToolOutcome::Unknown && !w.polling_only() {
            let mut result_keys: Vec<String> = objs
                .iter()
                .flat_map(|m| m.keys().filter(|k| is_known_key(k)).cloned())
                .collect();
            result_keys.sort();
            result_keys.dedup();
            un.push(Unhandled::OutcomeUnknown {
                tool: p.tool,
                result_keys,
            });
        }
        // TaskStop 과 같게: 중단이 실패하지 않았을 때만 작업표를 뗀다
        if let Some(s) = w.stop_session
            && self.background.contains_key(&s)
        {
            if matches!(outcome, ToolOutcome::Err | ToolOutcome::Rejected) {
                if let Some(item) = w.held {
                    self.background_item(&s, &item, out, un);
                }
            } else if let Some(bg) = self.background.remove(&s) {
                out.push(result(bg, ToolOutcome::Unknown, String::new()));
            }
        }
    }

    fn function_call(&mut self, p: &Value, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
        let call_id = str_at(p, &["call_id"]).unwrap_or_default().to_string();
        let name = str_at(p, &["name"]).unwrap_or_default();
        let args = parse_maybe_json(p.get("arguments"));
        let mut ask = false;
        let tool = match name {
            // `sleep` 은 질문 답을 기다리며 부르고 `send_message` 는 에이전트 사이 배관이다. 연출할 게 없다
            "sleep" | "send_message" => {
                self.ignored.insert(call_id);
                return;
            }
            "request_user_input_async" | "request_user_input" | "request_permissions" => {
                ask = true;
                ToolKind::Ask
            }
            "spawn_agent" => ToolKind::Spawn,
            // 확인된 협업 배관이라 미대응이 아니다
            "wait_agent" | "followup_task" => ToolKind::Other(name.to_string()),
            _ if name.starts_with("mcp__") => mcp_kind(name),
            _ => {
                un.push(Unhandled::ToolName {
                    name: name.to_string(),
                    input_keys: keys(&args),
                });
                ToolKind::Other(name.to_string())
            }
        };
        out.push(EventKind::ToolCall {
            call_id: call_id.clone(),
            tool,
            summary: ToolSummary {
                escalated: args.get("sandbox_permissions").and_then(Value::as_str)
                    == Some("require_escalated"),
                ..empty_summary()
            },
        });
        if ask {
            let question = args
                .pointer("/questions/0/question")
                .and_then(Value::as_str)
                .unwrap_or_default();
            out.push(EventKind::PermissionRequest {
                question: question.to_string(),
                plan: false,
            });
        }
        self.pending.push(Pending {
            call_id,
            tool: name.to_string(),
            wait: Wait::Function { ask },
        });
    }

    fn function_output(
        &mut self,
        call_id: &str,
        output: Option<&Value>,
        out: &mut Vec<EventKind>,
        un: &mut Vec<Unhandled>,
    ) {
        if self.ignored.remove(call_id) {
            return;
        }
        let Some(i) = self
            .pending
            .iter()
            .position(|p| p.call_id == call_id && matches!(p.wait, Wait::Function { .. }))
        else {
            return;
        };
        let obj = parse_maybe_json(output);
        // 질문 접수일 뿐이다. 답은 뒤의 user 메시지로 온다
        if matches!(self.pending[i].wait, Wait::Function { ask: true })
            && obj.get("accepted") == Some(&Value::Bool(true))
        {
            return;
        }
        let p = self.pending.remove(i);
        // 접수 응답이라 Ok. 실제 성패는 SubAgentActivity 가 말한다. 그 밖은 성패 구조 필드가 없다
        let outcome = match p.tool.as_str() {
            "spawn_agent" | "wait_agent" | "followup_task" => ToolOutcome::Ok,
            _ => ToolOutcome::Unknown,
        };
        let preview = match output {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        };
        out.push(result(p.call_id, outcome, preview));
        if outcome == ToolOutcome::Unknown {
            un.push(Unhandled::OutcomeUnknown {
                tool: p.tool,
                result_keys: keys(&obj).into_iter().filter(|k| is_known_key(k)).collect(),
            });
        }
    }

    fn event_msg(&mut self, p: &Value, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
        match str_at(p, &["type"]).unwrap_or_default() {
            "item_completed" => {
                let item = p.get("item").unwrap_or(&Value::Null);
                match str_at(item, &["type"]) {
                    Some("CommandExecution" | "FileChange" | "McpToolCall") => {
                        self.exec_item(item, out, un)
                    }
                    Some("SubAgentActivity") => subagent_activity(item, self.depth, out, un),
                    Some("ContextCompaction") => out.push(EventKind::ContextCompaction {
                        pre_tokens: None,
                        post_tokens: None,
                    }),
                    _ => {}
                }
            }
            "token_count" => {
                // info 가 없으면 rate-limit 알림이다
                let Some(last) = p.pointer("/info/last_token_usage") else {
                    return;
                };
                let (Some(input), Some(output)) = (
                    last.get("input_tokens").and_then(Value::as_u64),
                    last.get("output_tokens").and_then(Value::as_u64),
                ) else {
                    return;
                };
                // 압축 직후의 0은 사용량이 아니다 (4.6.1)
                if input == 0 && output == 0 {
                    return;
                }
                let cached = last
                    .get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let total = p.pointer("/info/total_token_usage").map(|t| {
                    let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
                    (
                        n("input_tokens"),
                        n("cached_input_tokens"),
                        n("output_tokens"),
                    )
                });
                if total.is_some() && total == self.last_total {
                    return;
                }
                self.last_total = total.or(self.last_total);
                // input_tokens 는 cached_input_tokens 를 이미 포함한다
                out.push(EventKind::TokenUsage {
                    input_total: input,
                    cache_read: cached.min(input),
                    output,
                    context_window: p
                        .pointer("/info/model_context_window")
                        .and_then(Value::as_u64),
                });
            }
            // 중단된 턴의 대기 호출은 출력을 받는다는 보장이 없다
            "turn_aborted" => {
                for p in self.pending.drain(..) {
                    out.push(result(p.call_id, ToolOutcome::Unknown, String::new()));
                }
            }
            "sub_agent_activity" => subagent_activity(p, self.depth, out, un),
            "collab_agent_spawn_end" => {
                if let Some(child) = str_at(p, &["new_thread_id"]) {
                    out.push(spawn(child, None, None, self.depth));
                }
            }
            "task_started" | "task_complete" | "thread_settings_applied" => {}
            other => un.push(Unhandled::RecordType {
                type_name: other.to_string(),
                keys: keys(p),
                renamed_turn: false,
            }),
        }
    }

    /// item에 call_id가 없다. 호출과 결과 사이에 온 것을 그 종류를 부른 가장 최근 exec 의 것으로 본다
    fn exec_item(&mut self, item: &Value, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
        let ty = str_at(item, &["type"]).unwrap_or_default();
        if ty == "CommandExecution"
            && let Some(pid) = item.get("process_id").and_then(id_string)
            && self.background.contains_key(&pid)
        {
            // 중단 요청 중이면 그 결과(TaskStop 규칙)로 정리한다. 중단이 실패할 때를 위해 보관한다
            let stopping = self.pending.iter_mut().find_map(|p| match &mut p.wait {
                Wait::Exec(w) if w.stop_session.as_deref() == Some(&pid) => Some(w),
                _ => None,
            });
            match stopping {
                Some(w) => w.held = Some(item.clone()),
                None => self.background_item(&pid, item, out, un),
            }
            return;
        }
        let (completed, failed) = item_state(item);
        if let Some(w) = self
            .pending
            .iter_mut()
            .rev()
            .filter_map(|p| match &mut p.wait {
                Wait::Exec(w) => Some(w),
                Wait::Function { .. } => None,
            })
            .find(|w| {
                w.calls(|t| match ty {
                    "CommandExecution" => t == "exec_command",
                    "FileChange" => t == "apply_patch",
                    _ => t.starts_with("mcp__"),
                })
            })
        {
            w.items += 1;
            w.completed += u32::from(completed);
            w.failed += u32::from(failed);
        }
    }

    /// 백그라운드로 남은 작업이 끝났다는 item. 원래 호출의 짝을 닫는다
    fn background_item(
        &mut self,
        pid: &str,
        item: &Value,
        out: &mut Vec<EventKind>,
        un: &mut Vec<Unhandled>,
    ) {
        let (completed, failed) = item_state(item);
        let call_id = self.background.remove(pid).unwrap_or_default();
        let outcome = if failed {
            ToolOutcome::Err
        } else if completed {
            ToolOutcome::Ok
        } else {
            ToolOutcome::Unknown
        };
        out.push(result(call_id, outcome, String::new()));
        if outcome == ToolOutcome::Unknown {
            un.push(Unhandled::OutcomeUnknown {
                tool: "exec".into(),
                result_keys: keys(item),
            });
        }
    }

    fn agent_ids(&self) -> (String, String, Option<String>) {
        let agent = self
            .canonical
            .clone()
            .unwrap_or_else(|| self.file_id.clone());
        let session = self.parent.clone().unwrap_or_else(|| agent.clone());
        (session, agent, self.parent.clone())
    }
}

impl Parser for CodexParser {
    fn parse_line(
        &mut self,
        bytes: &[u8],
        raw_ref: &RawRef,
        unhandled: &mut Vec<Unhandled>,
    ) -> Vec<Event> {
        let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
            return Vec::new();
        };
        if self.silent || !v.is_object() {
            return Vec::new();
        }
        // 자식 rollout 앞부분은 부모 이력의 복제다. 첫 session_meta 는 이 값을 알기 전에 읽혀 걸리지 않는다
        if let (Some(start), Some(ord)) =
            (self.history_start, v.get("ordinal").and_then(Value::as_u64))
            && ord < start
        {
            return Vec::new();
        }
        let ts = str_at(&v, &["timestamp"])
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        let p = v.get("payload").unwrap_or(&Value::Null);
        let mut out = Vec::new();
        match str_at(&v, &["type"]).unwrap_or_default() {
            "session_meta" => self.session_meta(p, &mut out, unhandled),
            "turn_context" => self.turn_context(p, &mut out),
            "response_item" => self.response_item(&v, p, raw_ref, &mut out, unhandled),
            "event_msg" => self.event_msg(p, &mut out, unhandled),
            "compacted"
            | "world_state"
            | "token_usage_record"
            | "inter_agent_communication_metadata" => {}
            other => unhandled.push(Unhandled::RecordType {
                type_name: other.to_string(),
                keys: keys(&v),
                renamed_turn: false,
            }),
        }
        if self.silent {
            return Vec::new();
        }
        let (session_id, agent_id, parent_agent) = self.agent_ids();
        out.into_iter()
            .map(|kind| Event {
                ts,
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                parent_agent: parent_agent.clone(),
                kind,
                raw_ref: raw_ref.clone(),
            })
            .collect()
    }

    /// 부모 thread 가 있는 자식이거나 내부 에이전트의 rollout 은 팀이 되지 않는다 (4.6.2)
    fn side(&self) -> bool {
        self.silent || self.parent.is_some()
    }
}

/// `interacted` 는 추가 지시라 연출할 게 없다
fn subagent_activity(item: &Value, depth: u8, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
    let kind = str_at(item, &["kind"]).unwrap_or_default();
    match (kind, str_at(item, &["agent_thread_id"])) {
        ("started", Some(child)) => out.push(spawn(
            child,
            str_at(item, &["model"]),
            str_at(item, &["reasoning_effort"]),
            depth,
        )),
        ("completed", Some(child)) => out.push(EventKind::SubagentDone {
            child_id: child.to_string(),
        }),
        ("started" | "completed" | "interacted", _) => {}
        _ => un.push(Unhandled::RecordType {
            type_name: format!("SubAgentActivity.{kind}"),
            keys: keys(item),
            renamed_turn: false,
        }),
    }
}

/// 지시문은 암호화돼 있어 prompt 는 비운다
fn spawn(child: &str, model: Option<&str>, effort: Option<&str>, depth: u8) -> EventKind {
    EventKind::SubagentSpawn {
        child_id: child.to_string(),
        prompt: String::new(),
        model: model.map(str::to_string),
        description: None,
        effort: effort.map(str::to_string),
        depth: depth.saturating_add(1),
    }
}

fn result(call_id: String, outcome: ToolOutcome, preview: String) -> EventKind {
    EventKind::ToolResult {
        call_id,
        outcome,
        outcome_confidence: Confidence::Confirmed,
        preview: preview.chars().take(PREVIEW_CHARS).collect(),
        test_result: None,
    }
}

fn empty_summary() -> ToolSummary {
    ToolSummary {
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
    }
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(v, |v, k| v.get(k))?.as_str()
}

/// `arguments`·`output` 는 JSON을 담은 문자열일 때가 많다
fn parse_maybe_json(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    }
}

/// session_id·process_id 는 숫자일 때도 문자열일 때도 있다
fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    k.sort();
    k
}

/// (completed, failed). CommandExecution 은 `exit_code` 도 본다
fn item_state(item: &Value) -> (bool, bool) {
    let status = str_at(item, &["status"]);
    let exit = item.get("exit_code").and_then(Value::as_i64);
    let failed = status == Some("failed") || exit.is_some_and(|c| c != 0);
    (status == Some("completed") && !failed, failed)
}

/// 미대응 로그에는 하네스가 정한 키만 남긴다. 모델이 찍은 JSON의 키에는 경로 같은 원문이 든다
fn is_known_key(k: &str) -> bool {
    [
        "exit_code",
        "session_id",
        "output",
        "chunk_id",
        "wall_time_seconds",
        "original_token_count",
    ]
    .contains(&k)
}

fn output_texts(output: Option<&Value>) -> Vec<String> {
    match output {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Default)]
struct ExecScan {
    /// `cmd:` 문자열 리터럴. 변수·`${}` 템플릿은 못 뽑는다
    cmds: Vec<String>,
    /// `tools.<이름>` 으로 부른 도구
    tools: Vec<String>,
    /// 첫 `tools.apply_patch(` 의 리터럴 인자
    patch: Option<String>,
    question: Option<String>,
    /// `sandbox_permissions: "require_escalated"` 리터럴
    escalated: bool,
    /// `write_stdin` 의 (session_id, chars)
    stdin: Vec<(Option<String>, Option<String>)>,
}

fn exec_tool(s: &ExecScan) -> (ToolKind, ToolSummary, bool) {
    let only = |f: fn(&str) -> bool| !s.tools.is_empty() && s.tools.iter().all(|t| f(t));
    if s.tools
        .iter()
        .any(|t| t == "request_user_input" || t == "request_permissions")
    {
        return (ToolKind::Ask, empty_summary(), false);
    }
    if !s.cmds.is_empty() {
        let cs: Vec<Classified> = s.cmds.iter().map(|c| classify_command(c)).collect();
        // 동률이면 앞 명령이 이긴다 (classify_command 와 같은 규칙)
        let mut rep = &cs[0];
        for c in &cs[1..] {
            if rank(&c.kind) > rank(&rep.kind) {
                rep = c;
            }
        }
        let summary = ToolSummary {
            path: rep.path.clone(),
            command: Some(rep.command.clone()),
            command_key: Some(rep.command_key.clone()),
            command_norm: Some(rep.command_norm.clone()),
            commit_message: rep.commit_message.clone(),
            // 리터럴을 못 뽑은 호출이 섞여 있어도 명령은 둘 이상이다
            compound: if cs.len() > 1 || s.tools.iter().filter(|t| *t == "exec_command").count() > 1
            {
                Compound::Mixed
            } else {
                rep.compound
            },
            ..empty_summary()
        };
        return (
            rep.kind.clone(),
            summary,
            cs.iter().any(|c| c.async_unknown),
        );
    }
    if only(|t| t == "apply_patch") {
        let (kind, path) = patch_target(s.patch.as_deref().unwrap_or_default());
        return (
            kind,
            ToolSummary {
                path,
                ..empty_summary()
            },
            false,
        );
    }
    if only(|t| t.starts_with("web__")) {
        return (ToolKind::WebSearch, empty_summary(), false);
    }
    if only(|t| t.starts_with("mcp__")) {
        return (mcp_kind(&s.tools[0]), empty_summary(), false);
    }
    (ToolKind::Script, empty_summary(), false)
}

/// 첫 파일 머리줄. 머리줄을 못 찾으면 경로 없는 FileEdit
fn patch_target(patch: &str) -> (ToolKind, Option<String>) {
    for line in patch.lines() {
        for (prefix, kind) in [
            ("*** Add File:", ToolKind::FileWrite),
            ("*** Update File:", ToolKind::FileEdit),
            ("*** Delete File:", ToolKind::FileEdit),
            ("*** Move to:", ToolKind::FileEdit),
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                return (kind, Some(path.trim().to_string()));
            }
        }
    }
    (ToolKind::FileEdit, None)
}

#[derive(Debug, PartialEq)]
enum Tok {
    /// 식별자와 숫자
    Word(String),
    /// `plain` 은 끝까지 닫혔고 `${}` 가 없는 리터럴
    Str {
        text: String,
        plain: bool,
    },
    Punct(char),
}

fn scan_exec(src: &str) -> ExecScan {
    let t = tokenize(src);
    let mut s = ExecScan::default();
    let word = |i: usize, w: &str| matches!(t.get(i), Some(Tok::Word(x)) if x == w);
    let punct = |i: usize, c: char| t.get(i) == Some(&Tok::Punct(c));
    // `key: 값` 의 값. 키는 식별자거나 문자열이다
    let value_of = |i: usize, key: &str| -> Option<&Tok> {
        let is_key = match t.get(i)? {
            Tok::Word(w) => w == key,
            Tok::Str { text, .. } => text == key,
            Tok::Punct(_) => false,
        };
        if is_key && punct(i + 1, ':') {
            t.get(i + 2)
        } else {
            None
        }
    };
    for i in 0..t.len() {
        if word(i, "tools")
            && punct(i + 1, '.')
            && let Some(Tok::Word(name)) = t.get(i + 2)
        {
            s.tools.push(name.clone());
            if name == "apply_patch"
                && s.patch.is_none()
                && punct(i + 3, '(')
                && let Some(Tok::Str { text, .. }) = t.get(i + 4)
            {
                s.patch = Some(text.clone());
            }
            if name == "write_stdin" {
                let (mut sid, mut chars) = (None, None);
                // 이 호출의 짝 괄호 안만 본다. 뒤의 무관한 객체가 이기면 안 된다
                let (mut j, mut depth) = (i + 3, 0);
                while j < t.len() {
                    if punct(j, '(') {
                        depth += 1;
                    } else if punct(j, ')') {
                        depth -= 1;
                        if depth <= 0 {
                            break;
                        }
                    } else if depth == 0 {
                        break;
                    }
                    match value_of(j, "session_id") {
                        Some(Tok::Word(n)) => sid = Some(n.clone()),
                        Some(Tok::Str { text, .. }) => sid = Some(text.clone()),
                        _ => {}
                    }
                    if let Some(Tok::Str { text, .. }) = value_of(j, "chars") {
                        chars = Some(text.clone());
                    }
                    j += 1;
                }
                s.stdin.push((sid, chars));
            }
        }
        // 객체 리터럴의 키 자리만 본다. 삼항식의 `"cmd"` 나 `cmd: "git " + sub` 는 값이 아니다
        if let Some(Tok::Str { text, plain: true }) = value_of(i, "cmd")
            && i > 0
            && matches!(t[i - 1], Tok::Punct('{' | ','))
            && matches!(t.get(i + 3), Some(Tok::Punct(',' | '}' | ')')))
        {
            s.cmds.push(text.clone());
        }
        if let Some(Tok::Str { text, .. }) = value_of(i, "sandbox_permissions")
            && text == "require_escalated"
        {
            s.escalated = true;
        }
        if s.question.is_none()
            && let Some(Tok::Str { text, .. }) = value_of(i, "question")
        {
            s.question = Some(text.clone());
        }
    }
    s
}

/// 문자열·주석·정규식 리터럴을 건너뛰는 정도의 JS 토큰 나누기
fn tokenize(src: &str) -> Vec<Tok> {
    let cs: Vec<char> = src.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let next = cs.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c == '/'
            && !matches!(
                out.last(),
                Some(Tok::Word(_) | Tok::Str { .. } | Tok::Punct(')' | ']'))
            )
            && let Some(end) = regex_end(&cs, i)
        {
            // 안의 따옴표가 문자열로 읽히면 뒤의 `cmd:` 를 삼킨다
            out.push(Tok::Str {
                text: String::new(),
                plain: false,
            });
            i = end;
        } else if matches!(c, '"' | '\'' | '`') {
            let (text, plain, end) = read_str(&cs, i);
            out.push(Tok::Str { text, plain });
            i = end;
        } else if is_word(c) {
            let start = i;
            while i < cs.len() && is_word(cs[i]) {
                i += 1;
            }
            out.push(Tok::Word(cs[start..i].iter().collect()));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// `start` 의 `/` 가 여는 정규식의 다음 위치. 한 줄 안에서 안 닫히면 정규식이 아니다
fn regex_end(cs: &[char], start: usize) -> Option<usize> {
    let (mut i, mut class) = (start + 1, false);
    while i < cs.len() {
        match cs[i] {
            '\n' => return None,
            '\\' => i += 1,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    None
}

fn read_str(cs: &[char], start: usize) -> (String, bool, usize) {
    let q = cs[start];
    let mut s = String::new();
    let mut plain = true;
    let mut i = start + 1;
    while i < cs.len() {
        let c = cs[i];
        if c == q {
            return (s, plain, i + 1);
        }
        if c == '\\' {
            let Some(&e) = cs.get(i + 1) else {
                break;
            };
            i += 2;
            match e {
                'n' => s.push('\n'),
                't' => s.push('\t'),
                'r' => s.push('\r'),
                'b' => s.push('\u{8}'),
                'f' => s.push('\u{c}'),
                'v' => s.push('\u{b}'),
                '0' => s.push('\0'),
                'x' => i = push_hex(cs, i, 2, &mut s),
                'u' if cs.get(i) == Some(&'{') => match cs[i..].iter().position(|&c| c == '}') {
                    Some(len) => {
                        let hex: String = cs[i + 1..i + len].iter().collect();
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(ch) => s.push(ch),
                            None => plain = false,
                        }
                        i += len + 1;
                    }
                    None => plain = false,
                },
                'u' => i = push_hex(cs, i, 4, &mut s),
                // 줄 잇기
                '\n' => {}
                '\r' => {
                    if cs.get(i) == Some(&'\n') {
                        i += 1;
                    }
                }
                other => s.push(other),
            }
            continue;
        }
        if q == '`' && c == '$' && cs.get(i + 1) == Some(&'{') {
            plain = false;
        }
        // 따옴표 문자열 안의 맨 개행은 문법 오류다. 닫히지 않은 것으로 보고 거기서 끊는다
        if q != '`' && c == '\n' {
            return (s, false, i);
        }
        s.push(c);
        i += 1;
    }
    (s, false, i)
}

fn hex_at(cs: &[char], i: usize, n: usize) -> Option<u32> {
    let digits = cs.get(i..i + n)?;
    digits
        .iter()
        .all(char::is_ascii_hexdigit)
        .then(|| u32::from_str_radix(&digits.iter().collect::<String>(), 16).ok())?
}

/// `\x`·`\u` 뒤 16진 `n` 자리. 모양이 틀리면 글자를 그대로 둔다
fn push_hex(cs: &[char], i: usize, n: usize, s: &mut String) -> usize {
    let code = hex_at(cs, i, n);
    // 상위 서로게이트 뒤에 `\u` 하위가 오면 한 글자다
    if let Some(hi) = code.filter(|c| n == 4 && (0xD800..0xDC00).contains(c)) {
        let lo = (cs.get(i + 4) == Some(&'\\') && cs.get(i + 5) == Some(&'u'))
            .then(|| hex_at(cs, i + 6, 4))
            .flatten()
            .filter(|lo| (0xDC00..0xE000).contains(lo));
        if let Some(ch) =
            lo.and_then(|lo| char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)))
        {
            s.push(ch);
            return i + 10;
        }
    }
    match code.and_then(char::from_u32) {
        Some(ch) => {
            s.push(ch);
            i + n
        }
        None => {
            s.push(if n == 2 { 'x' } else { 'u' });
            i
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    const SID: &str = "00000000-0000-4000-8000-000000000001";
    const PARENT: &str = "00000000-0000-4000-8000-0000000000aa";
    const CHILD: &str = "00000000-0000-4000-8000-0000000000bb";
    const GRANDCHILD: &str = "00000000-0000-4000-8000-0000000000cc";

    fn raw(line_no: u64) -> RawRef {
        RawRef {
            path: PathBuf::from("rollout.jsonl"),
            generation: 0,
            byte_offset: 0,
            line_no,
        }
    }

    fn rec(ty: &str, payload: Value) -> String {
        json!({"timestamp": "2026-01-01T00:00:00Z", "type": ty, "payload": payload}).to_string()
    }

    fn ord_rec(ordinal: u64, ty: &str, payload: Value) -> String {
        json!({"timestamp": "2026-01-01T00:00:00Z", "ordinal": ordinal, "type": ty, "payload": payload})
            .to_string()
    }

    fn meta() -> String {
        rec(
            "session_meta",
            json!({"id": SID, "cwd": "/w", "originator": "Codex Desktop", "source": "vscode"}),
        )
    }

    fn exec(call_id: &str, js: &str) -> String {
        rec(
            "response_item",
            json!({"type": "custom_tool_call", "name": "exec", "call_id": call_id, "input": js}),
        )
    }

    fn exec_out(call_id: &str, texts: &[&str]) -> String {
        let output: Vec<Value> = texts
            .iter()
            .map(|t| json!({"type": "input_text", "text": t}))
            .collect();
        rec(
            "response_item",
            json!({"type": "custom_tool_call_output", "call_id": call_id, "output": output}),
        )
    }

    fn cmd_item(status: &str, exit: Option<i64>, pid: &str) -> String {
        rec(
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "CommandExecution", "id": "exec-x", "process_id": pid,
                "command": ["bash", "-lc", "x"], "status": status, "exit_code": exit}}),
        )
    }

    fn func(call_id: &str, name: &str, args: Value) -> String {
        rec(
            "response_item",
            json!({"type": "function_call", "name": name, "call_id": call_id,
                   "arguments": args.to_string()}),
        )
    }

    fn func_out(call_id: &str, output: &str) -> String {
        rec(
            "response_item",
            json!({"type": "function_call_output", "call_id": call_id, "output": output}),
        )
    }

    fn user(text: &str) -> String {
        json!({"timestamp": "2026-01-01T00:00:00Z", "type": "response_item",
               "payload": {"type": "message", "role": "user",
                           "content": [{"type": "input_text", "text": text}]},
               "metadata": {"user_input_order": 0}})
        .to_string()
    }

    fn feed_events(p: &mut CodexParser, lines: &[String]) -> (Vec<Event>, Vec<Unhandled>) {
        let mut un = Vec::new();
        let mut ev = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            ev.extend(p.parse_line(l.as_bytes(), &raw(i as u64 + 1), &mut un));
        }
        (ev, un)
    }

    /// meta 이벤트는 빼고 돌려준다
    fn feed(lines: &[String]) -> (Vec<EventKind>, Vec<Unhandled>) {
        let mut all = vec![meta()];
        all.extend_from_slice(lines);
        let (ev, un) = feed_events(&mut CodexParser::new(SID), &all);
        let kinds = ev
            .into_iter()
            .map(|e| e.kind)
            .filter(|k| !matches!(k, EventKind::SessionMeta { .. }))
            .collect();
        (kinds, un)
    }

    fn fixture_file(scenario: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/codex")
            .join(scenario);
        std::fs::read_dir(&dir)
            .expect("fixture 폴더")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .expect("rollout 파일")
    }

    fn fixture(scenario: &str) -> (Vec<Event>, Vec<Unhandled>) {
        let file = fixture_file(scenario);
        let stem = file.file_stem().unwrap().to_str().unwrap();
        let id = &stem[stem.len() - 36..];
        let lines: Vec<String> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let (ev, un) = feed_events(&mut CodexParser::new(id), &lines);
        for e in &ev {
            assert_eq!(e.session_id, id);
            assert_eq!(e.agent_id, id);
            assert_eq!(e.parent_agent, None);
        }
        (ev, un)
    }

    fn tag(k: &EventKind) -> String {
        match k {
            EventKind::SessionMeta { model, .. } => format!("meta {model:?}"),
            EventKind::UserMessage { .. } => "user".into(),
            EventKind::AssistantText { .. } => "text".into(),
            EventKind::Thinking => "think".into(),
            EventKind::ToolCall { tool, summary, .. } => {
                format!("call {tool:?} {:?}", summary.compound)
            }
            EventKind::ToolResult { outcome, .. } => format!("result {outcome:?}"),
            EventKind::TokenUsage {
                input_total,
                output,
                context_window,
                ..
            } => format!("tokens {input_total} {output} {context_window:?}"),
            EventKind::PermissionRequest { .. } => "ask".into(),
            EventKind::ContextCompaction { .. } => "compact".into(),
            other => format!("{other:?}"),
        }
    }

    fn tags(ev: &[Event]) -> Vec<String> {
        ev.iter().map(|e| tag(&e.kind)).collect()
    }

    fn call_at(ev: &[Event], line_no: u64) -> (&str, &ToolKind, &ToolSummary) {
        ev.iter()
            .find_map(|e| match &e.kind {
                EventKind::ToolCall {
                    call_id,
                    tool,
                    summary,
                } if e.raw_ref.line_no == line_no => Some((call_id.as_str(), tool, summary)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{line_no}번 줄에 ToolCall 없음"))
    }

    fn result_at(ev: &[Event], line_no: u64) -> (&str, ToolOutcome) {
        ev.iter()
            .find_map(|e| match &e.kind {
                EventKind::ToolResult {
                    call_id, outcome, ..
                } if e.raw_ref.line_no == line_no => Some((call_id.as_str(), *outcome)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{line_no}번 줄에 ToolResult 없음"))
    }

    fn summary_of(cmd: &str, compound: Compound) -> ToolSummary {
        let c = classify_command(cmd);
        ToolSummary {
            path: c.path,
            command: Some(c.command),
            command_key: Some(c.command_key),
            command_norm: Some(c.command_norm),
            commit_message: c.commit_message,
            compound,
            ..empty_summary()
        }
    }

    fn only_call(kinds: &[EventKind]) -> (&ToolKind, &ToolSummary) {
        kinds
            .iter()
            .find_map(|k| match k {
                EventKind::ToolCall { tool, summary, .. } => Some((tool, summary)),
                _ => None,
            })
            .expect("ToolCall")
    }

    fn outcome_of(kinds: &[EventKind], id: &str) -> Vec<ToolOutcome> {
        kinds
            .iter()
            .filter_map(|k| match k {
                EventKind::ToolResult {
                    call_id, outcome, ..
                } if call_id == id => Some(*outcome),
                _ => None,
            })
            .collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_fixture_line_parses_without_panic() {
        fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, files);
                } else if p.extension().is_some_and(|x| x == "jsonl") {
                    files.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/codex"),
            &mut files,
        );
        assert!(!files.is_empty());
        for f in files {
            let mut p = CodexParser::new(SID);
            let mut un = Vec::new();
            for (i, l) in std::fs::read_to_string(&f).unwrap().lines().enumerate() {
                p.parse_line(l.as_bytes(), &raw(i as u64 + 1), &mut un);
            }
        }
    }

    #[test]
    fn codex_app_fixture() {
        let (ev, un) = fixture("codex-app");
        let want = [
            "meta None",                     // 1
            "meta Some(\"gpt-6-astra\")",    // 7
            "user",                          // 11 user_input_order 0
            "text",                          // 14
            "call Shell Mixed",              // 15
            "result Ok",                     // 18
            "tokens 32116 142 Some(258400)", // 19
            "think",                         // 21
            "call Shell Single",             // 22
            "result Ok",                     // 25
            "tokens 32393 163 Some(258400)", // 26
            "think",                         // 28
            "text",                          // 30
            "call Shell Single",             // 31
            "result Started",                // 33 session_id 있고 exit_code 없음
            "tokens 33279 112 Some(258400)", // 34
            "call Script Single",            // 35 write_stdin
            "result Unknown",                // 37
            "tokens 33598 40 Some(258400)",  // 38
            "think",                         // 40
            "text",                          // 42
            "call Script Single",            // 43
            "result Unknown",                // 45
            "tokens 33697 90 Some(258400)",  // 46
            "think",                         // 48
            "call Script Single",            // 49
            "result Unknown",                // 51
            "tokens 33845 63 Some(258400)",  // 52
            "think",                         // 54
            "call Script Single",            // 55
            "result Unknown",                // 57
            "tokens 33966 59 Some(258400)",  // 58
            "think",                         // 60
            "call Shell Mixed",              // 61
            "result Ok",                     // 64
            "tokens 34082 164 Some(258400)", // 65
            "think",                         // 67
            // 68 item 은 열린 exec 도, 짝이 맞는 백그라운드 session_id 도 없다
            "call Script Single",            // 69
            "result Unknown",                // 71
            "tokens 34411 61 Some(258400)",  // 72
            "call Shell Mixed",              // 73
            "result Ok",                     // 76
            "tokens 34531 196 Some(258400)", // 77
            "text",                          // 79
            "tokens 34852 54 Some(258400)",  // 81
        ];
        assert_eq!(tags(&ev), want);

        assert_eq!(
            ev[0].kind,
            EventKind::SessionMeta {
                cwd: PathBuf::from(r"C:\dir1"),
                model: None,
                title: None,
                product: Product::CodexApp,
                git_branch: None,
            }
        );
        assert_eq!(
            ev[0].ts,
            Some(
                DateTime::parse_from_rfc3339("2026-03-02T09:00:00Z")
                    .unwrap()
                    .into()
            )
        );

        // JSON → JS 두 겹을 벗긴 `cmd:` 리터럴
        let lit15 = r#"prog1 arg2 arg3 arg4 arg5 | prog6 arg7 arg8; $prog9 = arg10('C:\dir8 dir9\dir10\dir11\file12.exe'arg11 "$dir13\dir14\dir10\dir11\file12.exe"); prog12 ($arg13 arg14 $arg9) { arg15 (arg16 arg17 $arg13) { & $prog13 --flag1 } }"#;
        let (id, tool, summary) = call_at(&ev, 15);
        assert_eq!(id, "call_e27d3a940098c1bae7a9a6992762efd4");
        assert_eq!(tool, &ToolKind::Shell);
        assert_eq!(summary, &summary_of(lit15, Compound::Mixed));
        assert_eq!(summary.command_key.as_deref(), Some("prog1"));

        let lit22 = "winget arg19 --flag2 dir22 --flag3 --flag4 arg3 --flag5 --flag6";
        assert_eq!(call_at(&ev, 22).2, &summary_of(lit22, Compound::Single));
        assert_eq!(
            result_at(&ev, 18),
            ("call_e27d3a940098c1bae7a9a6992762efd4", ToolOutcome::Ok)
        );
        assert_eq!(
            result_at(&ev, 25),
            ("call_fbac3c11cc883bdde9b8afac105c51f8", ToolOutcome::Ok)
        );
        assert_eq!(
            result_at(&ev, 33),
            (
                "call_4bc1b4412f564fa9414b046a5a7681a0",
                ToolOutcome::Started
            )
        );

        // 61·73 의 리터럴은 CommandExecution item 의 명령(래퍼 셋째 칸)과 같아야 한다
        let lines: Vec<Value> = std::fs::read_to_string(fixture_file("codex-app"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for (call_line, item_line) in [(61u64, 63usize), (73, 75)] {
            let cmd = lines[item_line - 1]
                .pointer("/payload/item/command/2")
                .and_then(Value::as_str)
                .unwrap();
            assert_eq!(
                call_at(&ev, call_line).2,
                &summary_of(cmd, Compound::Mixed),
                "{call_line}"
            );
        }

        // 남은 Unknown 은 전부 write_stdin 폴링이라 미대응 로그에 없다
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn ask_reject_compact_fixture() {
        let (ev, un) = fixture("ask-reject-compact");
        let want = [
            "meta None",                     // 1
            "meta Some(\"gpt-6.1-sol\")",    // 9
            "user",                          // 14
            "text",                          // 17
            "call Ask Single",               // 18 request_user_input_async
            "ask",                           // 18
            "tokens 32398 81 Some(475000)",  // 22 (21 {accepted:true} 은 접수일 뿐)
            "think",                         // 24 (25 sleep 과 28 그 출력은 버린다)
            "tokens 32497 94 Some(475000)",  // 29
            "result Ok",                     // 30 질문 답. UserMessage 아님
            "think",                         // 33
            "text",                          // 35
            "call Shell Single",             // 36 require_escalated
            "result Rejected",               // 38
            "tokens 32696 239 Some(475000)", // 39
            "think",                         // 41
            "text",                          // 43
            "tokens 32980 345 Some(475000)", // 45
            "compact",                       // 53 (52 의 0 토큰은 내지 않는다)
            "user",                          // 61
            "text",                          // 64
            "tokens 32980 9 Some(475000)",   // 66
        ];
        assert_eq!(tags(&ev), want);
        assert!(un.is_empty(), "{un:?}");

        assert_eq!(call_at(&ev, 18).0, "call_b43f5c204d994ea0f065b2859aa1780b");
        assert_eq!(
            result_at(&ev, 30),
            ("call_b43f5c204d994ea0f065b2859aa1780b", ToolOutcome::Ok)
        );
        let (id, tool, summary) = call_at(&ev, 36);
        assert_eq!(id, "call_2d7f9633e1f8e2109f02d1736c053d36");
        assert_eq!(tool, &ToolKind::Shell);
        // 승인 창을 요청한 호출이라 escalated 만 다르다
        assert_eq!(
            summary,
            &ToolSummary {
                escalated: true,
                ..summary_of(r"prog1 arg2 arg3 arg4 'C:\file2.txt'", Compound::Single)
            }
        );
        assert_eq!(
            result_at(&ev, 38),
            (
                "call_2d7f9633e1f8e2109f02d1736c053d36",
                ToolOutcome::Rejected
            )
        );
        assert!(ev.iter().any(|e| e.raw_ref.line_no == 61
            && e.kind
                == EventKind::UserMessage {
                    text: "두갸고\n".into()
                }));
        assert!(ev.iter().any(|e| e.raw_ref.line_no == 53
            && e.kind
                == EventKind::ContextCompaction {
                    pre_tokens: None,
                    post_tokens: None
                }));
    }

    #[test]
    fn aborted_fixture() {
        let (ev, un) = fixture("aborted");
        // 18 info 없는 token_count 는 버린다. 20 turn_aborted 때 남은 호출이 없다
        assert_eq!(
            tags(&ev),
            ["meta None", "meta Some(\"gpt-6.1-sol\")", "user", "text"]
        );
        assert!(un.is_empty(), "{un:?}");
    }

    /// 누적 사용량이 그대로인 token_count 를 다시 쓰면 같은 사용량을 두 번 세지 않는다.
    /// 입력에서 캐시 몫은 따로 알려 준다
    #[test]
    fn repeated_token_count_is_counted_once() {
        let count = |total_in: u64, last_in: u64| {
            rec(
                "event_msg",
                json!({"type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": total_in, "cached_input_tokens": total_in / 2, "output_tokens": 10},
                    "last_token_usage": {"input_tokens": last_in, "cached_input_tokens": last_in / 2, "output_tokens": 5},
                    "model_context_window": 1000}}),
            )
        };
        let (kinds, _) = feed(&[count(100, 100), count(100, 100), count(250, 150)]);
        let usage: Vec<(u64, u64)> = kinds
            .iter()
            .filter_map(|k| match k {
                EventKind::TokenUsage {
                    input_total,
                    cache_read,
                    ..
                } => Some((*input_total, *cache_read)),
                _ => None,
            })
            .collect();
        assert_eq!(usage, [(100, 50), (150, 75)]);
    }

    #[test]
    fn product_from_originator() {
        let line = |originator: &str| {
            rec(
                "session_meta",
                json!({"id": SID, "cwd": "/w", "originator": originator,
                       "git": {"branch": "feat-x", "repository_url": "u"}}),
            )
        };
        let (ev, un) = feed_events(&mut CodexParser::new(SID), &[line("codex_exec")]);
        assert_eq!(
            ev[0].kind,
            EventKind::SessionMeta {
                cwd: PathBuf::from("/w"),
                model: None,
                title: None,
                product: Product::CodexCli,
                git_branch: Some("feat-x".into()),
            }
        );
        assert_eq!(
            un,
            [Unhandled::ProductValue {
                value: "codex_exec".into()
            }]
        );
        let (ev, un) = feed_events(&mut CodexParser::new(SID), &[line("codex-tui")]);
        assert!(matches!(
            ev[0].kind,
            EventKind::SessionMeta {
                product: Product::CodexCli,
                ..
            }
        ));
        assert!(un.is_empty());
    }

    #[test]
    fn model_changes_only_when_different() {
        let tc = |m: &str| rec("turn_context", json!({"turn_id": "t", "model": m}));
        let (ev, _) = feed_events(
            &mut CodexParser::new(SID),
            &[meta(), tc("m1"), tc("m1"), tc("m2")],
        );
        assert_eq!(
            tags(&ev),
            ["meta None", "meta Some(\"m1\")", "meta Some(\"m2\")"]
        );
    }

    #[test]
    fn unknown_types_go_to_unhandled_known_ones_are_silent() {
        let (kinds, un) = feed(&[
            rec("response_item", json!({"type": "ghost_snapshot", "x": 1})),
            rec("response_item", json!({"type": "tool_search_call", "q": 1})),
            rec("event_msg", json!({"type": "mystery", "a": 1})),
            json!({"timestamp": "2026-01-01T00:00:00Z", "type": "novel", "foo": 1}).to_string(),
            rec("world_state", json!({})),
            rec("token_usage_record", json!({})),
            rec("compacted", json!({"message": ""})),
            rec("inter_agent_communication_metadata", json!({})),
            rec("response_item", json!({"type": "additional_tools"})),
            rec(
                "response_item",
                json!({"type": "agent_message", "author": "a"}),
            ),
            rec("event_msg", json!({"type": "task_started"})),
            rec("event_msg", json!({"type": "task_complete"})),
            rec("event_msg", json!({"type": "thread_settings_applied"})),
            "not json".into(),
            "[1,2]".into(),
        ]);
        assert!(kinds.is_empty(), "{kinds:?}");
        let rt = |t: &str, k: &[&str]| Unhandled::RecordType {
            type_name: t.into(),
            keys: strings(k),
            renamed_turn: false,
        };
        assert_eq!(
            un,
            [
                rt("ghost_snapshot", &["type", "x"]),
                rt("tool_search_call", &["q", "type"]),
                rt("mystery", &["a", "type"]),
                rt("novel", &["foo", "timestamp", "type"]),
            ]
        );
    }

    #[test]
    fn exec_without_command_literal() {
        let kind = |js: &str| {
            let (kinds, _) = feed(&[exec("c1", js)]);
            let (tool, summary) = only_call(&kinds);
            (tool.clone(), summary.path.clone())
        };
        assert_eq!(
            kind(
                "await tools.apply_patch(\"*** Begin Patch\\n*** Add File: src/new.rs\\n+x\\n*** End Patch\")"
            ),
            (ToolKind::FileWrite, Some("src/new.rs".into()))
        );
        assert_eq!(
            kind(
                "await tools.apply_patch(`*** Begin Patch\n*** Update File: src/a.rs\n@@\n-a\n+b\n*** End Patch`)"
            ),
            (ToolKind::FileEdit, Some("src/a.rs".into()))
        );
        assert_eq!(
            kind("text(await tools.web__run({search_query:[{q:\"x\"}]}))"),
            (ToolKind::WebSearch, None)
        );
        // 변수, `${}` 템플릿, 주석 속 리터럴은 뽑지 않는다
        assert_eq!(
            kind("const c = 'git push'; text(await tools.exec_command({cmd: c}))"),
            (ToolKind::Script, None)
        );
        assert_eq!(
            kind("text(await tools.exec_command({cmd: `git push ${b}`}))"),
            (ToolKind::Script, None)
        );
        assert_eq!(
            kind("// cmd:\"git push\"\ntext(1)"),
            (ToolKind::Script, None)
        );
        assert_eq!(
            kind("await tools.write_stdin({session_id:1,chars:\"\"})"),
            (ToolKind::Script, None)
        );
        // apply_patch 와 웹 검색이 같이 있으면 어느 쪽도 "만" 이 아니다
        assert_eq!(
            kind("await tools.apply_patch(\"*** Add File: a\"); await tools.web__run({})"),
            (ToolKind::Script, None)
        );
    }

    #[test]
    fn exec_command_literals_are_classified() {
        let call = |js: &str| {
            let (kinds, _) = feed(&[exec("c1", js)]);
            let (tool, summary) = only_call(&kinds);
            (tool.clone(), summary.clone())
        };
        let (tool, summary) = call("text(await tools.exec_command({cmd:\"git push\"}))");
        assert_eq!(tool, ToolKind::GitPush);
        assert_eq!(summary, summary_of("git push", Compound::Single));

        // 둘 이상이면 우선순위 최상이 대표, compound 는 Mixed
        let (tool, summary) = call(
            "await Promise.allSettled([tools.exec_command({cmd:\"cat a.txt\"}), \
             tools.exec_command({\"cmd\":'cargo test'})])",
        );
        assert_eq!(tool, ToolKind::Test);
        assert_eq!(summary, summary_of("cargo test", Compound::Mixed));

        let (tool, summary) = call("tools.exec_command({cmd:\"cat a.txt\"})");
        assert_eq!(tool, ToolKind::FileRead);
        assert_eq!(summary.path.as_deref(), Some("a.txt"));

        // JS 이스케이프를 벗긴다
        let (_, summary) =
            call(r#"tools.exec_command({cmd:"echo \u0041\x42 \"q\" \\ \u{1F600}"})"#);
        assert_eq!(
            summary.command.as_deref(),
            Some("echo AB \"q\" \\ \u{1F600}")
        );
    }

    #[test]
    fn exec_outcome_from_command_items() {
        let js = "text(await tools.exec_command({cmd:\"cargo test\"}))";
        let (k, un) = feed(&[
            exec("c1", js),
            cmd_item("completed", Some(0), "1"),
            cmd_item("failed", Some(101), "2"),
            exec_out("c1", &["Script completed\n"]),
            exec("c2", js),
            cmd_item("completed", Some(0), "3"),
            cmd_item("completed", Some(0), "4"),
            exec_out("c2", &["Script completed\n"]),
            exec("c3", js),
            exec_out("c3", &["Script failed\n"]),
            exec("c4", js),
            exec_out("c4", &["Script completed\n", "{\"exit_code\":0}"]),
            exec("c5", "tools.exec_command({cmd:\"git push &\"})"),
            cmd_item("completed", Some(0), "5"),
            exec_out("c5", &["Script completed\n"]),
        ]);
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Err]);
        assert_eq!(outcome_of(&k, "c2"), [ToolOutcome::Ok]);
        assert_eq!(outcome_of(&k, "c3"), [ToolOutcome::Err]);
        // 모델이 찍은 exit_code 는 구조 필드가 아니다
        assert_eq!(outcome_of(&k, "c4"), [ToolOutcome::Unknown]);
        // 단독 `&` 는 셸이 즉시 0을 돌려준다
        assert_eq!(outcome_of(&k, "c5"), [ToolOutcome::Unknown]);
        assert_eq!(
            un,
            [
                Unhandled::OutcomeUnknown {
                    tool: "exec".into(),
                    result_keys: strings(&["exit_code"]),
                },
                Unhandled::OutcomeUnknown {
                    tool: "exec".into(),
                    result_keys: vec![],
                },
            ]
        );
    }

    #[test]
    fn background_command_finishes_later_by_process_id() {
        let (k, un) = feed(&[
            exec("bg", "tools.exec_command({cmd:\"npm run dev\"})"),
            exec_out(
                "bg",
                &[
                    "Script completed\n",
                    "{\"session_id\":4242,\"output\":\"\"}",
                ],
            ),
            exec("c2", "tools.exec_command({cmd:\"cat a\"})"),
            // 백그라운드 작업의 item 은 열린 exec 의 것이 아니다
            cmd_item("failed", Some(1), "4242"),
            cmd_item("completed", Some(0), "7"),
            exec_out("c2", &["Script completed\n"]),
        ]);
        let results: Vec<(String, ToolOutcome)> = k
            .iter()
            .filter_map(|k| match k {
                EventKind::ToolResult {
                    call_id, outcome, ..
                } => Some((call_id.clone(), *outcome)),
                _ => None,
            })
            .collect();
        assert_eq!(
            results,
            [
                ("bg".into(), ToolOutcome::Started),
                ("bg".into(), ToolOutcome::Err),
                ("c2".into(), ToolOutcome::Ok),
            ]
        );
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn write_stdin_ctrl_c_stops_background_like_task_stop() {
        let started = [
            exec("bg", "tools.exec_command({cmd:\"npm run dev\"})"),
            exec_out(
                "bg",
                &[
                    "Script completed\n",
                    "{\"session_id\":4242,\"output\":\"\"}",
                ],
            ),
        ];
        let stop = r#"text(await tools.write_stdin({session_id:4242,chars:"\u0003"}))"#;
        let mut lines = started.to_vec();
        lines.extend([
            exec("stop", stop),
            // 중단으로 죽은 프로세스의 item 은 사고로 세지 않는다
            cmd_item("failed", Some(130), "4242"),
            exec_out("stop", &["Script completed\n"]),
            cmd_item("failed", Some(130), "4242"),
        ]);
        let (k, _) = feed(&lines);
        assert_eq!(
            outcome_of(&k, "bg"),
            [ToolOutcome::Started, ToolOutcome::Unknown]
        );
        assert_eq!(outcome_of(&k, "stop"), [ToolOutcome::Unknown]);

        // 중단이 실패하면 작업표를 유지한다
        let mut lines = started.to_vec();
        lines.extend([
            exec("stop", stop),
            exec_out("stop", &["Script failed\n"]),
            cmd_item("completed", Some(0), "4242"),
        ]);
        let (k, _) = feed(&lines);
        assert_eq!(
            outcome_of(&k, "bg"),
            [ToolOutcome::Started, ToolOutcome::Ok]
        );
    }

    #[test]
    fn subagent_spawn_and_done_from_parent() {
        let act = |kind: &str| {
            rec(
                "event_msg",
                json!({"type": "item_completed", "item": {
                    "type": "SubAgentActivity", "id": "call_spawn", "kind": kind,
                    "agent_thread_id": CHILD, "agent_path": "/root/w",
                    "model": "gpt-x", "reasoning_effort": "high"}}),
            )
        };
        let (k, un) = feed(&[
            func("call_spawn", "spawn_agent", json!({"message": "enc"})),
            act("started"),
            func_out("call_spawn", "{\"task_name\":\"w\"}"),
            act("interacted"),
            func("call_wait", "wait_agent", json!({})),
            act("completed"),
            act("completed"),
        ]);
        let spawn = EventKind::SubagentSpawn {
            child_id: CHILD.into(),
            prompt: String::new(),
            model: Some("gpt-x".into()),
            description: None,
            effort: Some("high".into()),
            depth: 1,
        };
        let done = EventKind::SubagentDone {
            child_id: CHILD.into(),
        };
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            [
                "call Spawn Single".to_string(),
                tag(&spawn),
                "result Ok".into(),
                "call Other(\"wait_agent\") Single".into(),
                tag(&done),
                tag(&done),
            ]
        );
        // wait_agent 는 확인된 배관이라 tool_name 에 넣지 않는다. 접수 응답은 Ok 라 미대응도 없다
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn child_rollout_skips_copied_parent_history() {
        let msg = |ord: u64| {
            json!({"timestamp": "2026-01-01T00:00:00Z", "ordinal": ord, "type": "response_item",
                   "payload": {"type": "message", "role": "user",
                               "content": [{"type": "input_text", "text": format!("m{ord}")}]},
                   "metadata": {"user_input_order": ord}})
            .to_string()
        };
        let lines = [
            // 자기 meta 는 ordinal 0 이라 복제 구간 안이지만 채택한다
            ord_rec(
                0,
                "session_meta",
                json!({"id": CHILD, "parent_thread_id": PARENT, "forked_from_id": PARENT,
                       "thread_source": "subagent", "agent_path": "/root/w",
                       "subagent_history_start_ordinal": 4, "originator": "Codex Desktop",
                       "cwd": "/w", "source": {"subagent": {"thread_spawn": {
                           "parent_thread_id": PARENT, "depth": 1, "agent_path": "/root/w"}}}}),
            ),
            ord_rec(
                1,
                "session_meta",
                json!({"id": PARENT, "originator": "Codex Desktop", "cwd": "/other"}),
            ),
            msg(2),
            ord_rec(3, "turn_context", json!({"model": "parent-model"})),
            ord_rec(4, "turn_context", json!({"model": "child-model"})),
            msg(5),
            ord_rec(
                6,
                "event_msg",
                json!({"type": "item_completed", "item": {"type": "SubAgentActivity",
                       "kind": "started", "agent_thread_id": GRANDCHILD}}),
            ),
        ];
        let (ev, un) = feed_events(&mut CodexParser::new(CHILD), &lines);
        let kinds: Vec<EventKind> = ev.iter().map(|e| e.kind.clone()).collect();
        let meta = |model: Option<&str>| EventKind::SessionMeta {
            cwd: PathBuf::from("/w"),
            model: model.map(str::to_string),
            title: None,
            product: Product::CodexApp,
            git_branch: None,
        };
        assert_eq!(
            kinds,
            [
                meta(None),
                meta(Some("child-model")),
                EventKind::UserMessage { text: "m5".into() },
                EventKind::SubagentSpawn {
                    child_id: GRANDCHILD.into(),
                    prompt: String::new(),
                    model: None,
                    description: None,
                    effort: None,
                    depth: 2,
                },
            ]
        );
        for e in &ev {
            assert_eq!(
                (
                    e.session_id.as_str(),
                    e.agent_id.as_str(),
                    e.parent_agent.as_deref()
                ),
                (PARENT, CHILD, Some(PARENT))
            );
        }
        assert!(un.is_empty());
    }

    #[test]
    fn shapes_without_events() {
        let (k, un) = feed(&[
            // 호스트가 보낸 출력
            rec(
                "response_item",
                json!({"type": "function_call_output", "output": "x"}),
            ),
            rec("event_msg", json!({"type": "token_count", "info": null})),
            rec(
                "event_msg",
                json!({"type": "token_count", "rate_limits": {}}),
            ),
            func("call_sleep", "sleep", json!({"duration_ms": 1})),
            func_out("call_sleep", "slept"),
            func_out("call_nobody", "x"),
            exec_out("call_nobody", &["Script completed\n"]),
            // 표식 없는 user 메시지는 주입 문맥이다
            rec(
                "response_item",
                json!({"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "<environment_context/>"}]}),
            ),
            rec(
                "response_item",
                json!({"type": "custom_tool_call_output", "output": []}),
            ),
        ]);
        assert!(k.is_empty(), "{k:?}");
        assert_eq!(
            un,
            [Unhandled::OutcomeUnknown {
                tool: "exec".into(),
                result_keys: strings(&["output", "type"]),
            }]
        );
    }

    #[test]
    fn turn_aborted_closes_pending_calls() {
        let (k, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cargo build\"})"),
            func("q1", "request_user_input_async", json!({"questions": []})),
            func_out("q1", "{\"accepted\":true}"),
            rec(
                "event_msg",
                json!({"type": "turn_aborted", "reason": "interrupted"}),
            ),
            exec_out("c1", &["Script completed\n"]),
        ]);
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            [
                "call Build Single",
                "call Ask Single",
                "ask",
                "result Unknown",
                "result Unknown",
            ]
        );
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Unknown]);
        assert_eq!(outcome_of(&k, "q1"), [ToolOutcome::Unknown]);
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn question_is_answered_by_reply_message() {
        let reply = "<send_user_message_question_reply>\n[{\"questionItemId\":\"[\\\"x\\\",\\\"q1\\\",0]\",\"question\":\"Pick?\",\"answer\":\"A\"}]\n</send_user_message_question_reply>\n";
        let (k, un) = feed(&[
            func(
                "q1",
                "request_user_input",
                json!({"questions": [{"id": "i", "header": "h", "question": "Pick?",
                                      "options": [{"label": "A"}]}]}),
            ),
            func_out("q1", "{\"accepted\":true}"),
            user(reply),
            user("thanks"),
            exec(
                "c2",
                "await tools.request_user_input({questions:[{question:\"Proceed?\"}]})",
            ),
        ]);
        assert_eq!(
            k,
            [
                EventKind::ToolCall {
                    call_id: "q1".into(),
                    tool: ToolKind::Ask,
                    summary: empty_summary(),
                },
                EventKind::PermissionRequest {
                    question: "Pick?".into(),
                    plan: false,
                },
                result("q1".into(), ToolOutcome::Ok, String::new()),
                EventKind::UserMessage {
                    text: "thanks".into()
                },
                EventKind::ToolCall {
                    call_id: "c2".into(),
                    tool: ToolKind::Ask,
                    summary: empty_summary(),
                },
                EventKind::PermissionRequest {
                    question: "Proceed?".into(),
                    plan: false,
                },
            ]
        );
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn function_call_names() {
        let (k, un) = feed(&[
            func("m1", "mcp__github__create_issue", json!({"title": "t"})),
            func_out("m1", "{\"success\":false}"),
            func("o1", "update_plan", json!({"plan": [], "explanation": ""})),
            rec(
                "response_item",
                json!({"type": "web_search_call", "id": "ws1", "status": "completed",
                       "action": {"type": "search"}}),
            ),
        ]);
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            [
                "call Mcp { server: \"github\", tool: \"create_issue\" } Single",
                "result Unknown",
                "call Other(\"update_plan\") Single",
                "call WebSearch Single",
                "result Ok",
            ]
        );
        // success 는 하네스가 정한 키가 아니라 로그에 남기지 않는다
        assert_eq!(
            un,
            [
                Unhandled::OutcomeUnknown {
                    tool: "mcp__github__create_issue".into(),
                    result_keys: vec![],
                },
                Unhandled::ToolName {
                    name: "update_plan".into(),
                    input_keys: strings(&["explanation", "plan"]),
                },
            ]
        );
    }

    fn item(ty: &str, status: &str) -> String {
        rec(
            "event_msg",
            json!({"type": "item_completed", "item": {"type": ty, "id": "x", "status": status}}),
        )
    }

    fn call_kind(js: &str) -> ToolKind {
        let (kinds, _) = feed(&[exec("c1", js)]);
        only_call(&kinds).0.clone()
    }

    #[test]
    fn exec_with_only_mcp_tools_is_mcp() {
        assert_eq!(
            call_kind("await tools.mcp__github__create_issue({title: 't'})"),
            ToolKind::Mcp {
                server: "github".into(),
                tool: "create_issue".into()
            }
        );
        assert_eq!(
            call_kind("await tools.mcp__github__search_code({q: 'x'})"),
            ToolKind::WebSearch
        );
        assert_eq!(
            call_kind("await tools.mcp__browser__fetch_page({url: 'x'})"),
            ToolKind::WebFetch
        );
        // UUID 서버는 도구 이름의 첫 토큰이 상호다
        assert_eq!(
            call_kind("await tools.mcp__04586901_4c2c_4a3b_8d2e_0123456789ab__asset_add_file({})"),
            ToolKind::Mcp {
                server: "asset".into(),
                tool: "asset_add_file".into()
            }
        );
        // 다른 도구와 섞이면 "만" 이 아니다
        assert_eq!(
            call_kind("await tools.mcp__a__b({}); await tools.web__run({})"),
            ToolKind::Script
        );
    }

    #[test]
    fn items_attach_only_to_exec_that_called_their_kind() {
        let (k, _) = feed(&[
            exec("patch", "await tools.apply_patch('*** Add File: a')"),
            item("FileChange", "failed"),
            exec_out("patch", &["Script completed\n"]),
            exec("mcp", "await tools.mcp__s__t({})"),
            item("McpToolCall", "completed"),
            exec_out("mcp", &["Script completed\n"]),
            exec("mcp2", "await tools.mcp__s__t({})"),
            item("McpToolCall", "failed"),
            exec_out("mcp2", &["Script completed\n"]),
            // 무관한 종류의 item 은 이 호출의 것이 아니다
            exec("sh", "tools.exec_command({cmd:\"cat a\"})"),
            item("FileChange", "failed"),
            item("McpToolCall", "failed"),
            exec_out("sh", &["Script completed\n"]),
            exec("web", "tools.web__run({})"),
            cmd_item("failed", Some(1), "9"),
            exec_out("web", &["Script completed\n"]),
        ]);
        assert_eq!(outcome_of(&k, "patch"), [ToolOutcome::Err]);
        assert_eq!(outcome_of(&k, "mcp"), [ToolOutcome::Ok]);
        assert_eq!(outcome_of(&k, "mcp2"), [ToolOutcome::Err]);
        assert_eq!(outcome_of(&k, "sh"), [ToolOutcome::Unknown]);
        assert_eq!(outcome_of(&k, "web"), [ToolOutcome::Unknown]);
    }

    #[test]
    fn rejected_phrase_counts_only_for_failed_script_without_items() {
        let phrase =
            r#"exec_command failed: CreateProcess { message: "Rejected(\"rejected by user\")" }"#;
        let js = "tools.exec_command({cmd:\"cat a\"})";
        let (k, _) = feed(&[
            // 성공한 호출이 text() 로 그 문구를 찍었다
            exec("ok", js),
            cmd_item("completed", Some(0), "1"),
            exec_out("ok", &["Script completed\n", phrase]),
            exec("rej", js),
            exec_out("rej", &["Script failed\n", phrase]),
        ]);
        assert_eq!(outcome_of(&k, "ok"), [ToolOutcome::Ok]);
        assert_eq!(outcome_of(&k, "rej"), [ToolOutcome::Rejected]);
    }

    #[test]
    fn session_id_is_found_in_any_object_and_polls_do_not_restart_it() {
        let (k, _) = feed(&[
            exec("bg", "tools.exec_command({cmd:\"npm run dev\"})"),
            exec_out(
                "bg",
                &[
                    "Script completed\n",
                    "{\"note\":1}",
                    "{\"session_id\":5,\"output\":\"\"}",
                ],
            ),
            // 이미 백그라운드인 세션의 폴링 응답
            exec("poll", "tools.write_stdin({session_id:5,chars:\"\"})"),
            exec_out(
                "poll",
                &["Script completed\n", "{\"session_id\":5,\"output\":\"\"}"],
            ),
            cmd_item("completed", Some(0), "5"),
        ]);
        assert_eq!(
            outcome_of(&k, "bg"),
            [ToolOutcome::Started, ToolOutcome::Ok]
        );
        assert_eq!(outcome_of(&k, "poll"), [ToolOutcome::Unknown]);
    }

    #[test]
    fn unknown_log_keeps_only_harness_keys() {
        let (_, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cat a\"})"),
            exec_out(
                "c1",
                &["Script completed\n", "{\"/home/me/x\":1,\"output\":\"\"}"],
            ),
            func("f1", "mystery", json!({})),
            func_out("f1", "{\"/home/me/y\":1,\"session_id\":1}"),
        ]);
        assert_eq!(
            un,
            [
                Unhandled::OutcomeUnknown {
                    tool: "exec".into(),
                    result_keys: strings(&["output"]),
                },
                Unhandled::ToolName {
                    name: "mystery".into(),
                    input_keys: vec![],
                },
                Unhandled::OutcomeUnknown {
                    tool: "mystery".into(),
                    result_keys: strings(&["session_id"]),
                },
            ]
        );
    }

    #[test]
    fn polling_only_unknown_is_not_logged_and_send_message_is_silent() {
        let (k, un) = feed(&[
            exec("poll", "tools.write_stdin({session_id:5,chars:\"\"})"),
            exec_out("poll", &["Script completed\n", "{\"output\":\"\"}"]),
            func("sm", "send_message", json!({"to": "x"})),
            func_out("sm", "{\"ok\":true}"),
        ]);
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            ["call Script Single", "result Unknown"]
        );
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn web_search_call_pairs_on_its_own_line() {
        let mut p = CodexParser::new(SID);
        let mut un = Vec::new();
        let mut at = |payload: Value| {
            let r = RawRef {
                byte_offset: 42,
                ..raw(1)
            };
            p.parse_line(rec("response_item", payload).as_bytes(), &r, &mut un)
        };
        let ev = at(json!({"type": "web_search_call", "status": "failed"}));
        let ids: Vec<&str> = ev
            .iter()
            .map(|e| match &e.kind {
                EventKind::ToolCall { call_id, .. } | EventKind::ToolResult { call_id, .. } => {
                    call_id.as_str()
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(ids, ["ws-42", "ws-42"]);
        assert_eq!(tags(&ev), ["call WebSearch Single", "result Err"]);
        let ev = at(json!({"type": "web_search_call", "id": "w", "status": "in_progress"}));
        assert_eq!(tags(&ev), ["call WebSearch Single", "result Unknown"]);
    }

    #[test]
    fn internal_agent_rollouts_emit_nothing() {
        let metas = [
            json!({"id": SID, "thread_source": "guardian_review"}),
            json!({"id": SID, "thread_source": "roborev"}),
            json!({"id": SID, "source": {"subagent": "review"}}),
            json!({"id": SID, "source": {"subagent": {"other": "guardian"}}}),
        ];
        for m in metas {
            let (ev, un) = feed_events(
                &mut CodexParser::new(SID),
                &[
                    rec("session_meta", m.clone()),
                    rec("turn_context", json!({"model": "m"})),
                    user("hi"),
                    rec("event_msg", json!({"type": "mystery"})),
                ],
            );
            assert!(ev.is_empty() && un.is_empty(), "{m}: {ev:?} {un:?}");
        }
        // thread_spawn 자식은 내부 작업이 아니다
        let (ev, _) = feed_events(
            &mut CodexParser::new(SID),
            &[rec(
                "session_meta",
                json!({"id": SID, "thread_source": "subagent",
                       "source": {"subagent": {"thread_spawn": {"parent_thread_id": PARENT}}}}),
            )],
        );
        assert_eq!(ev.len(), 1);
    }

    #[test]
    fn subagent_notification_and_unknown_activity_kind_go_to_unhandled() {
        let (k, un) = feed(&[
            user(
                "<subagent_notification>\n{\"agent_path\":\"/root/w\",\"status\":\"completed\"}\n</subagent_notification>",
            ),
            user("<subagent_notification>not json</subagent_notification>"),
            rec(
                "event_msg",
                json!({"type": "item_completed", "item": {"type": "SubAgentActivity",
                       "kind": "paused", "agent_thread_id": CHILD}}),
            ),
        ]);
        assert!(k.is_empty(), "{k:?}");
        let rt = |name: &str, k: &[&str]| Unhandled::RecordType {
            type_name: name.into(),
            keys: strings(k),
            renamed_turn: false,
        };
        assert_eq!(
            un,
            [
                rt("subagent_notification", &["agent_path", "status"]),
                rt("subagent_notification", &[]),
                rt(
                    "SubAgentActivity.paused",
                    &["agent_thread_id", "kind", "type"]
                ),
            ]
        );
    }

    #[test]
    fn cmd_literal_must_be_an_object_key_with_a_plain_value() {
        assert_eq!(
            call_kind("text(await tools.exec_command({cmd: x ? \"cmd\" : \"rm -rf build\"}))"),
            ToolKind::Script
        );
        assert_eq!(
            call_kind("tools.exec_command({cmd: \"git \" + sub})"),
            ToolKind::Script
        );
        assert_eq!(
            call_kind("tools.exec_command({tty: true, cmd: \"git push\"})"),
            ToolKind::GitPush
        );
    }

    #[test]
    fn write_stdin_args_stop_at_the_closing_paren() {
        let s = scan_exec(
            "await tools.write_stdin({session_id:4242}); const o = {session_id: 9, chars: \"\\u0003\"};",
        );
        assert_eq!(s.stdin, [(Some("4242".into()), None)]);
    }

    #[test]
    fn held_item_closes_background_when_stop_fails() {
        let (k, _) = feed(&[
            exec("bg", "tools.exec_command({cmd:\"npm run dev\"})"),
            exec_out(
                "bg",
                &[
                    "Script completed\n",
                    "{\"session_id\":4242,\"output\":\"\"}",
                ],
            ),
            exec(
                "stop",
                r#"text(await tools.write_stdin({session_id:4242,chars:"\u0003"}))"#,
            ),
            cmd_item("failed", Some(130), "4242"),
            exec_out("stop", &["Script failed\n"]),
        ]);
        assert_eq!(
            outcome_of(&k, "bg"),
            [ToolOutcome::Started, ToolOutcome::Err]
        );
    }

    #[test]
    fn regex_literal_quotes_do_not_swallow_the_command() {
        assert_eq!(
            call_kind("x.replace(/`/g,''); tools.exec_command({cmd:\"git push\"})"),
            ToolKind::GitPush
        );
        assert_eq!(
            call_kind("x.replace(/[/'\"]\\//g,''); tools.exec_command({cmd:\"git push\"})"),
            ToolKind::GitPush
        );
        // 나눗셈은 정규식이 아니다
        assert_eq!(
            call_kind("const h = (a + b) / 2 / 3; tools.exec_command({cmd:\"git push\"})"),
            ToolKind::GitPush
        );
    }

    #[test]
    fn surrogate_pair_escape_is_one_char() {
        let (kinds, _) = feed(&[exec(
            "c1",
            r#"tools.exec_command({cmd:"echo \uD83D\uDE00"})"#,
        )]);
        let (_, summary) = only_call(&kinds);
        assert_eq!(summary.command.as_deref(), Some("echo \u{1F600}"));
    }

    #[test]
    fn two_exec_commands_are_mixed_even_if_one_is_not_a_literal() {
        let (kinds, _) = feed(&[exec(
            "c1",
            "await Promise.allSettled([tools.exec_command({cmd:\"git push\"}), tools.exec_command({cmd: c})])",
        )]);
        let (tool, summary) = only_call(&kinds);
        assert_eq!(tool, &ToolKind::GitPush);
        assert_eq!(summary.compound, Compound::Mixed);
    }

    #[test]
    fn test_exec_result_fills_test_result() {
        let (k, _) = feed(&[
            exec("t", "tools.exec_command({cmd:\"cargo test\"})"),
            cmd_item("failed", Some(101), "1"),
            exec_out(
                "t",
                &[
                    "Script completed
",
                    "test result: FAILED. 3 passed; 1 failed; 0 ignored
",
                ],
            ),
        ]);
        let got = k.iter().find_map(|e| match e {
            EventKind::ToolResult { test_result, .. } => *test_result,
            _ => None,
        });
        assert_eq!(
            got,
            Some(crate::parse::TestResult {
                passed: 3,
                failed: 1
            })
        );
    }

    #[test]
    fn require_escalated_marks_summary() {
        let (k, _) = feed(&[
            exec(
                "e",
                "tools.exec_command({cmd:\"git push\", sandbox_permissions:\"require_escalated\"})",
            ),
            exec("n", "tools.exec_command({cmd:\"git push\"})"),
            func(
                "f",
                "shell",
                json!({"command": ["ls"], "sandbox_permissions": "require_escalated"}),
            ),
        ]);
        let esc = |id: &str| {
            k.iter()
                .find_map(|e| match e {
                    EventKind::ToolCall {
                        call_id, summary, ..
                    } if call_id == id => Some(summary.escalated),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!((esc("e"), esc("n"), esc("f")), (true, false, true));
    }

    #[test]
    fn commit_message_is_carried_to_summary() {
        let (k, _) = feed(&[exec(
            "c",
            "tools.exec_command({cmd:\"git commit -m \\\"fix: x\\\"\"})",
        )]);
        let (tool, summary) = only_call(&k);
        assert_eq!(tool, &ToolKind::GitCommit);
        assert_eq!(summary.commit_message.as_deref(), Some("fix: x"));
    }
}
