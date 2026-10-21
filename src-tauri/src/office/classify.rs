//! 이벤트 → 방·행동·문장 (PLAN 4.7.1), 시간 전이 (4.7.2)

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, Timelike, Utc};

use super::rank::rank_of;
use super::stats::TeamStats;
use super::work::{Role, Touched, WorkTally, WorkType, touches_file};
use super::{
    Activity, Bubble, BubbleSay, Counters, Cue, Employee, Incident, Light, Mark, PendingCall, Pose,
    Rank, Room, Team, TeamPhase, TeamProps, TimelineKind, Tokens, call_hash,
};
use crate::discover::Product;
use crate::parse::{
    Compound, Confidence, Event, EventKind, TestResult, ToolKind, ToolOutcome, ToolSummary,
};

const RECENT: usize = 3;
const REJECT_HOLD: Duration = Duration::seconds(4);
const THINK_AFTER: Duration = Duration::seconds(2);
const DUCK_AFTER: Duration = Duration::seconds(10);
const THINK_GIVE_UP: Duration = Duration::minutes(10);
const WAIT_USER_AFTER: Duration = Duration::seconds(3);
const LONG_RUN: Duration = Duration::seconds(120);
const CALL_GIVE_UP: Duration = Duration::minutes(30);
const SUB_DONE_AFTER: Duration = Duration::seconds(60);
const DOZE_AFTER: Duration = Duration::minutes(3);
const OFF_AFTER: Duration = Duration::minutes(30);
const INCIDENT_CLEAR: Duration = Duration::seconds(120);
/// codex 소스의 yield 상한 (4.7.2)
const ESCALATION_WAIT: Duration = Duration::seconds(35);
const REPEAT_EDIT_WINDOW: Duration = Duration::seconds(60);
const REPEAT_EDITS: usize = 3;
const PIZZA_AFTER: Duration = Duration::hours(2);
/// 이 시간 안에 이만큼 읽으면 연속 읽기라 서고로 간다 (4.7.1)
const READ_WINDOW: Duration = Duration::seconds(10);
const READ_BURST: usize = 3;

impl Activity {
    fn new(
        sentence: impl Into<String>,
        room: Room,
        since: DateTime<Utc>,
        confidence: Confidence,
    ) -> Self {
        Self {
            sentence: sentence.into(),
            room,
            since,
            expected_secs: None,
            pose: Pose::Sit,
            confidence,
            extra: 0,
            elapsed: false,
        }
    }

    /// 문장에 우리 틀의 `{elapsed}` 자리가 있다
    fn timed(mut self) -> Self {
        self.elapsed = true;
        self
    }
}

impl Employee {
    fn new(
        id: &str,
        rank: Rank,
        parent: Option<String>,
        depth: u8,
        at: DateTime<Utc>,
        confidence: Confidence,
    ) -> Self {
        let boss = rank == Rank::Bujang;
        let (arrival, room) = if boss {
            (sentence::ARRIVAL_BOSS, Room::TeamLead)
        } else {
            (sentence::ARRIVAL, Room::Dev)
        };
        Self {
            id: id.to_string(),
            name: String::new(),
            rank,
            role: Role::Trainee,
            touched: Touched::default(),
            model: None,
            description: None,
            activity: Activity::new(arrival, room, at, confidence),
            recent: VecDeque::new(),
            counters: Counters::default(),
            last_say: None,
            hired_at: at,
            retired_at: None,
            seat: None,
            pending: Vec::new(),
            background: Vec::new(),
            quiet_since: None,
            waiting_user_since: None,
            rejected_at: None,
            asking: None,
            last_event: at,
            context_used: None,
            context_window: None,
            parent,
            depth,
            confidence,
            last_was_text: false,
            edits: HashMap::new(),
            red: None,
            review_pens: 0,
            bubble: None,
            logged: false,
            greeted: false,
            unknown_tools: HashSet::new(),
            script_said: false,
            reads: VecDeque::new(),
            ducked: None,
            spawn_logged: false,
            too_long: None,
            record: Arc::default(),
        }
    }

    /// 진행 중이거나 머무는 상태 (호출 대표 작업, 대기, 졸음 등). 행적은 끝난 일이라 넣지 않는다 (3.5)
    fn set_activity(&mut self, a: Activity) {
        self.activity = a;
        self.logged = false;
    }

    /// 끝난 일 (완료, 사고, 반려, 보고 등). 바로 행적 맨 앞에 넣어 티커가 그때 흘린다
    fn log_activity(&mut self, a: Activity) {
        self.log_past(a.clone());
        self.activity = a;
        self.logged = true;
    }

    /// 지금 활동은 그대로 두고 행적에만 넣는다
    fn log_past(&mut self, a: Activity) {
        self.recent.push_front(a);
        self.recent.truncate(RECENT);
    }

    fn is_boss(&self) -> bool {
        self.rank == Rank::Bujang
    }

    /// 일 말풍선. 사장님 손이 필요한 직원은 작업 중·농담 풀을 쓰지 않는다 (3.6)
    /// 냈으면 true. "처음 한 번" 알림은 낸 뒤에만 소비한다
    fn say(&mut self, at: DateTime<Utc>, say: BubbleSay) -> bool {
        !needs_boss(self, at) && self.say_waiting(at, say)
    }

    /// 대기용 말풍선 (허가 대기 질문, 계획 결재, 러버덕, 부장 응답 원문). 사장님 손이 필요해도 낸다.
    /// 지금 말풍선보다 오래됐거나 같은 시각이면 덮지 않는다. 틱과 이벤트의 순서가 달라도
    /// (실시간 대 재구성) 같은 말풍선이 남게
    fn say_waiting(&mut self, at: DateTime<Utc>, say: BubbleSay) -> bool {
        if self.bubble.as_ref().is_some_and(|b| b.at >= at) {
            return false;
        }
        self.bubble = Some(Bubble { at, say });
        true
    }
}

impl TeamProps {
    fn new(at: DateTime<Utc>) -> Self {
        Self {
            boss_visit: None,
            plant: None,
            light: Light::Off,
            light_at: at,
            trash: 0,
            safe_since: at,
            pizza: 0,
            kanban: Vec::new(),
            night: false,
            pr: None,
        }
    }
}

impl Team {
    pub fn new(id: impl Into<String>, product: Product, started: DateTime<Utc>) -> Self {
        let id = id.into();
        let boss = Employee::new(&id, Rank::Bujang, None, 0, started, Confidence::Confirmed);
        Self {
            id,
            product,
            product_known: false,
            log_product: product,
            registry_product: None,
            title: String::new(),
            log_title: String::new(),
            registry_name: None,
            work_type: WorkType::NewDev,
            work: WorkTally::default(),
            cwd: PathBuf::new(),
            git_branch: None,
            started,
            last_activity: started,
            phase: TeamPhase::Working,
            alive_in_registry: false,
            desk_slot: None,
            employees: vec![boss],
            props: TeamProps::new(started),
            incident: None,
            stats: TeamStats::default(),
            file_ts: HashMap::new(),
            light_before: Light::Off,
            light_at_before: started,
            awake_since: started,
            pizza_at_wake: 0,
            seen_vendors: HashSet::new(),
            seen_skills: HashSet::new(),
            sources: Vec::new(),
            seq: 0,
            turns: 0,
            tokens: None,
        }
    }

    /// 재구성 전에 meta.json 으로 서브를 먼저 만든다 (4.3 직원 사전 등록).
    /// 자식 파일의 이벤트가 부모의 Spawn 보다 이른 ts 라서 없으면 임시 직원이 생긴다
    pub fn preregister(
        &mut self,
        agent_id: &str,
        model: Option<&str>,
        effort: Option<&str>,
        description: Option<&str>,
        depth: u8,
        at: DateTime<Utc>,
    ) {
        if self.employees.iter().any(|e| e.id == agent_id) {
            return;
        }
        let mut e = Employee::new(
            agent_id,
            rank_of(model, effort),
            None,
            depth,
            at,
            Confidence::Confirmed,
        );
        e.model = model.map(str::to_string);
        e.description = description.map(str::to_string);
        self.employees.push(e);
    }

    /// 서브 파일·meta.json 이 사라진 직원만 퇴장시킨다. 팀은 그대로 둔다 (4.5 신규 세션 발견 5번)
    pub fn retire_employee(&mut self, agent_id: &str, at: DateTime<Utc>, cues: &mut Vec<Cue>) {
        if let Some(i) = self.employees.iter().skip(1).position(|e| e.id == agent_id) {
            self.retire(i + 1, at, Confidence::Confirmed, cues);
        }
    }

    /// 이벤트 하나를 적용한다 (4.7.1). 시각은 언제나 `ev.ts` 다. 없으면 같은 파일의 직전 ts
    pub fn apply(&mut self, ev: &Event, cues: &mut Vec<Cue>) {
        if ev.kind == EventKind::Reset {
            // 팀 전체를 다시 만드는 것은 부르는 쪽 몫이다 (4.3 재구성 규칙)
            return;
        }
        let path = &ev.raw_ref.path;
        let ts = ev
            .ts
            .or_else(|| self.file_ts.get(path).copied())
            .unwrap_or(self.last_activity);
        if let Some(t) = ev.ts {
            match self.file_ts.get_mut(path) {
                Some(last) => *last = t,
                None => {
                    self.file_ts.insert(path.clone(), t);
                }
            }
            // 팀 시계를 앞으로 미는 이벤트만 깨운다. 늦게 읽힌 옛 줄은 침묵을 끊지 않았다
            if t >= self.last_activity {
                if self.phase == TeamPhase::Off {
                    self.awake_since = t;
                    self.pizza_at_wake = self.props.pizza;
                }
                self.phase = TeamPhase::Working;
            }
            self.last_activity = self.last_activity.max(t);
        }
        let Some(i) = self.employee_for(ev, ts, cues) else {
            return;
        };
        self.employees[i].last_event = ts;
        // 상태를 바꾸기 전에 본다: 같은 제목·PR·중복 Spawn 은 턴마다 다시 쓰여 기록하면 타임라인이 같은 줄로 찬다
        let fresh = self.recordable(ev);
        match &ev.kind {
            EventKind::SessionMeta {
                cwd,
                model,
                product,
                product_observed,
                git_branch,
                ..
            } => {
                self.cwd = cwd.clone();
                // 관측해 정한 동은 뒤의 기본값 meta 가 덮지 않는다
                if *product_observed || !self.product_known {
                    self.log_product = *product;
                    self.product_known |= *product_observed;
                }
                self.show_product();
                self.git_branch = git_branch.clone();
                if model.is_some() {
                    self.employees[i].model = model.clone();
                }
            }
            EventKind::UserMessage { text } => {
                let e = &mut self.employees[i];
                e.quiet_since = Some(ts);
                e.waiting_user_since = None;
                e.last_was_text = false;
                // 서브 파일의 지시는 사장님 방문이 아니다
                let boss = e.is_boss();
                if boss {
                    // 사장님이 답했으면 반려 직후가 끝난다
                    e.rejected_at = None;
                    self.props.boss_visit = Some(text.clone());
                    cues.push(Cue::BossVisit { text: text.clone() });
                    self.work.message(text);
                    self.sync_work();
                }
                if !self.refresh(i, ts) && boss {
                    self.employees[i].log_activity(Activity::new(
                        sentence::REQUEST,
                        Room::TeamLead,
                        ts,
                        Confidence::Confirmed,
                    ));
                }
            }
            EventKind::Thinking => {
                let e = &mut self.employees[i];
                e.last_say = Some("…".to_string());
                e.waiting_user_since = None;
                e.last_was_text = false;
            }
            EventKind::AssistantText { text } => {
                let e = &mut self.employees[i];
                e.last_say = Some(text.chars().take(60).collect());
                e.quiet_since = None;
                e.last_was_text = true;
                if e.is_boss() {
                    e.waiting_user_since = Some(ts);
                }
                // 리뷰실에서 한 말의 `파일:줄` 이 빨간펜이다. 리뷰 중엔 리뷰실에 남는다 (3.4)
                let (sentence, room) = if e.activity.room == Room::Review {
                    e.review_pens += count_file_lines(text);
                    (sentence::review(e.review_pens), Room::Review)
                } else {
                    (sentence::report(text), home_room(e))
                };
                if !self.refresh(i, ts) {
                    self.employees[i].log_activity(Activity {
                        pose: Pose::Type,
                        ..Activity::new(sentence, room, ts, Confidence::Confirmed)
                    });
                }
                let e = &mut self.employees[i];
                // 응답 원문은 풀이 아니다. 반려 직후 4초에도 낸다 (3.6)
                if e.is_boss() {
                    e.say_waiting(
                        ts,
                        BubbleSay::Say {
                            text: sentence::cut(text, sentence::LINE),
                        },
                    );
                }
            }
            EventKind::ToolCall {
                call_id,
                tool,
                summary,
            } => {
                if lights(tool, summary) {
                    if self.props.light != Light::Yellow {
                        self.light_before = self.props.light;
                        self.light_at_before = self.props.light_at;
                    }
                    self.props.light = Light::Yellow;
                    self.props.light_at = ts;
                }
                let e = &mut self.employees[i];
                e.waiting_user_since = None;
                e.quiet_since = None;
                e.last_was_text = false;
                count(&mut e.counters, tool);
                if touches_file(tool)
                    && let Some(p) = &summary.path
                {
                    e.role = e.touched.touch(p, &self.cwd);
                }
                if e.is_boss() {
                    self.work.call(call_id, tool, summary.path.as_deref());
                }
                if is_review(tool) && e.activity.room != Room::Review {
                    e.review_pens = 0;
                }
                if *tool == ToolKind::FileEdit
                    && let Some(p) = &summary.path
                {
                    e.edits.entry(p.clone()).or_default().push(ts);
                }
                let big_read = *tool == ToolKind::FileRead && {
                    e.reads.push_back(ts);
                    e.reads.retain(|&t| ts - t <= READ_WINDOW);
                    summary.range.is_none()
                        || e.reads.iter().filter(|&&t| t <= ts).count() >= READ_BURST
                };
                e.pending.push(PendingCall {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    summary: summary.clone(),
                    started: ts,
                    big_read,
                });
                if *tool == ToolKind::Spawn {
                    cues.push(Cue::Clap {
                        agent: e.id.clone(),
                    });
                }
                self.refresh(i, ts);
                self.sync_work();
                self.call_bubble(i, tool, summary, ts);
            }
            EventKind::ToolResult {
                call_id, outcome, ..
            } => {
                self.work.result(call_id, *outcome);
                self.on_result(i, &ev.kind, ts, cues);
                self.sync_work();
            }
            EventKind::SubagentSpawn {
                child_id,
                prompt,
                model,
                description,
                effort,
                depth,
            } => {
                let parent = ev.agent_id.clone();
                let (rank, desc, fresh) =
                    match self.employees.iter_mut().find(|e| &e.id == child_id) {
                        Some(e) => {
                            // 앞선 ts 의 중복 Spawn(meta.json 선발견, Codex 중복 started)은 재출근이 아니다
                            if rehires(e, ev.ts) {
                                e.retired_at = None;
                                e.spawn_logged = false;
                                // 옛 텍스트 응답이 남으면 다음 tick 이 60초 규칙으로 다시 퇴근시킨다 (4.7.2)
                                e.last_was_text = false;
                                cues.push(Cue::Rehire {
                                    agent: e.id.clone(),
                                });
                            }
                            // 임시 직원이었으면 이제야 직급을 안다
                            if e.confidence == Confidence::Inferred || model.is_some() {
                                e.rank = rank_of(model.as_deref(), effort.as_deref());
                            }
                            e.confidence = Confidence::Confirmed;
                            e.model = model.clone().or(e.model.take());
                            e.description = description.clone().or(e.description.take());
                            e.parent = Some(parent);
                            let fresh = !std::mem::replace(&mut e.spawn_logged, true);
                            (e.rank, e.description.clone(), fresh)
                        }
                        None => {
                            let mut e = Employee::new(
                                child_id,
                                rank_of(model.as_deref(), effort.as_deref()),
                                Some(parent),
                                *depth,
                                ts,
                                Confidence::Confirmed,
                            );
                            e.model = model.clone();
                            e.description = description.clone();
                            e.spawn_logged = true;
                            cues.push(Cue::Enter {
                                agent: e.id.clone(),
                            });
                            let r = (e.rank, e.description.clone(), true);
                            self.employees.push(e);
                            r
                        }
                    };
                // 이름은 world 가 붙여서 여기선 직급과 지시 내용으로 쓴다.
                // 허가 대기나 다른 진행 중 호출이 있으면 그쪽이 앞선다 (2.5)
                let e = &mut self.employees[i];
                let busy = e.asking.is_some()
                    || e.pending.last().is_some_and(|c| c.tool != ToolKind::Spawn);
                if !busy && fresh {
                    let room = room_for(&ToolKind::Spawn, e.is_boss(), false);
                    e.log_activity(Activity::new(
                        sentence::spawn(rank, desc.as_deref().unwrap_or(prompt)),
                        room,
                        ts,
                        Confidence::Confirmed,
                    ));
                }
            }
            EventKind::SubagentDone { child_id } => {
                // 완료 알림을 받은 쪽은 이어서 응답한다. 턴 종료(결재 대기)가 아니다 (2.4)
                let e = &mut self.employees[i];
                e.waiting_user_since = None;
                e.quiet_since = Some(ts);
                if let Some(k) = self.employees.iter().position(|e| &e.id == child_id) {
                    self.retire(k, ts, Confidence::Confirmed, cues);
                }
            }
            EventKind::PermissionRequest { question, plan } => {
                // ExitPlanMode 는 계획 본문 대신 정해진 문구를 단다 (3.4 팀장실)
                let q = if *plan {
                    sentence::PLAN.to_string()
                } else {
                    question.clone()
                };
                self.employees[i].asking = Some(q);
                self.refresh(i, ts);
                let question = question.trim();
                if *plan {
                    self.employees[i].say_waiting(ts, BubbleSay::PlanApproval);
                } else if !question.is_empty() {
                    let text = sentence::cut(question, sentence::QUOTE);
                    self.employees[i].say_waiting(ts, BubbleSay::Say { text });
                }
            }
            EventKind::ContextCompaction {
                pre_tokens,
                post_tokens,
            } => {
                let e = &mut self.employees[i];
                e.context_used = *post_tokens;
                if let Some(pre) = pre_tokens {
                    e.context_window = Some(window_from_pre_tokens(*pre, e.context_window));
                }
                let boss = e.is_boss();
                if boss {
                    // 압축 레코드가 침묵을 끊는다. 안 끊으면 같은 틱에 고민·결재 대기가 분갈이를 덮는다
                    e.quiet_since = None;
                    e.waiting_user_since = None;
                    self.props.plant = Some(0.0);
                    cues.push(Cue::Repot);
                } else {
                    e.last_say = Some(sentence::REPOT_SAY.to_string());
                }
                if !self.refresh(i, ts) && boss {
                    self.employees[i].log_activity(Activity::new(
                        sentence::REPOT,
                        Room::TeamLead,
                        ts,
                        Confidence::Confirmed,
                    ));
                }
                self.employees[i].say(ts, BubbleSay::Compaction);
            }
            EventKind::TitleChanged { title } => {
                self.log_title = title.clone();
                self.show_title(cues);
            }
            EventKind::PrLinked { number, url } => {
                self.props.pr = Some((*number, url.clone()));
            }
            EventKind::TodoChanged { items } => self.props.kanban = items.clone(),
            EventKind::TokenUsage {
                input_total,
                context_window,
                ..
            } => {
                let e = &mut self.employees[i];
                e.context_used = Some(*input_total);
                // ponytail: 창 규칙 2(같은 호스트의 다른 세션에서 배운 값)는 world 가 안다, world 티켓에서 넣는다
                if context_window.is_some() {
                    e.context_window = *context_window;
                } else if e.context_window.is_none_or(|w| *input_total > w) {
                    e.context_window = Some(window_from_model(e.model.as_deref(), *input_total));
                }
                // 서브의 사용량이 팀 화분을 덮어쓰지 않게 부장 것만 쓴다
                if e.is_boss()
                    && let Some(w) = e.context_window.filter(|&w| w > 0)
                {
                    self.props.plant = Some((*input_total as f64 / w as f64).min(1.0) as f32);
                }
            }
            EventKind::Reset => {}
        }
        if fresh {
            self.record(i, ev, ts);
        }
    }

    /// 세션 등록부 값 (4.4.1 쓰임 2·3). 등록부에 없으면 둘 다 None 이라 jsonl 제목으로 돌아간다.
    /// 보이는 값(제목·동)이 바뀌었으면 true
    pub fn set_registry(
        &mut self,
        name: Option<&str>,
        product: Option<Product>,
        cues: &mut Vec<Cue>,
    ) -> bool {
        self.registry_product = product;
        self.registry_name = name.map(str::to_string);
        let moved = self.show_product();
        self.show_title(cues) || moved
    }

    /// 제목처럼 동도 값 두 개(jsonl·등록부)에서 여기서만 정한다. 등록부가 사라지면 재구성과 같은 동으로 돌아간다 (4.3).
    /// 바뀌었으면 true
    fn show_product(&mut self) -> bool {
        let p = match self.registry_product {
            Some(r) if !self.product_known => r,
            _ => self.log_product,
        };
        let moved = p != self.product;
        self.product = p;
        moved
    }

    /// 같은 제목이 턴마다 다시 쓰인다 (4.6.1). 바뀔 때만 액자를 간다
    fn show_title(&mut self, cues: &mut Vec<Cue>) -> bool {
        let title = self.registry_name.as_ref().unwrap_or(&self.log_title);
        if *title == self.title {
            return false;
        }
        self.title = title.clone();
        cues.push(Cue::FrameSwap {
            title: self.title.clone(),
        });
        true
    }

    fn recordable(&self, ev: &Event) -> bool {
        match &ev.kind {
            EventKind::TitleChanged { title } => *title != self.log_title,
            EventKind::PrLinked { number, .. } => {
                self.props.pr.as_ref().map(|p| p.0) != Some(*number)
            }
            // 재출근이 아닌 두 번째 Spawn (meta.json 선발견, Codex 의 started 와 spawn_end)
            EventKind::SubagentSpawn { child_id, .. } => !self
                .employees
                .iter()
                .any(|e| &e.id == child_id && e.spawn_logged && !rehires(e, ev.ts)),
            _ => true,
        }
    }

    /// 팝업(3.7) 재료를 쌓는다. 이벤트 순서만 보므로 재구성과 실행 중 적용이 같은 값을 낸다
    fn record(&mut self, i: usize, ev: &Event, ts: DateTime<Utc>) {
        match &ev.kind {
            EventKind::UserMessage { .. } if self.employees[i].is_boss() => self.turns += 1,
            EventKind::TokenUsage {
                input_total,
                cache_read,
                output,
                ..
            } => {
                let t = self.tokens.get_or_insert(Tokens::default());
                t.input = t
                    .input
                    .saturating_add(input_total.saturating_sub(*cache_read));
                t.output = t.output.saturating_add(*output);
            }
            EventKind::SubagentSpawn {
                child_id, prompt, ..
            } if !prompt.is_empty() => {
                if let Some(c) = self.employees.iter_mut().find(|e| &e.id == child_id)
                    && c.record.prompt.is_none()
                {
                    Arc::make_mut(&mut c.record).prompt = Some(prompt.clone());
                }
            }
            _ => {}
        }
        let Some(kind) = TimelineKind::of(&ev.kind) else {
            return;
        };
        let source = self.source(&ev.raw_ref.path);
        let seq = self.seq;
        self.seq += 1;
        let raw = &ev.raw_ref;
        let (call, outcome, test) = match &ev.kind {
            EventKind::ToolCall { call_id, .. } => (call_hash(call_id), None, None),
            EventKind::ToolResult {
                call_id,
                outcome,
                outcome_confidence,
                test_result,
                ..
            } => (
                call_hash(call_id),
                Some((*outcome, *outcome_confidence)),
                *test_result,
            ),
            _ => (0, None, None),
        };
        let mut mark = Mark {
            source,
            generation: raw.generation,
            byte_offset: raw.byte_offset,
            line_no: raw.line_no,
            ts,
            seq,
            kind,
            nth: 0,
            call,
            outcome,
            test,
        };
        let is_boss = self.employees[i].is_boss();
        let r = Arc::make_mut(&mut self.employees[i].record);
        let same = r
            .timeline
            .iter()
            .rev()
            .take_while(|m| m.same_line(&mark))
            .filter(|m| m.kind == kind)
            .count();
        mark.nth = u8::try_from(same).unwrap_or(u8::MAX);
        r.timeline.push(mark);
        match &ev.kind {
            EventKind::UserMessage { text } if r.first_message.is_none() && is_boss => {
                r.first_message = Some(text.clone());
            }
            EventKind::ToolCall { tool, summary, .. } => {
                let path = summary.path.as_deref();
                match tool {
                    ToolKind::FileRead => r.files_read.add(path),
                    ToolKind::FileWrite | ToolKind::FileEdit => r.files_edited.add(path),
                    _ => {}
                }
                // 명령으로 한 읽기(`cat`)도 실행한 명령이다
                let cmd = summary
                    .command_norm
                    .as_deref()
                    .map(|c| sentence::cut(c, COMMAND_CHARS));
                r.commands.add(cmd.as_deref());
            }
            _ => {}
        }
    }

    fn source(&mut self, path: &Path) -> u32 {
        let i = match self.sources.iter().position(|p| p == path) {
            Some(i) => i,
            None => {
                self.sources.push(path.to_path_buf());
                self.sources.len() - 1
            }
        };
        u32::try_from(i).unwrap_or(u32::MAX)
    }

    fn sync_work(&mut self) {
        if let Some(w) = self.work.verdict() {
            self.work_type = w;
        }
    }

    /// 시간 기반 전이 (4.7.2). `now` 는 벽시계 또는 논리 시계(4.3)다.
    /// `local` 은 야근 판정용 현지 시간대. 테스트가 실행 환경의 시간대에 기대지 않게 받는다.
    /// 전이가 남기는 시각은 이 틱이 아니라 문턱 시각이다. 그래야 틱 간격과 무관하다 (4.3 재구성 규칙)
    pub fn tick(&mut self, now: DateTime<Utc>, local: FixedOffset, cues: &mut Vec<Cue>) {
        if let Some(at) = self
            .incident
            .as_ref()
            .map(|inc| inc.last_error + INCIDENT_CLEAR)
            .filter(|&at| now > at)
        {
            self.end_incident(at, cues);
        }
        // 팀 단계보다 먼저 정리해야 포기한 호출이 졸음을 막지 않는다
        for i in 0..self.employees.len() {
            while let Some(k) = self.employees[i]
                .pending
                .iter()
                .position(|c| now - c.started > CALL_GIVE_UP && !waits_for_boss(c))
            {
                let call = self.employees[i].pending.remove(k);
                let at = call.started + CALL_GIVE_UP;
                self.give_up(i, call, at, true);
            }
        }
        // 직원 전이(응답 대기 만료, 서브 종료 추정)는 팀이 졸기 전에 일어났다
        let working = self.phase == TeamPhase::Working;
        for i in 0..self.employees.len() {
            self.tick_employee(i, now, working, cues);
        }
        // 팀 단계가 뒤에 와야 졸음이 결재 대기를 이긴다 (2.5 우선순위 4·5번)
        let quiet = now - self.last_activity;
        let idle = self
            .employees
            .iter()
            .filter(|e| e.retired_at.is_none())
            .all(|e| e.pending.is_empty() && e.background.is_empty() && !thinking(e, now));
        if quiet > DOZE_AFTER && idle && working {
            self.phase = TeamPhase::Dozing;
            // 마지막 활동 3분 뒤. 응답 대기(러버덕)나 호출 정리가 그보다 늦게 끝났으면 그때
            let at = self
                .employees
                .iter()
                .map(|e| match e.activity.pose {
                    Pose::Duck => e.activity.since + THINK_GIVE_UP,
                    _ => e.activity.since,
                })
                .fold(self.last_activity + DOZE_AFTER, DateTime::max);
            self.employees[0].set_activity(Activity {
                pose: Pose::Sleep,
                ..Activity::new(sentence::DOZING, Room::Pantry, at, Confidence::Confirmed)
            });
        }
        if quiet > OFF_AFTER && !self.alive_in_registry && self.phase != TeamPhase::Off {
            self.phase = TeamPhase::Off;
            // 끝내 결과가 안 올 호출과 작업표의 뒷정리 (4.7.2)
            let at = self.last_activity + OFF_AFTER;
            for i in 0..self.employees.len() {
                self.give_up_all(i, at, cues);
            }
        }
        let hour = now.with_timezone(&local).hour();
        self.props.night = self.phase != TeamPhase::Off && !(6..22).contains(&hour);
        if self.phase == TeamPhase::Working {
            // 졸거나 퇴근한 동안은 쌓지 않는다
            let due = self.pizza_at_wake + pizza_count(now - self.awake_since);
            self.props.pizza = self.props.pizza.max(due);
        }
    }

    /// 결재 대기 또는 응답 대기 (3.3 상태 점 초록 깜빡임)
    pub fn awaiting(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        thinking(e, now)
            || e.waiting_user_since
                .is_some_and(|t| now - t >= WAIT_USER_AFTER)
    }

    /// 허가 대기: 질문했거나 Codex 승인 창으로 짐작한다 (3.3 결재판 아이콘)
    pub fn asking(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        e.asking.is_some() || escalation_wait(e, now)
    }

    /// 사장님 손이 필요한 상태: 허가 대기, 결재 대기, 반려 직후 (4.7.2 끝 문단).
    /// 말풍선·토스트·장애 판정·탕비실 연출 금지가 모두 이것을 쓴다
    pub fn needs_boss(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        needs_boss(e, now)
    }

    /// 장애 해제 (4.7.3). 상황실에 있던 부장은 자리로 돌아간다 (S27)
    fn end_incident(&mut self, at: DateTime<Utc>, cues: &mut Vec<Cue>) {
        if self.incident.take().is_none() {
            return;
        }
        cues.push(Cue::Siren { on: false });
        let boss = &mut self.employees[0];
        let over = Activity::new(
            sentence::INCIDENT_OVER,
            Room::TeamLead,
            at,
            Confidence::Confirmed,
        );
        // 부장이 직접 테스트로 푼 것이든 시간이 지나 풀린 것이든 행적에는 남는다. 자리만 옮긴다
        if boss.activity.room == Room::Incident {
            boss.log_activity(over);
        } else {
            boss.log_past(over);
        }
    }

    /// 이벤트를 받을 직원. 퇴근한 직원의 늦게 읽힌 옛 줄이면 None (무시한다)
    fn employee_for(
        &mut self,
        ev: &Event,
        ts: DateTime<Utc>,
        cues: &mut Vec<Cue>,
    ) -> Option<usize> {
        if let Some(i) = self.employees.iter().position(|e| e.id == ev.agent_id) {
            let e = &mut self.employees[i];
            if e.retired_at.is_some() {
                // 종료로 본 서브가 그 뒤에 다시 움직이면 재출근 (S60)
                if !rehires(e, ev.ts) {
                    return None;
                }
                e.retired_at = None;
                cues.push(Cue::Rehire {
                    agent: e.id.clone(),
                });
            }
            return Some(i);
        }
        // 서브 파일을 Spawn 보다 먼저 읽었다. 직급·설명은 뒤의 SubagentSpawn 이 채운다 (4.3)
        let e = Employee::new(
            &ev.agent_id,
            Rank::Sawon,
            ev.parent_agent.clone(),
            1,
            ts,
            Confidence::Inferred,
        );
        self.employees.push(e);
        Some(self.employees.len() - 1)
    }

    fn on_result(&mut self, i: usize, kind: &EventKind, ts: DateTime<Utc>, cues: &mut Vec<Cue>) {
        let EventKind::ToolResult {
            call_id,
            outcome,
            outcome_confidence,
            preview,
            test_result,
        } = kind
        else {
            return;
        };
        let test_result = *test_result;
        let e = &mut self.employees[i];
        // 4.7.1 공통 전제
        let (call, was_background) =
            if let Some(k) = e.pending.iter().position(|c| &c.call_id == call_id) {
                (e.pending.remove(k), false)
            } else if let Some(k) = e.background.iter().position(|c| &c.call_id == call_id) {
                (e.background.remove(k), true)
            } else {
                return;
            };
        let agent = e.id.clone();
        if was_background {
            cues.push(Cue::Paper {
                agent: agent.clone(),
                on: false,
            });
        }
        if call.tool == ToolKind::Ask {
            e.asking = None;
        }
        if *outcome != ToolOutcome::Started {
            e.quiet_since = Some(ts);
            // 턴이 끝난 뒤 온 백그라운드 완료도 다음 응답을 부른다 (2.4 응답 대기)
            e.waiting_user_since = None;
        }
        let home = home_room(e);
        let confirmed = *outcome_confidence == Confidence::Confirmed;
        let declares = call.summary.compound != Compound::Mixed;
        match outcome {
            ToolOutcome::Started => {
                e.background.push(call.clone());
                cues.push(Cue::Paper { agent, on: true });
                let n = e.background.len();
                e.log_activity(Activity::new(
                    sentence::background(&call, n),
                    home,
                    ts,
                    Confidence::Confirmed,
                ));
            }
            ToolOutcome::Ok if confirmed => {
                // 백그라운드 소요는 실행 시간이 아니라 완료 알림을 읽은 때까지라 평균에 넣지 않는다
                if !was_background && let Some(key) = &call.summary.command_key {
                    let secs = (ts - call.started).num_milliseconds() as f32 / 1000.0;
                    self.stats.record_duration(key, secs);
                }
                self.stats.record_ok(call.summary.command_norm.as_deref());
                let e = &mut self.employees[i];
                match (&call.tool, declares) {
                    (ToolKind::Test, true) => {
                        self.props.light = Light::Green;
                        self.props.light_at = ts;
                        let t = test_result.unwrap_or(TestResult {
                            passed: 0,
                            failed: 0,
                        });
                        cues.push(Cue::Stamp {
                            passed: t.passed,
                            failed: t.failed,
                            estimated: false,
                        });
                    }
                    (ToolKind::Test, false) => {
                        if let Some(t) = test_result {
                            cues.push(Cue::Stamp {
                                passed: t.passed,
                                failed: t.failed,
                                estimated: true,
                            });
                        }
                    }
                    (ToolKind::Build, true) => {
                        self.props.light = Light::Green;
                        self.props.light_at = ts;
                    }
                    (ToolKind::GitPush | ToolKind::PrCreate, true) => cues.push(Cue::Truck),
                    (ToolKind::Release, true) => cues.push(Cue::Fireworks),
                    _ => {}
                }
                // 테스트 전부 통과는 빨간불을 끄고 장애를 푼다 (4.7.3). Mixed 는 종료 코드가
                // 통과를 보증하지 못해 본문의 숫자로만 본다
                let all_pass = call.tool == ToolKind::Test
                    && if declares {
                        test_result.is_none_or(|t| t.failed == 0)
                    } else {
                        test_result.is_some_and(|t| t.failed == 0)
                    };
                if all_pass {
                    e.red = None;
                }
                // 리뷰 도구는 끝나도 리뷰실에 남아 이어지는 지적을 센다
                let room = if is_review(&call.tool) {
                    Room::Review
                } else {
                    home
                };
                e.log_activity(Activity::new(
                    sentence::done(&call, declares, test_result, e.review_pens),
                    room,
                    ts,
                    Confidence::Confirmed,
                ));
                if all_pass {
                    e.say(ts, BubbleSay::AllGreen);
                    self.end_incident(ts, cues);
                }
            }
            ToolOutcome::Err if confirmed => {
                e.counters.errors += 1;
                cues.push(Cue::Shake { agent });
                self.props.trash += 1;
                self.props.safe_since = ts;
                match &call.tool {
                    ToolKind::Build => {
                        self.props.light = Light::Red;
                        self.props.light_at = ts;
                    }
                    ToolKind::Test => {
                        self.props.light = Light::Red;
                        self.props.light_at = ts;
                        let fails = e.red.as_ref().map_or(0, |r| r.1);
                        e.red = Some((sentence::failed_test(&call, preview), fails + 1));
                        if let Some(t) = test_result {
                            cues.push(Cue::Stamp {
                                passed: t.passed,
                                failed: t.failed,
                                estimated: !declares,
                            });
                        }
                    }
                    ToolKind::GitPush | ToolKind::PrCreate => cues.push(Cue::Return),
                    _ => {}
                }
                if self
                    .stats
                    .record_error(call.summary.command_norm.as_deref(), ts)
                {
                    match &mut self.incident {
                        Some(inc) => inc.last_error = ts,
                        None => {
                            self.incident = Some(Incident {
                                since: ts,
                                last_error: ts,
                            });
                            cues.push(Cue::Siren { on: true });
                        }
                    }
                } else if let Some(inc) = &mut self.incident {
                    inc.last_error = ts;
                }
                // 사고 1건은 말풍선이 없다 (HANDOFF 5절). 장애는 상황실 문구가 맡는다
                self.employees[i].log_activity(Activity::new(
                    sentence::accident(&call, preview),
                    home,
                    ts,
                    Confidence::Confirmed,
                ));
            }
            ToolOutcome::Rejected => {
                e.rejected_at = Some(ts);
                cues.push(Cue::Reject { agent });
                e.log_activity(Activity::new(
                    sentence::rejected(&call),
                    home,
                    ts,
                    Confidence::Confirmed,
                ));
                self.restore_light(&call);
            }
            // Unknown, 또는 본문으로 짐작한 Ok/Err. 신호등·무재해·출고·평균을 건드리지 않는다 (S59)
            _ => {
                e.log_activity(Activity::new(
                    sentence::unknown(&call),
                    home,
                    ts,
                    Confidence::Inferred,
                ));
                e.say(ts, BubbleSay::UnknownResult);
                self.restore_light(&call);
            }
        }
        self.refresh(i, ts);
    }

    /// 허가 대기와 진행 중 호출은 다른 무엇보다 앞선다 (2.5 우선순위 1·2번). 그 문장을 썼으면 true
    fn refresh(&mut self, i: usize, ts: DateTime<Utc>) -> bool {
        let boss = self.employees[i].is_boss();
        let expected = self.employees[i]
            .pending
            .last()
            .and_then(|c| c.summary.command_key.as_ref())
            .and_then(|k| self.stats.avg.get(k).copied());
        let e = &mut self.employees[i];
        if let Some(q) = &e.asking {
            let sentence = sentence::asking(q);
            // 같은 허가 대기가 이어지면 경과를 처음부터 세지 않는다
            let since = if e.activity.room == Room::Door && e.activity.sentence == sentence {
                e.activity.since
            } else {
                ts
            };
            let a = Activity::new(sentence, Room::Door, since, Confidence::Confirmed);
            e.set_activity(a);
            return true;
        }
        if let Some(call) = e.pending.last() {
            let extra = e.pending.len() - 1;
            let a = Activity {
                expected_secs: expected,
                pose: pose_for(call),
                extra: extra as u32,
                elapsed: sentence::call_timed(call, e),
                ..Activity::new(
                    sentence::call(call, extra, e, expected),
                    room_for(&call.tool, boss, call.big_read),
                    call.started,
                    Confidence::Confirmed,
                )
            };
            e.set_activity(a);
            return true;
        }
        false
    }

    /// 호출 하나에 붙는 말풍선 (3.4, 3.6, HANDOFF 5절 "말풍선" 열). 서브의 첫 일이면 직급 말투가
    /// 앞서되, 한 번뿐인 "처음 본 것" 알림은 그대로 낸다. "처음 한 번" 표시는 말풍선이 실제로
    /// 나간 뒤에만 한다. 막힌 알림을 소비하면 영영 못 낸다
    fn call_bubble(&mut self, i: usize, tool: &ToolKind, s: &ToolSummary, ts: DateTime<Utc>) {
        /// 낸 뒤에 표시할 처음 한 번 알림
        enum Once {
            Vendor(String),
            Skill(String),
            Tool(String),
            Script,
        }
        let e = &mut self.employees[i];
        let quote = |t: &str| BubbleSay::Say {
            text: sentence::cut(t, sentence::QUOTE),
        };
        let mut once = None;
        let say = match tool {
            ToolKind::FileSearch => s.pattern.as_deref().or(s.command.as_deref()).map(quote),
            ToolKind::Shell => s.command.as_deref().map(quote),
            ToolKind::WebSearch => s.pattern.as_deref().map(quote),
            ToolKind::Mcp { server, tool } => Some(if self.seen_vendors.contains(server) {
                BubbleSay::Mcp {
                    server: server.clone(),
                    tool: tool.clone(),
                }
            } else {
                once = Some(Once::Vendor(server.clone()));
                BubbleSay::NewVendor {
                    server: server.clone(),
                }
            }),
            ToolKind::Skill { name } if !self.seen_skills.contains(name) => {
                once = Some(Once::Skill(name.clone()));
                Some(BubbleSay::NewSkill {
                    skill: name.clone(),
                })
            }
            ToolKind::Other(name) if !e.unknown_tools.contains(name) => {
                once = Some(Once::Tool(name.clone()));
                Some(BubbleSay::UnknownTool)
            }
            ToolKind::Script if !e.script_said => {
                once = Some(Once::Script);
                Some(BubbleSay::ScriptFallback)
            }
            ToolKind::FileEdit => s.path.as_ref().and_then(|p| {
                let recent = e.edits.get(p)?.iter();
                let n = recent
                    .filter(|&&t| t <= ts && ts - t <= REPEAT_EDIT_WINDOW)
                    .count();
                (n == REPEAT_EDITS).then_some(BubbleSay::Scratch)
            }),
            _ => None,
        };
        // 질문은 직급 인사를 받지 않는다. 결재판을 든 채 인사가 남는다
        let greet = !e.is_boss() && !e.greeted && *tool != ToolKind::Ask;
        let say = if greet && once.is_none() {
            Some(BubbleSay::Rank)
        } else {
            say
        };
        let Some(say) = say else {
            return;
        };
        if !e.say(ts, say) {
            return;
        }
        e.greeted |= greet;
        match once {
            Some(Once::Vendor(v)) => {
                self.seen_vendors.insert(v);
            }
            Some(Once::Skill(k)) => {
                self.seen_skills.insert(k);
            }
            Some(Once::Tool(t)) => {
                e.unknown_tools.insert(t);
            }
            Some(Once::Script) => e.script_said = true,
            None => {}
        }
    }

    /// 확정 결과 없이 끝난 호출이 켠 노랑은 켜기 전 색으로 돌린다. 함께 도는 빌드·테스트가 있으면 둔다
    fn restore_light(&mut self, call: &PendingCall) {
        let running = self
            .employees
            .iter()
            .flat_map(|e| e.pending.iter().chain(&e.background))
            .any(|c| lights(&c.tool, &c.summary));
        if lights(&call.tool, &call.summary) && !running && self.props.light == Light::Yellow {
            self.props.light = self.light_before;
            self.props.light_at = self.light_at_before;
        }
    }

    /// 결과가 영영 오지 않는 호출을 Unknown 으로 정리한다 (4.7.2). 실적·평균은 건드리지 않는다
    fn give_up(&mut self, i: usize, call: PendingCall, at: DateTime<Utc>, bubble: bool) {
        if call.tool == ToolKind::Ask {
            self.employees[i].asking = None;
        }
        self.restore_light(&call);
        let e = &mut self.employees[i];
        let room = home_room(e);
        e.log_activity(Activity::new(
            sentence::unknown(&call),
            room,
            at,
            Confidence::Inferred,
        ));
        // 퇴근·서브 retire 의 뒷정리는 말풍선이 없다 (HANDOFF 5절). 30분 포기만 낸다
        if bubble {
            e.say(at, BubbleSay::UnknownResult);
        }
        self.refresh(i, at);
    }

    /// 팀 Off·퇴근: 남은 호출과 작업표를 전부 정리한다
    fn give_up_all(&mut self, i: usize, at: DateTime<Utc>, cues: &mut Vec<Cue>) {
        let e = &mut self.employees[i];
        let mut calls = std::mem::take(&mut e.pending);
        for c in e.background.drain(..) {
            cues.push(Cue::Paper {
                agent: e.id.clone(),
                on: false,
            });
            calls.push(c);
        }
        for call in calls {
            self.give_up(i, call, at, false);
        }
    }

    fn retire(&mut self, i: usize, at: DateTime<Utc>, confidence: Confidence, cues: &mut Vec<Cue>) {
        if self.employees[i].retired_at.is_some() {
            return;
        }
        // 남은 호출이 팀 졸음을 막지 않게 (4.7.2)
        self.give_up_all(i, at, cues);
        let e = &mut self.employees[i];
        e.retired_at = Some(at);
        e.set_activity(Activity::new(
            sentence::retired(confidence),
            Room::Pantry,
            at,
            confidence,
        ));
        cues.push(Cue::Retire {
            agent: e.id.clone(),
        });
    }

    fn tick_employee(&mut self, i: usize, now: DateTime<Utc>, working: bool, cues: &mut Vec<Cue>) {
        let avg = &self.stats.avg;
        let incident = self.incident.as_ref().map(|inc| inc.since);
        let e = &mut self.employees[i];
        if e.retired_at.is_some() || e.asking.is_some() {
            return;
        }
        if let Some(call) = e.pending.last().cloned() {
            // Codex 승인 창은 레코드가 없어 무결과 시간으로 짐작한다. 결과가 오면 refresh 가 푼다
            if escalation_wait(e, now) {
                e.set_activity(Activity::new(
                    sentence::escalated(&call),
                    Room::Door,
                    call.started + ESCALATION_WAIT,
                    Confidence::Inferred,
                ));
                return;
            }
            // 함께 도는 호출 하나만 평균 2배를 넘어도 쪼그린다 (4.7.2). 말풍선은 넘은 문턱 시각에
            let slow = e.pending.iter().find_map(|c| {
                let a = *c.summary.command_key.as_ref().and_then(|k| avg.get(k))?;
                ((now - c.started).num_milliseconds() as f32 / 1000.0 > a * 2.0).then(|| {
                    let at = c.started + Duration::milliseconds((a * 2000.0) as i64);
                    (c.call_id.clone(), at)
                })
            });
            if let Some((id, at)) = &slow
                && e.too_long.as_ref() != Some(id)
            {
                e.too_long = Some(id.clone());
                e.say(*at, BubbleSay::TooLong);
            }
            // 장기 실행은 함께 도는 호출 중 가장 오래된 것으로 말한다. 경과도 평균도 그 호출 것이다
            let long = e
                .pending
                .iter()
                .find(|c| now - c.started > LONG_RUN)
                .cloned();
            if let Some(c) = &long
                && let Some(s) = sentence::long_run(c, e.activity.extra as usize)
            {
                e.activity.sentence = s;
                e.activity.since = c.started;
                e.activity.expected_secs = c
                    .summary
                    .command_key
                    .as_ref()
                    .and_then(|k| avg.get(k))
                    .copied();
                e.activity.elapsed = true;
            }
            if slow.is_some() || long.is_some() {
                e.activity.pose = Pose::Squat;
            }
            return;
        }
        if e.rejected_at.is_some_and(|t| now - t < REJECT_HOLD) {
            return;
        }
        if let Some(t) = e
            .waiting_user_since
            .filter(|&t| working && now - t >= WAIT_USER_AFTER)
        {
            e.set_activity(
                Activity {
                    pose: Pose::Pace,
                    ..Activity::new(
                        sentence::WAITING_USER,
                        Room::TeamLead,
                        t,
                        Confidence::Inferred,
                    )
                }
                .timed(),
            );
            return;
        }
        // 장애 중 부장은 상황실에 있다 (3.4, S26). 사장님 손이 필요한 상태가 먼저다
        if let Some(since) = incident.filter(|_| working && e.is_boss()) {
            // 들어가는 때는 장애 시작과 앞 일이 끝난 때 중 늦은 쪽
            let entered = (e.activity.room != Room::Incident).then(|| since.max(e.activity.since));
            e.set_activity(Activity::new(
                sentence::INCIDENT,
                Room::Incident,
                since,
                Confidence::Confirmed,
            ));
            if let Some(at) = entered {
                e.say(at, BubbleSay::Incident);
            }
            return;
        }
        if let Some(t) = e.quiet_since.filter(|_| working) {
            let quiet = now - t;
            if quiet >= THINK_AFTER {
                let (sentence, pose) = if quiet > DUCK_AFTER {
                    (sentence::DUCK, Pose::Duck)
                } else {
                    (sentence::THINK, Pose::Think)
                };
                let room = home_room(e);
                // 러버덕 연출은 부장의 것이다. 한 침묵(quiet_since)에 한 번만. 자세가 아니라
                // 침묵으로 가려야 활동은 그대로 두고 침묵만 다시 세우는 이벤트 뒤에도 실시간과 재구성이 같다
                let to_duck = e.is_boss() && pose == Pose::Duck && e.ducked != Some(t);
                e.set_activity(
                    Activity {
                        pose,
                        ..Activity::new(sentence, room, t, Confidence::Inferred)
                    }
                    .timed(),
                );
                if to_duck {
                    e.ducked = Some(t);
                    e.say_waiting(t + DUCK_AFTER, BubbleSay::ThinkingDuck);
                }
                if quiet <= THINK_GIVE_UP {
                    return;
                }
                // 응답이 끊긴 것으로 보고 졸음 규칙에 맡긴다. 러버덕은 틱 간격과 무관하게 남는다
                e.quiet_since = None;
            }
        }
        // 완료 기록을 못 본 서브: 마지막이 텍스트 응답이고 60초 무변경 (4.7.2)
        if !e.is_boss()
            && e.background.is_empty()
            && e.last_was_text
            && now - e.last_event > SUB_DONE_AFTER
        {
            let at = e.last_event + SUB_DONE_AFTER;
            self.retire(i, at, Confidence::Inferred, cues);
        }
    }
}

/// 여러 파일의 이벤트를 적용 순서로 늘어놓는다 (4.3 "순서"). `files[0]` 이 메인 파일이다.
/// 키는 파일 안 누적 최대 ts 라서 ts 가 거꾸로 가는 줄도 파일 안 순서를 지킨다.
/// 키가 같으면 파일 순서, 그다음 줄 순서
pub fn merge(files: Vec<Vec<Event>>) -> Vec<Event> {
    let mut keyed = Vec::new();
    for (f, events) in files.into_iter().enumerate() {
        let mut key = None;
        for (n, ev) in events.into_iter().enumerate() {
            key = key.max(ev.ts);
            keyed.push((key, f, n, ev));
        }
    }
    keyed.sort_by_key(|k| (k.0, k.1, k.2));
    keyed.into_iter().map(|(.., ev)| ev).collect()
}

/// 직원 상세의 명령 목록 한 줄. 원문 전체는 heredoc 스크립트처럼 길 수 있다
const COMMAND_CHARS: usize = 200;

/// 퇴근한 직원은 그보다 뒤 ts 의 이벤트로만 돌아온다. 늦게 읽힌 옛 줄은 확인된 종료를 풀지 않는다 (4.7.1)
fn rehires(e: &Employee, ts: Option<DateTime<Utc>>) -> bool {
    e.retired_at.is_some_and(|r| ts.is_some_and(|t| t > r))
}

/// 사장님 결재를 기다리는 호출. 자리를 비우면 30분을 넘길 수 있어 포기하지 않는다.
/// 죽은 세션은 팀 Off 가 정리한다 (4.7.2)
fn waits_for_boss(c: &PendingCall) -> bool {
    c.tool == ToolKind::Ask || c.summary.escalated
}

/// 신호등을 노랑으로 켜는 호출. Mixed 는 결과를 보증하지 못해 신호등을 건드리지 않는다 (4.6.4)
fn lights(tool: &ToolKind, s: &ToolSummary) -> bool {
    matches!(tool, ToolKind::Build | ToolKind::Test) && s.compound != Compound::Mixed
}

/// `Team::needs_boss` 의 몸. 팀 상태를 보지 않아 직원 말풍선도 이것을 쓴다
fn needs_boss(e: &Employee, now: DateTime<Utc>) -> bool {
    e.asking.is_some()
        || escalation_wait(e, now)
        || e.waiting_user_since
            .is_some_and(|t| now - t >= WAIT_USER_AFTER)
        || e.rejected_at.is_some_and(|t| now - t < REJECT_HOLD)
}

/// Codex 승인 창 추정 (4.7.2): 대표 호출이 escalated 이고 결과 없이 35초
fn escalation_wait(e: &Employee, now: DateTime<Utc>) -> bool {
    e.pending
        .last()
        .is_some_and(|c| c.summary.escalated && now - c.started >= ESCALATION_WAIT)
}

/// 활성 2시간을 넘기면 1상자, 그 뒤 60분마다 1상자 (4.7.2, S42)
fn pizza_count(active: Duration) -> u32 {
    if active <= PIZZA_AFTER {
        return 0;
    }
    let over = (active - PIZZA_AFTER).num_seconds() - 1;
    u32::try_from(over / 3600 + 1).unwrap_or(u32::MAX)
}

fn is_review(tool: &ToolKind) -> bool {
    match tool {
        ToolKind::GitDiff | ToolKind::PrView => true,
        ToolKind::Skill { name } => name.to_ascii_lowercase().contains("review"),
        _ => false,
    }
}

/// 리뷰 지적 수: `auth.rs:42` 처럼 확장자 있는 파일 이름 뒤에 `:숫자` 가 붙은 토큰 (3.4 리뷰실)
fn count_file_lines(text: &str) -> u32 {
    let is_ref = |tok: &str| {
        tok.char_indices().any(|(k, c)| {
            c == ':'
                && tok[k + 1..].starts_with(|d: char| d.is_ascii_digit())
                && tok[..k].rsplit_once('.').is_some_and(|(name, ext)| {
                    !name.is_empty()
                        && (1..=5).contains(&ext.len())
                        && ext.chars().all(|x| x.is_ascii_alphanumeric())
                })
        })
    };
    let toks = text.split(|c: char| c.is_whitespace() || "`'\"()[]<>,".contains(c));
    u32::try_from(toks.filter(|t| is_ref(t)).count()).unwrap_or(u32::MAX)
}

/// 응답 대기 중인가 (2.5 우선순위 4번)
fn thinking(e: &Employee, now: DateTime<Utc>) -> bool {
    e.pending.is_empty()
        && e.waiting_user_since.is_none()
        && e.quiet_since
            .is_some_and(|t| now - t >= THINK_AFTER && now - t <= THINK_GIVE_UP)
}

fn home_room(e: &Employee) -> Room {
    if e.is_boss() {
        Room::TeamLead
    } else {
        Room::Dev
    }
}

fn room_for(tool: &ToolKind, boss: bool, big_read: bool) -> Room {
    match tool {
        ToolKind::FileSearch | ToolKind::GitQuery => Room::Archive,
        ToolKind::FileRead if big_read => Room::Archive,
        ToolKind::FileRead
        | ToolKind::FileWrite
        | ToolKind::FileEdit
        | ToolKind::Spawn
        | ToolKind::Todo
        | ToolKind::Other(_) => {
            if boss {
                Room::TeamLead
            } else {
                Room::Dev
            }
        }
        ToolKind::Shell | ToolKind::Script | ToolKind::Build => Room::Build,
        ToolKind::Test => Room::Test,
        ToolKind::GitDiff | ToolKind::PrView => Room::Review,
        ToolKind::Skill { name } if name.to_ascii_lowercase().contains("review") => Room::Review,
        ToolKind::GitCommit | ToolKind::GitPush | ToolKind::PrCreate | ToolKind::Release => {
            Room::Shipping
        }
        ToolKind::Mcp { .. } | ToolKind::Skill { .. } => Room::Meeting,
        ToolKind::WebSearch | ToolKind::WebFetch => Room::Outside,
        ToolKind::Ask => Room::Door,
    }
}

fn pose_for(call: &PendingCall) -> Pose {
    match call.tool {
        ToolKind::FileRead if !call.big_read => Pose::Read,
        ToolKind::FileWrite | ToolKind::FileEdit => Pose::Type,
        _ => Pose::Sit,
    }
}

/// 카드 실적 칸. Other 는 세지 않는다 (4.7.1)
fn count(c: &mut Counters, tool: &ToolKind) {
    match tool {
        ToolKind::FileRead | ToolKind::FileSearch | ToolKind::GitQuery => c.read += 1,
        ToolKind::FileWrite | ToolKind::FileEdit => c.edit += 1,
        ToolKind::Shell
        | ToolKind::Script
        | ToolKind::Build
        | ToolKind::Test
        | ToolKind::GitDiff
        | ToolKind::GitCommit
        | ToolKind::GitPush
        | ToolKind::PrView
        | ToolKind::PrCreate
        | ToolKind::Release => c.shell += 1,
        ToolKind::WebSearch | ToolKind::WebFetch => c.outside += 1,
        ToolKind::Mcp { .. } | ToolKind::Skill { .. } => c.meeting += 1,
        ToolKind::Spawn | ToolKind::Ask | ToolKind::Todo | ToolKind::Other(_) => {}
    }
}

/// 컨텍스트 창 (4.7.1 끝 문단 규칙 1): compaction 의 preTokens 를 200k·1M 중 가까운 위로 올린다
fn window_from_pre_tokens(pre: u64, known: Option<u64>) -> u64 {
    let w = if pre <= 200_000 { 200_000 } else { 1_000_000 };
    known.map_or(w, |k| k.max(w))
}

/// 서브 meta.json 에서 읽은 것 (4.3 직원 사전 등록)
#[derive(Debug, Clone)]
pub struct SubMeta {
    pub agent_id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub description: Option<String>,
    pub depth: u8,
}

/// 파일을 처음부터 읽어 팀을 다시 만든다 (4.3 재구성 규칙). `files[0]` 이 메인 파일이다
pub fn rebuild(
    id: &str,
    product: Product,
    subs: &[SubMeta],
    files: Vec<Vec<Event>>,
    local: FixedOffset,
) -> Team {
    replay(id, product, subs, &merge(files), local)
}

/// 적용 순서로 늘어놓은 이벤트로 팀을 만든다. 이벤트마다 적용하기 전에 그 ts 로 tick 해서
/// 이벤트 사이에 넘은 시간 전이가 다음 이벤트보다 먼저 일어난다. 끝난 뒤 벽시계 tick 은 부르는 쪽이 한다
pub fn replay(
    id: &str,
    product: Product,
    subs: &[SubMeta],
    events: &[Event],
    local: FixedOffset,
) -> Team {
    let started = events
        .iter()
        .find_map(|e| e.ts)
        .unwrap_or(DateTime::UNIX_EPOCH);
    let mut team = Team::new(id, product, started);
    for s in subs {
        // 하나씩 적용했을 때처럼 자기 첫 이벤트와 Spawn 중 이른 때 입사한다
        let hired = events
            .iter()
            .filter(|e| {
                e.agent_id == s.agent_id
                    || matches!(&e.kind, EventKind::SubagentSpawn { child_id, .. } if *child_id == s.agent_id)
            })
            .filter_map(|e| e.ts)
            .min()
            .unwrap_or(started);
        team.preregister(
            &s.agent_id,
            s.model.as_deref(),
            s.effort.as_deref(),
            s.description.as_deref(),
            s.depth,
            hired,
        );
    }
    // 조용한 재구성: 연출 신호는 버린다
    let mut cues = Vec::new();
    for ev in events {
        if let Some(t) = ev.ts {
            team.tick(t, local, &mut cues);
        }
        team.apply(ev, &mut cues);
        cues.clear();
    }
    team
}

/// 규칙 3: 모델명으로. 입력이 창을 넘으면 1M 으로 올린다
fn window_from_model(model: Option<&str>, input_total: u64) -> u64 {
    let m = model.unwrap_or("").to_ascii_lowercase();
    let small = m.contains("haiku") || ["-4-0", "-4-1", "claude-3"].iter().any(|p| m.contains(p));
    if small && input_total <= 200_000 {
        200_000
    } else {
        1_000_000
    }
}

/// "지금 하는 일" 문장 (3.5 표). 경과 시간은 `{elapsed}` 자리만 두고 프론트가 채운다 (4.8).
/// 경로·명령·패턴은 백틱으로 감싼다. 명령은 파서가 이미 40자로 잘랐다
pub(crate) mod sentence {
    use super::{
        Confidence, Employee, PendingCall, REPEAT_EDIT_WINDOW, REPEAT_EDITS, Rank, TestResult,
        ToolKind, is_review,
    };

    pub const QUOTE: usize = 30;
    pub const LINE: usize = 40;

    pub const ARRIVAL_BOSS: &str = "출근 중";
    pub const ARRIVAL: &str = "출근 중 · 자리 찾는 중";
    pub const REQUEST: &str = "요구사항 접수";
    pub const ASKING: &str = "결재판 들고 문 앞 대기";
    /// ExitPlanMode 의 질문 자리 (3.4 팀장실)
    pub const PLAN: &str = "계획 결재 올립니다";
    pub const INCIDENT: &str = "장애 상황실 소집, 무재해 기록 0분";
    pub const INCIDENT_OVER: &str = "장애 해제";
    pub const THINK: &str = "턱 괴고 고민 중인 듯 ({elapsed})";
    pub const DUCK: &str = "러버덕과 상담 중인 듯 ({elapsed})";
    pub const WAITING_USER: &str = "요구사항 대기 중인 듯 · 사장님 자리 비움 {elapsed}";
    pub const DOZING: &str = "졸음";
    pub const REPOT: &str = "분갈이 중 (서류 정리)";
    pub const REPOT_SAY: &str = "분갈이";

    /// 내용 속 백틱은 닮은 글자(ˋ)로 바꿔 코드 조각의 짝이 뒤집히지 않게 한다
    fn code(s: &str) -> String {
        format!("`{}`", s.replace('`', "\u{02CB}"))
    }

    /// 첫 줄을 `n` 자로. 뒤에 더 있으면 "…"
    // ponytail: char 단위라 ZWJ 이모지·NFD 자모가 경계에서 갈릴 수 있다, 신고가 오면 grapheme 단위로
    pub fn cut(text: &str, n: usize) -> String {
        let trimmed = text.trim();
        let line = trimmed.lines().next().unwrap_or_default();
        let head: String = line.chars().take(n).collect();
        let head = head.trim_end();
        if head.len() < trimmed.len() {
            format!("{head}…")
        } else {
            head.to_string()
        }
    }

    fn rank_name(r: Rank) -> &'static str {
        match r {
            Rank::Bujang => "부장",
            Rank::Gwajang => "과장",
            Rank::Daeri => "대리",
            Rank::Sawon => "사원",
        }
    }

    /// 입력이 없을 때 쓰는 도구 이름
    pub fn label(tool: &ToolKind) -> String {
        match tool {
            ToolKind::FileRead => "열람",
            ToolKind::FileSearch => "검색",
            ToolKind::FileWrite => "새 서류",
            ToolKind::FileEdit => "수정",
            ToolKind::Shell => "셸 명령",
            ToolKind::Script => "스크립트",
            ToolKind::Build => "빌드",
            ToolKind::Test => "테스트",
            ToolKind::GitQuery => "git 조회",
            ToolKind::GitDiff => "git diff",
            ToolKind::GitCommit => "커밋",
            ToolKind::GitPush => "push",
            ToolKind::PrView => "PR 조회",
            ToolKind::PrCreate => "PR 생성",
            ToolKind::Release => "릴리스",
            ToolKind::Spawn => "업무 지시",
            ToolKind::Ask => "결재",
            ToolKind::WebSearch => "웹 검색",
            ToolKind::WebFetch => "웹 열람",
            ToolKind::Todo => "칸반 정리",
            ToolKind::Mcp { server, tool } => return format!("{server} {tool}"),
            ToolKind::Skill { name } | ToolKind::Other(name) => return name.clone(),
        }
        .to_string()
    }

    /// 반려·결과 미확인·완료 문장의 대상: 명령, 경로, 패턴 순. 없으면 도구 이름
    pub fn subject(call: &PendingCall) -> String {
        let s = &call.summary;
        s.command
            .as_deref()
            .or(s.path.as_deref())
            .or(s.pattern.as_deref())
            .map_or_else(|| label(&call.tool), code)
    }

    fn command(call: &PendingCall, fallback: &str) -> String {
        call.summary
            .command
            .as_deref()
            .map_or_else(|| fallback.to_string(), code)
    }

    fn path(call: &PendingCall) -> String {
        call.summary
            .path
            .as_deref()
            .map_or_else(|| "서류".to_string(), code)
    }

    /// 평균을 알면 " (평소 n초)". 이 호출 동안 바뀌지 않아 고정 글자로 붙인다
    fn usual(expected: Option<f32>) -> String {
        let Some(secs) = expected.map(|s| s.round() as u64) else {
            return String::new();
        };
        match (secs / 60, secs % 60) {
            (0, s) => format!(" (평소 {s}초)"),
            (m, 0) => format!(" (평소 {m}분)"),
            (m, s) => format!(" (평소 {m}분 {s}초)"),
        }
    }

    fn red(r: &(String, u32)) -> String {
        format!("빨간불 끄는 중: {} {}번째 시도", code(&r.0), r.1 + 1)
    }

    pub fn report(text: &str) -> String {
        format!("보고서 제출함: \"{}\"", cut(text, QUOTE))
    }
    pub fn review(pens: u32) -> String {
        format!("PR 리뷰 중 · 빨간펜 {pens}개")
    }

    /// 진행 중 호출 (대표 작업). `extra` 는 함께 도는 호출 수, `expected` 는 이 명령의 평균 소요
    pub fn call(call: &PendingCall, extra: usize, e: &Employee, expected: Option<f32>) -> String {
        let s = &call.summary;
        let base = match &call.tool {
            ToolKind::FileRead => {
                let mut t = format!("{} 열람 중", path(call));
                if let Some((a, b)) = s.range {
                    t += &format!(" ({a}~{b}행)");
                }
                t
            }
            ToolKind::FileSearch => match (&s.pattern, &s.path) {
                (Some(p), Some(dir)) => format!("'{}' 전사 수색 중 ({})", code(p), code(dir)),
                (Some(p), None) => format!("'{}' 전사 수색 중", code(p)),
                (None, _) => format!("{} 전사 수색 중", command(call, "서류철")),
            },
            ToolKind::FileWrite => format!("{} 새 서류 작성 중", path(call)),
            ToolKind::FileEdit => match (&e.red, &s.path) {
                (Some(r), _) => red(r),
                (None, Some(p)) => {
                    let times = e.edits.get(p).map_or(&[][..], Vec::as_slice);
                    let recent = times
                        .iter()
                        .filter(|&&t| t <= call.started && call.started - t <= REPEAT_EDIT_WINDOW)
                        .count();
                    if recent >= REPEAT_EDITS {
                        format!("{} 리팩터링 중 (오늘 {}번째 수정)", code(p), times.len())
                    } else if let Some(n) = s.lines_changed {
                        format!("{} 수정 중 ({n}줄 교체)", code(p))
                    } else {
                        format!("{} 수정 중", code(p))
                    }
                }
                (None, None) => "서류 수정 중".to_string(),
            },
            ToolKind::Test => match &e.red {
                Some(r) => red(r),
                None => format!(
                    "{} 돌리는 중 · {{elapsed}}째{}",
                    command(call, "테스트"),
                    usual(expected)
                ),
            },
            ToolKind::Build => "빌드 서버 돌리는 중, 선풍기 가동".to_string(),
            ToolKind::Script => "스크립트 돌리는 중 · {elapsed}째".to_string(),
            ToolKind::Shell
            | ToolKind::GitQuery
            | ToolKind::GitPush
            | ToolKind::PrCreate
            | ToolKind::Release => format!("{} 실행 중 · {{elapsed}}째", command(call, "셸 명령")),
            ToolKind::GitDiff | ToolKind::PrView => review(e.review_pens),
            ToolKind::Skill { .. } if is_review(&call.tool) => review(e.review_pens),
            ToolKind::Skill { name } => format!("외주 매뉴얼 {} 펼쳐보는 중", code(name)),
            ToolKind::Mcp { server, tool } => {
                format!("거래처 {}와 {} 건으로 미팅 중", code(server), code(tool))
            }
            ToolKind::WebSearch | ToolKind::WebFetch => match &s.pattern {
                Some(p) => format!("'{}' 건으로 외근 중", code(p)),
                None => "외근 중".to_string(),
            },
            ToolKind::GitCommit => match &s.commit_message {
                Some(m) => format!("출고 포장 중: \"{}\"", cut(m, LINE)),
                None => format!("출고 포장 중: {}", command(call, "커밋")),
            },
            ToolKind::Spawn => "업무 지시 중".to_string(),
            ToolKind::Ask => ASKING.to_string(),
            ToolKind::Todo => "칸반 보드 정리 중".to_string(),
            ToolKind::Other(name) => format!("처음 보는 업무: {}", code(name)),
        };
        more(base, extra)
    }

    /// 함께 도는 호출이 있으면 " (외 n건)"
    fn more(base: String, extra: usize) -> String {
        if extra > 0 {
            format!("{base} (외 {extra}건)")
        } else {
            base
        }
    }

    /// `call` 이 `{elapsed}` 자리를 둔 호출인가. `call` 의 틀과 짝이다
    pub fn call_timed(call: &PendingCall, e: &Employee) -> bool {
        match call.tool {
            ToolKind::Script
            | ToolKind::Shell
            | ToolKind::GitQuery
            | ToolKind::GitPush
            | ToolKind::PrCreate
            | ToolKind::Release => true,
            ToolKind::Test => e.red.is_none(),
            _ => false,
        }
    }

    /// 서브 생성 (3.5). 이름 대신 직급과 description, 없으면 prompt 앞부분
    pub fn spawn(rank: Rank, what: &str) -> String {
        format!("{}에게 \"{}\" 지시", rank_name(rank), cut(what, QUOTE))
    }

    pub fn background(call: &PendingCall, jobs: usize) -> String {
        format!(
            "백그라운드: {} 돌아가는 중 (작업표 {jobs}장)",
            subject(call)
        )
    }

    /// 확인된 성공. Mixed 명령은 결과를 보증하지 못해 "완료"만 적는다 (4.6.4)
    pub fn done(call: &PendingCall, declares: bool, test: Option<TestResult>, pens: u32) -> String {
        let s = &call.summary;
        match &call.tool {
            ToolKind::FileRead => format!("{} 열람", path(call)),
            ToolKind::FileSearch => match &s.pattern {
                Some(p) => format!("'{}' 검색", code(p)),
                None => format!("{} 검색", command(call, "서류철")),
            },
            ToolKind::FileWrite => format!("{} 작성", path(call)),
            ToolKind::FileEdit => match s.lines_changed {
                Some(n) => format!("{} 수정 ({n}줄)", path(call)),
                None => format!("{} 수정", path(call)),
            },
            ToolKind::Test if declares => match test {
                Some(t) => format!("합격 {} / 반려 {}", t.passed, t.failed),
                None => format!("{} 통과", command(call, "테스트")),
            },
            ToolKind::Build if declares => "빌드 성공".to_string(),
            ToolKind::GitPush | ToolKind::PrCreate if declares => "출고 완료".to_string(),
            ToolKind::Release if declares => "릴리스 완료".to_string(),
            ToolKind::GitCommit => "출고 포장 완료".to_string(),
            t if is_review(t) => review(pens),
            ToolKind::WebSearch | ToolKind::WebFetch => "외근 복귀".to_string(),
            ToolKind::Mcp { server, .. } => format!("거래처 {} 미팅 완료", code(server)),
            ToolKind::Skill { name } => format!("외주 매뉴얼 {} 확인", code(name)),
            ToolKind::Spawn => "업무 지시 완료".to_string(),
            ToolKind::Ask => "결재 받음".to_string(),
            ToolKind::Todo => "칸반 보드 정리".to_string(),
            _ => format!("{} 완료", subject(call)),
        }
    }

    /// 결과 본문에서 사고 원인 한 줄. 에러 낱말이 든 줄, 없으면 "Exit code" 아닌 첫 줄
    fn cause(preview: &str) -> Option<&str> {
        let lines = || preview.lines().map(str::trim).filter(|l| !l.is_empty());
        lines()
            .find(|l| {
                let low = l.to_lowercase();
                ["error", "fail", "panic", "fatal", "denied", "rejected"]
                    .iter()
                    .any(|w| low.contains(w))
            })
            .or_else(|| lines().find(|l| !l.starts_with("Exit code")))
            .or_else(|| lines().next())
    }

    pub fn accident(call: &PendingCall, preview: &str) -> String {
        let why = cause(preview).map(|l| code(&cut(l, LINE)));
        match &call.tool {
            ToolKind::GitPush | ToolKind::PrCreate => {
                format!("반송: {}", why.unwrap_or_else(|| "push 거부됨".to_string()))
            }
            _ => format!("사고: {}", why.unwrap_or_else(|| subject(call))),
        }
    }

    /// 실패한 테스트 이름 (cargo `test x ... FAILED`, pytest `FAILED x - …`). 못 찾으면 명령
    pub fn failed_test(call: &PendingCall, preview: &str) -> String {
        preview
            .lines()
            .map(str::trim)
            .find_map(|l| {
                l.strip_prefix("test ")
                    .and_then(|r| r.strip_suffix(" ... FAILED"))
                    .or_else(|| {
                        l.strip_prefix("FAILED ")
                            .and_then(|r| r.split_whitespace().next())
                    })
            })
            .map(str::to_string)
            .or_else(|| call.summary.command.clone())
            .unwrap_or_else(|| label(&call.tool))
    }

    pub fn rejected(call: &PendingCall) -> String {
        format!("반려됨: {} 안 됩니다", subject(call))
    }
    /// 표(3.5)의 문구를 그대로 쓴다. 추정 표시는 confidence 가 한다
    pub fn unknown(call: &PendingCall) -> String {
        format!("결과 확인 불가: {}", subject(call))
    }
    /// 셸 2분 초과. 명령 없는 호출(편집 허가 대기 등)은 문장을 그대로 둔다
    pub fn long_run(call: &PendingCall, extra: usize) -> Option<String> {
        let c = call.summary.command.as_deref()?;
        Some(more(
            format!("서버실에서 땀 흘리는 중 ({} · {{elapsed}})", code(c)),
            extra,
        ))
    }
    pub fn asking(q: &str) -> String {
        if q.trim().is_empty() {
            ASKING.to_string()
        } else {
            format!("{ASKING}: 「{}」", cut(q, QUOTE))
        }
    }
    pub fn escalated(call: &PendingCall) -> String {
        format!("{} 결재판 들고 문 앞 대기 중인 듯", subject(call))
    }
    pub fn retired(c: Confidence) -> String {
        match c {
            Confidence::Confirmed => "탕비실에서 쉬는 중".to_string(),
            Confidence::Inferred => "탕비실에서 게임 중인 듯".to_string(),
        }
    }
}
