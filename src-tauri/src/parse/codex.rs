//! Codex 기록 파서 (docs/CONTRACTS.md C7)

mod script;

use script::{ExecScan, scan_exec};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    Classified, Compound, Confidence, Event, EventKind, Parser, RawRef, ToolKind, ToolOutcome,
    ToolSummary, Unhandled, classify_command, mcp_kind, parse_test_result, rank,
};
use crate::discover::Product;

/// 승인 창 거부 때 하네스가 쓰는 문구. 결과 text 안에서는 따옴표가 `\"` 로 이스케이프돼 있다
const REJECTED: &str = r#"Rejected(\"rejected by user\")"#;
const QUESTION_REPLY: &str = "<send_user_message_question_reply>";
/// exec 가 끝나기 전에 돌아왔다는 첫 text. 결과는 같은 cell 의 `wait` 응답으로 온다
const CELL_RUNNING: &str = "Script running with cell ID ";
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
    /// 묶음이 끝나기 전 중복 item 이 다른 호출에 붙지 않도록 종료한 세션도 남긴다.
    background: HashMap<String, BackgroundSession>,
    background_calls: HashMap<String, BackgroundCall>,
    /// 이벤트를 내지 않는 호출(`sleep`, `send_message`). 뒤따르는 출력도 버린다
    ignored: HashSet<String>,
    /// 성공한 followup_task의 Interacted만 기존 직원 재개로 쓴다.
    followups: HashSet<String>,
    known_agents: HashSet<String>,
    /// 실행 중인 exec 를 기다리는 `wait` 의 call_id → 그 exec 의 call_id. 이벤트 없이 결과만 원래 exec 로 넘긴다
    cell_waits: HashMap<String, String>,
    /// 내부 에이전트의 rollout. 파일 전체에서 이벤트를 내지 않는다
    silent: bool,
    /// 마지막 token_count 의 누적 사용량 (입력, 캐시, 출력). 누적이 그대로인 기록은 같은 사용량을 다시 쓴 것이다
    last_total: Option<(u64, u64, u64)>,
}

struct Meta {
    cwd: PathBuf,
    product: Product,
    product_observed: bool,
    git_branch: Option<String>,
}

struct Pending {
    call_id: String,
    /// `OutcomeUnknown.tool`
    tool: String,
    wait: Wait,
}

struct BackgroundSession {
    call_id: String,
    done: bool,
}

struct BackgroundCall {
    remaining: usize,
    outcome: ToolOutcome,
}

enum Wait {
    Exec(Box<ExecWait>),
    /// `ask` 면 `{accepted: true}` 를 받은 뒤에도 질문 답을 기다린다
    Function {
        ask: bool,
    },
}

#[derive(Default)]
struct ExecWait {
    /// 이 호출이 부른 `tools.<이름>`. item 은 맞는 종류를 부른 exec 에만 붙는다
    tools: Vec<String>,
    /// 단독 `&` 로 띄운 명령. 셸이 즉시 0을 돌려준다 (docs/CONTRACTS.md C7)
    async_unknown: bool,
    /// Test 로 분류된 호출. 결과 본문에서 도장 숫자를 뽑는다
    test: bool,
    /// 중단 실패 때 원래 결과로 닫기 위해 해당 세션의 item 을 보관한다.
    stop_sessions: Vec<(String, Option<Value>)>,
    /// `Script running with cell ID` 로 돌아온 cell. 결과는 이 cell 의 `wait` 가 받는다
    cell: Option<String>,
    /// exec 출력보다 먼저 끝난 명령을 뒤늦게 백그라운드로 등록하지 않는다.
    commands: HashSet<String>,
    /// 중간 출력의 연결 근거만 보관한다. bool은 종료 코드를 확인했는지다.
    output_sessions: HashMap<String, bool>,
    output_keys: HashSet<String>,
    output_rejected: bool,
    output_failed: bool,
    output_overflow: bool,
    items: u32,
    completed: u32,
    failed: u32,
}

impl ExecWait {
    fn remember_output(&mut self, texts: &[String]) {
        self.output_rejected |= texts.iter().any(|t| t.contains(REJECTED));
        for (index, text) in texts.iter().enumerate() {
            let text = if index == 0 {
                if !(text.starts_with(CELL_RUNNING)
                    || text.starts_with("Script completed")
                    || text.starts_with("Script failed"))
                {
                    continue;
                }
                let Some((_, body)) = text
                    .split_once("\nOutput:\n")
                    .or_else(|| text.split_once("\r\nOutput:\r\n"))
                else {
                    continue;
                };
                body
            } else {
                text.as_str()
            };
            let Ok(Value::Object(obj)) = serde_json::from_str(text) else {
                continue;
            };
            self.output_keys
                .extend(obj.keys().filter(|k| is_known_key(k)).cloned());
            let exit = obj.get("exit_code").and_then(Value::as_i64);
            self.output_failed |= self
                .calls(|tool| tool == "exec_command" || tool == "write_stdin")
                && exit.is_some_and(|code| code != 0);
            let Some(id) = obj.get("session_id").and_then(id_string) else {
                continue;
            };
            // ponytail: 호출당 ID 256개·각 128바이트까지만 보존, 더 큰 실제 호출이 확인되면 상한을 올린다.
            if id.len() > 128
                || (self.output_sessions.len() >= 256 && !self.output_sessions.contains_key(&id))
            {
                self.output_overflow = true;
                continue;
            }
            *self.output_sessions.entry(id).or_default() |= exit.is_some();
        }
    }

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
            background_calls: HashMap::new(),
            ignored: HashSet::new(),
            followups: HashSet::new(),
            known_agents: HashSet::new(),
            cell_waits: HashMap::new(),
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
        let originator = str_at(p, &["originator"]);
        let product = match originator {
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
            product_observed: originator.is_some(),
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
            product_observed: m.product_observed,
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
                wait: Wait::Exec(Box::default()),
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
        let stop_sessions = scan
            .stdin
            .iter()
            .filter(|(s, chars)| {
                chars.as_deref() == Some("\u{3}")
                    && s.as_ref()
                        .is_some_and(|s| self.background.get(s).is_some_and(|b| !b.done))
            })
            .filter_map(|(s, _)| s.clone())
            .map(|s| (s, None))
            .collect();
        self.pending.push(Pending {
            call_id,
            tool: "exec".into(),
            wait: Wait::Exec(Box::new(ExecWait {
                tools: scan.tools,
                async_unknown,
                test,
                stop_sessions,
                ..ExecWait::default()
            })),
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
        let texts = output_texts(output);
        if let Wait::Exec(w) = &mut self.pending[i].wait {
            w.remember_output(&texts);
        }
        // 시작됨은 완료가 아니다 (docs/CONTRACTS.md C8). 호출을 남겨 두고 cell 의 wait 응답을 기다린다
        if let Some(cell) = texts
            .first()
            .and_then(|t| t.strip_prefix(CELL_RUNNING))
            .and_then(|t| t.split_whitespace().next())
            && let Wait::Exec(w) = &mut self.pending[i].wait
        {
            w.cell = Some(cell.to_string());
            return;
        }
        let p = self.pending.remove(i);
        self.exec_close(p, texts, out, un);
    }

    /// exec 의 최종 결과. 바로 온 출력과 cell 의 wait 응답이 같이 쓴다
    fn exec_close(
        &mut self,
        p: Pending,
        texts: Vec<String>,
        out: &mut Vec<EventKind>,
        un: &mut Vec<Unhandled>,
    ) {
        let Wait::Exec(w) = p.wait else {
            unreachable!("부르는 쪽이 Exec 만 넘긴다")
        };
        let first = texts.first().map(String::as_str).unwrap_or_default();
        // 이미 백그라운드인 세션의 폴링 응답은 새 작업이 아니다. 원래 호출의 짝을 덮어쓰면 안 된다
        let started: HashSet<String> = w
            .output_sessions
            .iter()
            .filter_map(|(id, finished)| (!finished).then_some(id))
            .filter(|s| {
                w.calls(|t| t == "exec_command")
                    && !self.background.contains_key(*s)
                    && !w.commands.contains(*s)
            })
            .cloned()
            .collect();
        // 거부 문구는 item 없이 `Script failed` 로 끝난 호출에서만 본다. 성공한 호출이 text() 로 찍은 값에도 있을 수 있다
        let rejected = w.items == 0 && first.starts_with("Script failed") && w.output_rejected;
        let mut outcome = if !started.is_empty() {
            ToolOutcome::Started
        } else if rejected {
            ToolOutcome::Rejected
        } else if w.failed > 0 || w.output_failed {
            ToolOutcome::Err
        } else if w.items > 0 && w.completed == w.items {
            ToolOutcome::Ok
        } else if first.starts_with("Script failed") {
            ToolOutcome::Err
        } else {
            ToolOutcome::Unknown
        };
        if outcome == ToolOutcome::Ok && (w.async_unknown || w.output_overflow) {
            outcome = ToolOutcome::Unknown;
        }
        if outcome == ToolOutcome::Started {
            self.background_calls.insert(
                p.call_id.clone(),
                BackgroundCall {
                    remaining: started.len(),
                    outcome: if rejected {
                        ToolOutcome::Rejected
                    } else if w.failed > 0 || w.output_failed || first.starts_with("Script failed")
                    {
                        ToolOutcome::Err
                    } else if w.async_unknown || w.output_overflow || w.completed < w.items {
                        ToolOutcome::Unknown
                    } else {
                        ToolOutcome::Ok
                    },
                },
            );
            for s in started {
                self.background.insert(
                    s,
                    BackgroundSession {
                        call_id: p.call_id.clone(),
                        done: false,
                    },
                );
            }
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
        if (outcome == ToolOutcome::Unknown && !w.polling_only()) || w.output_overflow {
            let mut result_keys: Vec<String> = w.output_keys.iter().cloned().collect();
            result_keys.sort();
            un.push(Unhandled::OutcomeUnknown {
                tool: p.tool,
                result_keys,
            });
        }
        // TaskStop 과 같게: 중단이 실패하지 않았을 때만 작업표를 뗀다
        for (s, held) in w.stop_sessions {
            if !self.background.get(&s).is_some_and(|b| !b.done) {
                continue;
            }
            if matches!(outcome, ToolOutcome::Err | ToolOutcome::Rejected) {
                if let Some(item) = held {
                    self.background_item(&s, &item, out, un);
                }
            } else {
                self.finish_background(&s, ToolOutcome::Unknown, out);
            }
        }
    }

    fn function_call(&mut self, p: &Value, out: &mut Vec<EventKind>, un: &mut Vec<Unhandled>) {
        let call_id = str_at(p, &["call_id"]).unwrap_or_default().to_string();
        let name = str_at(p, &["name"]).unwrap_or_default();
        let args = parse_maybe_json(p.get("arguments"));
        if name == "wait"
            && let Some(cell) = args.get("cell_id").and_then(id_string)
            && let Some(exec) = self.pending.iter().rev().find(
                |p| matches!(&p.wait, Wait::Exec(w) if w.cell.as_deref() == Some(cell.as_str())),
            )
        {
            self.cell_waits.insert(call_id, exec.call_id.clone());
            return;
        }
        let mut ask = false;
        let tool = match name {
            // `sleep` 은 질문 답을 기다리며 부르고 `send_message` 는 에이전트 사이 배관이다. 연출할 게 없다
            "followup_task" => {
                self.followups.insert(call_id.clone());
                self.ignored.insert(call_id);
                return;
            }
            "sleep" | "send_message" | "wait_agent" | "interrupt_agent" => {
                self.ignored.insert(call_id);
                return;
            }
            "request_user_input_async" | "request_user_input" | "request_permissions" => {
                ask = true;
                ToolKind::Ask
            }
            "spawn_agent" => ToolKind::Spawn,
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
            self.followups.remove(call_id);
            return;
        }
        if let Some(exec) = self.cell_waits.remove(call_id) {
            self.exec_output(&exec, output, out, un);
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
            "spawn_agent" => ToolOutcome::Ok,
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
                    Some("SubAgentActivity") => subagent_activity(
                        item,
                        self.depth,
                        &mut self.followups,
                        &mut self.known_agents,
                        out,
                        un,
                    ),
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
                // 압축 직후의 0은 사용량이 아니다 (docs/CONTRACTS.md C7)
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
            "sub_agent_activity" => subagent_activity(
                p,
                self.depth,
                &mut self.followups,
                &mut self.known_agents,
                out,
                un,
            ),
            "collab_agent_spawn_end" => {
                if let Some(child) = str_at(p, &["new_thread_id"]) {
                    self.known_agents.insert(child.to_string());
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
            && let Some(background) = self.background.get(&pid)
        {
            if background.done {
                return;
            }
            // 중단 요청 중이면 그 결과(TaskStop 규칙)로 정리한다. 중단이 실패할 때를 위해 보관한다
            let stopping = self.pending.iter_mut().find_map(|p| match &mut p.wait {
                Wait::Exec(w) => w
                    .stop_sessions
                    .iter_mut()
                    .find_map(|(id, held)| (id == &pid).then_some(held)),
                _ => None,
            });
            match stopping {
                Some(held) => *held = Some(item.clone()),
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
            if ty == "CommandExecution"
                && let Some(pid) = item.get("process_id").and_then(id_string)
            {
                w.commands.insert(pid);
            }
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
        let outcome = if failed {
            ToolOutcome::Err
        } else if completed {
            ToolOutcome::Ok
        } else {
            ToolOutcome::Unknown
        };
        self.finish_background(pid, outcome, out);
        if outcome == ToolOutcome::Unknown {
            un.push(Unhandled::OutcomeUnknown {
                tool: "exec".into(),
                result_keys: keys(item),
            });
        }
    }

    fn finish_background(&mut self, pid: &str, outcome: ToolOutcome, out: &mut Vec<EventKind>) {
        let Some(session) = self.background.get_mut(pid).filter(|s| !s.done) else {
            return;
        };
        session.done = true;
        let call_id = session.call_id.clone();
        let call = self
            .background_calls
            .get_mut(&call_id)
            .expect("백그라운드 호출");
        call.remaining -= 1;
        call.outcome = match (call.outcome, outcome) {
            (ToolOutcome::Err, _) | (_, ToolOutcome::Err) => ToolOutcome::Err,
            (ToolOutcome::Rejected, _) | (_, ToolOutcome::Rejected) => ToolOutcome::Rejected,
            (ToolOutcome::Unknown, _) | (_, ToolOutcome::Unknown) => ToolOutcome::Unknown,
            _ => ToolOutcome::Ok,
        };
        if call.remaining > 0 {
            return;
        }
        let outcome = call.outcome;
        self.background_calls.remove(&call_id);
        self.background.retain(|_, s| s.call_id != call_id);
        out.push(result(call_id, outcome, String::new()));
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
            unhandled.push(Unhandled::InvalidJson);
            return Vec::new();
        };
        if self.silent || !v.is_object() {
            return Vec::new();
        }
        // 자식 rollout 앞부분은 부모 이력의 복제다. probe·상세 읽기도 같은 파서를 쓰므로 여기서 중복을 거른다.
        // 첫 session_meta 는 이 값을 알기 전에 읽혀 걸리지 않는다.
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

    /// 부모 thread 가 있는 자식이거나 내부 에이전트의 rollout 은 팀이 되지 않는다 (docs/CONTRACTS.md C7)
    fn side(&self) -> bool {
        self.silent || self.parent.is_some()
    }
}

/// `interacted` 는 메시지 전달과 재개가 같은 형식이므로 확인된 followup 호출만 재개로 낸다.
fn subagent_activity(
    item: &Value,
    depth: u8,
    followups: &mut HashSet<String>,
    known_agents: &mut HashSet<String>,
    out: &mut Vec<EventKind>,
    un: &mut Vec<Unhandled>,
) {
    let kind = str_at(item, &["kind"]).unwrap_or_default();
    match (kind, str_at(item, &["agent_thread_id"])) {
        ("started", Some(child)) => {
            known_agents.insert(child.to_string());
            out.push(spawn(
                child,
                str_at(item, &["model"]),
                str_at(item, &["reasoning_effort"]),
                depth,
            ));
        }
        ("completed", Some(child)) => out.push(EventKind::SubagentDone {
            child_id: child.to_string(),
        }),
        ("interrupted", Some(child)) => out.push(EventKind::SubagentInterrupted {
            child_id: child.to_string(),
        }),
        ("interacted", Some(child))
            if known_agents.contains(child)
                && str_at(item, &["id"]).is_some_and(|id| followups.remove(id)) =>
        {
            out.push(spawn(child, None, None, depth));
        }
        ("started" | "completed" | "interacted" | "interrupted", _) => {}
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
                product_observed: true,
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
                product_observed: true,
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
                product_observed: true,
                ..
            }
        ));
        assert!(un.is_empty());
        // originator 가 없으면 기본값이라 등록부 몫으로 남긴다
        let bare = rec("session_meta", json!({"id": SID, "cwd": "/w"}));
        let (ev, _) = feed_events(&mut CodexParser::new(SID), &[bare]);
        assert!(matches!(
            ev[0].kind,
            EventKind::SessionMeta {
                product: Product::CodexCli,
                product_observed: false,
                ..
            }
        ));
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
                Unhandled::InvalidJson,
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
    fn background_group_waits_for_every_command_and_preserves_unrelated_test() {
        for pids in [["111", "222"], ["222", "111"]] {
            for exits in [[0, 0], [0, 1], [1, 0]] {
                let mut parser = CodexParser::new(SID);
                let (mut events, un) = feed_events(
                    &mut parser,
                    &[
                        meta(),
                        exec(
                            "bg",
                            "text(await tools.exec_command({cmd:\"npm run dev\"})); text(await tools.exec_command({cmd:\"npm run dev\"}));",
                        ),
                        exec_out(
                            "bg",
                            &[
                                "Script completed\n",
                                r#"{"session_id":111}"#,
                                r#"{"session_id":222}"#,
                            ],
                        ),
                        cmd_item("completed", Some(exits[0]), pids[0]),
                    ],
                );
                assert!(un.is_empty(), "{un:?}");
                let kinds: Vec<_> = events.iter().map(|e| e.kind.clone()).collect();
                assert_eq!(outcome_of(&kinds, "bg"), [ToolOutcome::Started]);
                let local = chrono::FixedOffset::east_opt(0).unwrap();
                let team =
                    crate::office::classify::replay(SID, Product::CodexApp, &[], &events, local);
                assert_eq!(team.employees[0].background.len(), 1);

                let (tail, un) = feed_events(
                    &mut parser,
                    &[
                        exec(
                            "healthy",
                            "text(await tools.exec_command({cmd:\"cargo test\"}))",
                        ),
                        // 이미 닫힌 세션의 중복 item 도 새 호출의 결과가 아니다.
                        cmd_item("completed", Some(exits[0]), pids[0]),
                        cmd_item("completed", Some(exits[1]), pids[1]),
                        cmd_item("completed", Some(0), "333"),
                        exec_out(
                            "healthy",
                            &["Script completed\n", "test result: ok. 1 passed; 0 failed"],
                        ),
                    ],
                );
                assert!(un.is_empty(), "{un:?}");
                events.extend(tail);
                let kinds: Vec<_> = events.iter().map(|e| e.kind.clone()).collect();
                let failed = exits.contains(&1);
                assert_eq!(
                    outcome_of(&kinds, "bg"),
                    [
                        ToolOutcome::Started,
                        if failed {
                            ToolOutcome::Err
                        } else {
                            ToolOutcome::Ok
                        }
                    ]
                );
                assert_eq!(outcome_of(&kinds, "healthy"), [ToolOutcome::Ok]);
                let team =
                    crate::office::classify::replay(SID, Product::CodexApp, &[], &events, local);
                assert!(team.employees[0].background.is_empty());
                assert_eq!(team.props.test, Some(crate::office::TestVerdict::Pass));
                assert_eq!(team.employees[0].counters.errors, u32::from(failed));
            }
        }
    }

    #[test]
    fn background_group_skips_finished_output_and_keeps_earlier_failure() {
        for exit in [0, 1] {
            for first_output in [
                r#"{"session_id":111,"exit_code":0}"#,
                r#"{"session_id":111}"#,
            ] {
                let (k, un) = feed(&[
                    exec(
                        "bg",
                        "text(await tools.exec_command({cmd:\"x\"})); text(await tools.exec_command({cmd:\"y\"}));",
                    ),
                    cmd_item("completed", Some(exit), "111"),
                    exec_out(
                        "bg",
                        &["Script completed\n", first_output, r#"{"session_id":222}"#],
                    ),
                    cmd_item("completed", Some(0), "222"),
                ]);
                assert_eq!(
                    outcome_of(&k, "bg"),
                    [
                        ToolOutcome::Started,
                        if exit == 0 {
                            ToolOutcome::Ok
                        } else {
                            ToolOutcome::Err
                        }
                    ]
                );
                assert!(un.is_empty(), "{un:?}");
            }
        }
    }

    #[test]
    fn background_group_poll_and_stop_leave_other_command_running() {
        for exit in [0, 1] {
            let mut parser = CodexParser::new(SID);
            let (events, un) = feed_events(
                &mut parser,
                &[
                    exec(
                        "bg",
                        "text(await tools.exec_command({cmd:\"x\"})); text(await tools.exec_command({cmd:\"y\"}));",
                    ),
                    exec_out(
                        "bg",
                        &[
                            "Script completed\n",
                            r#"{"session_id":111}"#,
                            r#"{"session_id":222}"#,
                        ],
                    ),
                    exec("poll", "text(await tools.write_stdin({session_id:222}))"),
                    exec_out("poll", &["Script completed\n", r#"{"session_id":222}"#]),
                    exec(
                        "stop",
                        r#"text(await tools.write_stdin({session_id:111,chars:"\u0003"}))"#,
                    ),
                    cmd_item("failed", Some(130), "111"),
                    exec_out("stop", &["Script completed\n"]),
                ],
            );
            assert!(un.is_empty(), "{un:?}");
            let kinds: Vec<_> = events.into_iter().map(|e| e.kind).collect();
            assert_eq!(outcome_of(&kinds, "bg"), [ToolOutcome::Started]);
            let (events, un) =
                feed_events(&mut parser, &[cmd_item("completed", Some(exit), "222")]);
            assert!(un.is_empty(), "{un:?}");
            let kinds: Vec<_> = events.into_iter().map(|e| e.kind).collect();
            assert_eq!(
                outcome_of(&kinds, "bg"),
                [if exit == 0 {
                    ToolOutcome::Unknown
                } else {
                    ToolOutcome::Err
                }]
            );
        }
    }

    #[test]
    fn background_group_can_stop_several_sessions_together() {
        for stop_failed in [false, true] {
            let mut parser = CodexParser::new(SID);
            let (events, un) = feed_events(
                &mut parser,
                &[
                    exec(
                        "bg",
                        "text(await tools.exec_command({cmd:\"x\"})); text(await tools.exec_command({cmd:\"y\"})); text(await tools.exec_command({cmd:\"z\"}));",
                    ),
                    exec_out(
                        "bg",
                        &[
                            "Script completed\n",
                            r#"{"session_id":111}"#,
                            r#"{"session_id":222}"#,
                            r#"{"session_id":333}"#,
                        ],
                    ),
                    exec(
                        "stop",
                        r#"text(await tools.write_stdin({session_id:111,chars:"\u0003"})); text(await tools.write_stdin({session_id:222,chars:"\u0003"}));"#,
                    ),
                    cmd_item("failed", Some(130), "111"),
                    cmd_item("failed", Some(130), "222"),
                    exec_out(
                        "stop",
                        &[if stop_failed {
                            "Script failed\n"
                        } else {
                            "Script completed\n"
                        }],
                    ),
                ],
            );
            assert!(un.is_empty(), "{un:?}");
            let kinds: Vec<_> = events.into_iter().map(|e| e.kind).collect();
            assert_eq!(outcome_of(&kinds, "bg"), [ToolOutcome::Started]);
            let (events, un) = feed_events(&mut parser, &[cmd_item("completed", Some(0), "333")]);
            assert!(un.is_empty(), "{un:?}");
            let kinds: Vec<_> = events.into_iter().map(|e| e.kind).collect();
            assert_eq!(
                outcome_of(&kinds, "bg"),
                [if stop_failed {
                    ToolOutcome::Err
                } else {
                    ToolOutcome::Unknown
                }]
            );
        }
    }

    #[test]
    fn background_group_from_cell_wait_releases_process_ids_when_done() {
        let (k, un) = feed(&[
            exec(
                "bg",
                "text(await tools.exec_command({cmd:\"x\"})); text(await tools.exec_command({cmd:\"y\"}));",
            ),
            exec_out("bg", &["Script running with cell ID 7\n"]),
            func("wait", "wait", json!({"cell_id":"7"})),
            rec(
                "response_item",
                json!({"type":"function_call_output", "call_id":"wait", "output":[
                    {"type":"input_text","text":"Script completed\n"},
                    {"type":"input_text","text":r#"{"session_id":111}"#},
                    {"type":"input_text","text":r#"{"session_id":222}"#}
                ]}),
            ),
            cmd_item("failed", Some(1), "111"),
            cmd_item("completed", Some(0), "222"),
            exec(
                "healthy",
                "text(await tools.exec_command({cmd:\"cargo test\"}))",
            ),
            cmd_item("completed", Some(0), "111"),
            exec_out("healthy", &["Script completed\n"]),
        ]);
        assert_eq!(
            outcome_of(&k, "bg"),
            [ToolOutcome::Started, ToolOutcome::Err]
        );
        assert_eq!(outcome_of(&k, "healthy"), [ToolOutcome::Ok]);
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
            func_out("call_wait", "{\"status\":\"completed\"}"),
            func("call_followup", "followup_task", json!({"message": "work"})),
            func_out("call_followup", "{\"status\":\"accepted\"}"),
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
                tag(&done),
                tag(&done),
            ]
        );
        // 협업 배관의 호출·결과는 업무가 아니지만 실제 자식 생성·종료는 남긴다.
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn interrupted_is_distinct_and_only_successful_followup_activity_restarts() {
        let act = |id: &str, kind: &str| {
            rec(
                "event_msg",
                json!({"type":"item_completed","item":{"type":"SubAgentActivity",
                "id":id,"kind":kind,"agent_thread_id":CHILD,"agent_path":"/root/w"}}),
            )
        };
        let (k, un) = feed(&[
            act("initial", "started"),
            act("stop", "interrupted"),
            func(
                "send",
                "send_message",
                json!({"target":"/root/w","message":"m"}),
            ),
            act("send", "interacted"),
            func(
                "failed",
                "followup_task",
                json!({"target":"/root/w","message":"m"}),
            ),
            func_out("failed", "error"),
            act("failed", "interacted"),
            func(
                "follow",
                "followup_task",
                json!({"target":"/root/w","message":"m"}),
            ),
            act("follow", "interacted"),
            func_out("follow", ""),
            act("follow", "interacted"),
            func(
                "unknown",
                "followup_task",
                json!({"target":"/root/unknown","message":"m"}),
            ),
            rec(
                "event_msg",
                json!({"type":"item_completed","item":{"type":"SubAgentActivity",
                "id":"unknown","kind":"interacted","agent_thread_id":"unknown","agent_path":"/root/unknown"}}),
            ),
            act("other", "interacted"),
        ]);
        assert_eq!(
            k,
            [
                spawn(CHILD, None, None, 0),
                EventKind::SubagentInterrupted {
                    child_id: CHILD.into()
                },
                spawn(CHILD, None, None, 0),
            ]
        );
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
            product_observed: true,
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

    /// (줄 번호, 호출·결과, call_id). 도구 이벤트만
    fn tool_lines(ev: &[Event]) -> Vec<(u64, String, String)> {
        ev.iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolCall { call_id, .. } => {
                    Some((e.raw_ref.line_no, "call".into(), call_id.clone()))
                }
                EventKind::ToolResult {
                    call_id, outcome, ..
                } => Some((
                    e.raw_ref.line_no,
                    format!("result {outcome:?}"),
                    call_id.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cell_wait_result_goes_to_original_exec() {
        let (ev, un) = fixture("exec-cell-wait");
        let first = "call_788ab61139f70b90465ef05f8340ec8e";
        let second = "call_86d81e3c219658a5d131f5e8cf9fb84d";
        // 9·15 `Script running` 과 13·19·29 wait 호출은 아무것도 내지 않는다.
        // 21 은 exit_code 없는 session_id 가 있어 exec 의 백그라운드 시작 규칙대로 Started,
        // 32 는 write_stdin 폴링이라 판정 근거가 없어 Unknown
        assert_eq!(
            tool_lines(&ev),
            [
                (7, "call".to_string(), first.to_string()),
                (21, "result Started".into(), first.into()),
                (25, "call".into(), second.into()),
                (32, "result Unknown".into(), second.into()),
            ]
        );
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn yielded_session_id_connects_later_stdin_completion() {
        for (in_wait, plain) in [(false, false), (true, false), (false, true), (true, true)] {
            let yielded = |include: bool| {
                let header = "Script running with cell ID 7\nOutput:\n";
                let body = r#"{"session_id":4242,"output":"private-output"}"#;
                if plain {
                    json!(format!("{header}{}", if include { body } else { "" }))
                } else {
                    let mut blocks = vec![json!({"text":header})];
                    if include {
                        blocks.push(json!({"text":body}));
                    }
                    json!(blocks)
                }
            };
            let (k, un) = feed(&[
                exec("bg", "tools.exec_command({cmd:\"npm run dev\"})"),
                rec(
                    "response_item",
                    json!({"type":"custom_tool_call_output","call_id":"bg","output":yielded(!in_wait)}),
                ),
                func("w1", "wait", json!({"cell_id":7})),
                rec(
                    "response_item",
                    json!({"type":"function_call_output","call_id":"w1","output":yielded(in_wait)}),
                ),
                func("w2", "wait", json!({"cell_id":7})),
                func_out("w2", "Script completed\n"),
                exec("poll", "tools.write_stdin({session_id:4242,chars:\"\"})"),
                cmd_item("completed", Some(0), "4242"),
                exec_out("poll", &["Script completed\n"]),
            ]);
            assert_eq!(
                outcome_of(&k, "bg"),
                [ToolOutcome::Started, ToolOutcome::Ok]
            );
            assert!(
                !serde_json::to_string(&k)
                    .unwrap()
                    .contains("private-output")
            );
            assert!(un.is_empty(), "{un:?}");
        }
    }

    #[test]
    fn yielded_finished_session_is_not_started_again() {
        let (k, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cargo build\"})"),
            exec_out(
                "c1",
                &["Script running with cell ID 7\n", r#"{"session_id":4242}"#],
            ),
            func("w1", "wait", json!({"cell_id":7})),
            cmd_item("completed", Some(0), "4242"),
            func_out("w1", "Script completed\n"),
        ]);
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Ok]);
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn yielded_failure_code_survives_a_final_response_without_it() {
        for output in [
            json!(
                "Script running with cell ID 7\nOutput:\n{\"exit_code\":1,\"output\":\"private-output\"}"
            ),
            json!([{"text":"Script running with cell ID 7\n"},{"text":r#"{"exit_code":1,"output":"private-output"}"#}]),
        ] {
            let (k, un) = feed(&[
                exec("c1", "tools.exec_command({cmd:\"cargo build\"})"),
                exec_out("c1", &["Script running with cell ID 7\n"]),
                func("w1", "wait", json!({"cell_id":7})),
                rec(
                    "response_item",
                    json!({"type":"function_call_output","call_id":"w1","output":output}),
                ),
                func("w2", "wait", json!({"cell_id":7})),
                func_out("w2", "Script completed\n"),
            ]);
            assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Err]);
            assert!(
                !serde_json::to_string(&k)
                    .unwrap()
                    .contains("private-output")
            );
            assert!(un.is_empty(), "{un:?}");
        }
    }

    #[test]
    fn oversized_yielded_session_keeps_outcome_uncertain_without_raw_diagnostics() {
        let oversized =
            json!({"session_id":"s".repeat(129),"private-key":"private-output"}).to_string();
        let (k, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cargo build\"})"),
            exec_out("c1", &["Script running with cell ID 7\n", &oversized]),
            func("w1", "wait", json!({"cell_id":7})),
            cmd_item("completed", Some(0), "1"),
            func_out("w1", "Script completed\n"),
        ]);
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Unknown]);
        assert_eq!(
            un,
            [Unhandled::OutcomeUnknown {
                tool: "exec".into(),
                result_keys: vec!["session_id".into()]
            }]
        );
        assert!(!serde_json::to_string(&un).unwrap().contains("private"));
    }

    #[test]
    fn invalid_json_is_private_and_next_line_still_parses() {
        let (k, un) = feed(&["private-token {invalid".into(), user("hello")]);
        assert!(k.iter().any(|k| matches!(k, EventKind::UserMessage { .. })));
        assert_eq!(un, [Unhandled::InvalidJson]);
        assert_eq!(serde_json::to_string(&un).unwrap(), r#"["InvalidJson"]"#);
    }

    #[test]
    fn cell_wait_failure_closes_exec_with_test_counts() {
        let (k, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cargo test\"})"),
            exec_out(
                "c1",
                &["Script running with cell ID 7\nWall time 10.0 seconds\n"],
            ),
            func(
                "w1",
                "wait",
                json!({"cell_id": "7", "yield_time_ms": 20000}),
            ),
            func_out(
                "w1",
                "Script running with cell ID 7\nWall time 30.0 seconds\n",
            ),
            func("w2", "wait", json!({"cell_id": 7})),
            func_out(
                "w2",
                "Script failed\ntest result: FAILED. 3 passed; 1 failed; 0 ignored\n",
            ),
        ]);
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            ["call Test Single", "result Err"]
        );
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Err]);
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
        assert!(un.is_empty(), "{un:?}");
    }

    /// cell 대기 중 온 item 은 아직 남은 exec 에 붙어 wait 응답의 판정 근거가 된다
    #[test]
    fn item_during_cell_wait_decides_exec() {
        let (k, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cargo build\"})"),
            exec_out("c1", &["Script running with cell ID 5\n"]),
            func("w1", "wait", json!({"cell_id": "5"})),
            cmd_item("completed", Some(0), "1"),
            func_out("w1", "Script completed\n"),
        ]);
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Ok]);
        assert!(un.is_empty(), "{un:?}");
    }

    #[test]
    fn turn_aborted_closes_exec_waiting_for_cell() {
        let (k, un) = feed(&[
            exec("c1", "tools.exec_command({cmd:\"cargo build\"})"),
            exec_out("c1", &["Script running with cell ID 2\n"]),
            func("w1", "wait", json!({"cell_id": "2"})),
            rec(
                "event_msg",
                json!({"type": "turn_aborted", "reason": "interrupted"}),
            ),
            func_out("w1", "Script completed\n"),
        ]);
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            ["call Build Single", "result Unknown"]
        );
        assert_eq!(outcome_of(&k, "c1"), [ToolOutcome::Unknown]);
        assert!(un.is_empty(), "{un:?}");
    }

    /// 파일을 중간부터 읽어 cell 을 모르면 wait 는 그대로 모르는 도구다
    #[test]
    fn wait_without_known_cell_stays_unknown_tool() {
        let (k, un) = feed(&[
            func("w1", "wait", json!({"cell_id": "9"})),
            func_out("w1", "Script completed\n"),
        ]);
        assert_eq!(
            k.iter().map(tag).collect::<Vec<_>>(),
            ["call Other(\"wait\") Single", "result Unknown"]
        );
        assert!(
            un.iter()
                .any(|u| matches!(u, Unhandled::ToolName { name, .. } if name == "wait")),
            "{un:?}"
        );
    }

    #[test]
    fn replayed_cell_wait_leaves_no_pending_call() {
        use crate::office::classify::replay;
        let (ev, _) = fixture("exec-cell-wait");
        let local = chrono::FixedOffset::east_opt(0).unwrap();
        let team = replay(
            "9394fa16-cc2e-5621-fd1e-7497c4634613",
            Product::CodexApp,
            &[],
            &ev,
            local,
        );
        assert!(
            team.employees[0].pending.is_empty(),
            "{:?}",
            team.employees[0].pending
        );
    }
}
