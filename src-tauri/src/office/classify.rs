//! 이벤트 → 방·행동·문장 및 시간 전이 (docs/CONTRACTS.md C8)

mod results;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, Timelike, Utc};

use super::rank::rank_of;
use super::stats::TeamStats;
use super::work::{Role, Touched, WorkTally, WorkType, touches_file};
use super::{
    Activity, Bubble, BubbleSay, Counters, Cue, Daylight, Employee, Light, Mark, PendingCall, Pose,
    Rank, Room, Team, TeamPhase, TeamProps, TestCounts, TestVerdict, TimelineKind, Tokens,
    call_hash,
};
use crate::discover::Product;
use crate::parse::{
    Compound, Confidence, Event, EventKind, ToolKind, ToolOutcome, ToolSummary, Unhandled,
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
/// 서브 종료 뒤 탕비실에 머무는 시간 (design/HANDOFF.md 5.1절 서브 퇴근)
const DEPART_AFTER: Duration = Duration::minutes(10);
const DOZE_AFTER: Duration = Duration::minutes(3);
const OFF_AFTER: Duration = Duration::minutes(30);
const INCIDENT_CLEAR: Duration = Duration::seconds(120);
/// codex 소스의 yield 상한 (docs/CONTRACTS.md C8)
const ESCALATION_WAIT: Duration = Duration::seconds(35);
const REPEAT_EDIT_WINDOW: Duration = Duration::seconds(60);
const REPEAT_EDITS: usize = 3;
const PIZZA_AFTER: Duration = Duration::hours(2);
/// 이 시간 안에 이만큼 읽으면 연속 읽기라 서고로 간다 (docs/CONTRACTS.md C8)
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
    pub(crate) fn rejection_held(&self, now: DateTime<Utc>) -> bool {
        self.rejected_at.is_some_and(|t| now - t < REJECT_HOLD)
    }

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
            room_recent: VecDeque::new(),
            counters: Counters::default(),
            last_say: None,
            hired_at: at,
            retired_at: None,
            interrupted_at: None,
            departed_at: None,
            seat: None,
            pending: Vec::new(),
            background: Vec::new(),
            quiet_since: None,
            waiting_user_since: None,
            rejected_at: None,
            asking: None,
            planning: false,
            pass_streak: 0,
            medal_until: None,
            blanket: false,
            last_check_failed: false,
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
            approved: false,
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

    pub fn build_test_light(&self) -> Light {
        if self
            .pending
            .iter()
            .chain(&self.background)
            .any(|call| lights(&call.tool, &call.summary))
        {
            Light::Yellow
        } else if self.last_check_failed {
            Light::Red
        } else {
            Light::Off
        }
    }

    /// 진행 중이거나 머무는 상태 (호출 대표 작업, 대기, 졸음 등). 행적은 끝난 일이라 넣지 않는다 (docs/CONTRACTS.md C3)
    fn set_activity(&mut self, a: Activity) {
        if a.pose != self.activity.pose || a.since != self.activity.since {
            self.blanket = false;
        }
        self.activity = a;
        self.logged = false;
    }

    /// 끝난 일 (완료, 사고, 반려, 보고 등). 바로 행적 맨 앞에 넣어 티커가 그때 흘린다
    fn log_activity(&mut self, a: Activity) {
        self.log_past(a.clone());
        self.set_activity(a);
        self.logged = true;
    }

    /// 지금 활동은 그대로 두고 행적에만 넣는다
    fn log_past(&mut self, a: Activity) {
        let room = a.room;
        self.room_recent.push_front(a.clone());
        let mut in_room = 0;
        self.room_recent.retain(|past| {
            if past.room == room {
                in_room += 1;
            }
            past.room != room || in_room <= 10
        });
        self.recent.push_front(a);
        self.recent.truncate(RECENT);
    }

    fn is_boss(&self) -> bool {
        self.rank == Rank::Bujang
    }

    /// 일 말풍선. 사장님 손이 필요한 직원은 작업 중·농담 풀을 쓰지 않는다
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
            test: None,
            test_at: at,
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
            idle_alive: false,
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
            commits: 0,
            daily: HashMap::new(),
            daily_events: Arc::new(Vec::new()),
            date: chrono::NaiveDate::MIN,
            local: FixedOffset::east_opt(0).unwrap(),
            daylight: daylight(started, FixedOffset::east_opt(0).unwrap()),
            tokens: None,
        }
    }

    /// 재구성 전에 meta.json 으로 서브를 먼저 만든다 (docs/CONTRACTS.md C4 직원 사전 등록).
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

    /// 서브 파일·meta.json 이 사라진 직원만 퇴장시킨다. 팀은 그대로 둔다 (docs/CONTRACTS.md C6 신규 세션 발견)
    pub fn retire_employee(&mut self, agent_id: &str, at: DateTime<Utc>, cues: &mut Vec<Cue>) {
        if let Some(i) = self.employees.iter().skip(1).position(|e| e.id == agent_id) {
            self.retire(i + 1, at, Confidence::Confirmed, cues);
        }
    }

    /// 이벤트 하나를 적용한다 (docs/CONTRACTS.md C8). 시각은 언제나 `ev.ts` 다. 없으면 같은 파일의 직전 ts.
    /// 끝에 `note_gaps` 를 본다
    pub fn apply(&mut self, ev: &Event, cues: &mut Vec<Cue>) {
        let before = self.words();
        let from = cues.len();
        self.apply_event(ev, cues);
        self.note_gaps(&before, from, event_name(&ev.kind), cues);
    }

    fn apply_event(&mut self, ev: &Event, cues: &mut Vec<Cue>) {
        if ev.kind == EventKind::Reset {
            // 팀 전체를 다시 만드는 것은 부르는 쪽 몫이다 (docs/CONTRACTS.md C4 재구성 규칙)
            return;
        }
        let path = &ev.raw_ref.path;
        let ts = ev
            .ts
            .or_else(|| self.file_ts.get(path).copied())
            .unwrap_or(self.last_activity);
        if let EventKind::SubagentInterrupted { child_id } = &ev.kind {
            let Some(child) = self.employees.iter().find(|e| &e.id == child_id) else {
                return;
            };
            if child.retired_at.is_some()
                || child.last_event > ts
                || child.interrupted_at.is_some_and(|at| at >= ts)
            {
                return;
            }
        }
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
                self.idle_alive = false;
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
                match (self.refresh(i, ts), boss) {
                    (false, true) => self.employees[i].log_activity(Activity::new(
                        sentence::REQUEST,
                        Room::TeamLead,
                        ts,
                        Confidence::Confirmed,
                    )),
                    (true, true) => self.conflict(i, ts, conflict::REQUEST, cues),
                    _ => {}
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
                if e.is_boss() && e.pending.is_empty() {
                    e.waiting_user_since = Some(ts);
                }
                // 리뷰실에서 한 말의 `파일:줄` 이 빨간펜이다. 리뷰 중엔 리뷰실에 남는다 (design/HANDOFF.md 5.1절)
                let (sentence, room) = if e.activity.room == Room::Review {
                    e.review_pens += count_file_lines(text);
                    e.approved |= approves(text);
                    (sentence::review(e.review_pens), Room::Review)
                } else {
                    (sentence::report(text), home_room(e))
                };
                if self.refresh(i, ts) {
                    self.conflict(i, ts, conflict::REPORT, cues);
                } else {
                    self.employees[i].log_activity(Activity {
                        pose: Pose::Type,
                        ..Activity::new(sentence, room, ts, Confidence::Confirmed)
                    });
                }
                let e = &mut self.employees[i];
                // 응답 원문은 풀이 아니다. 반려 직후 4초에도 낸다
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
                    e.approved = false;
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
                if *tool == ToolKind::EnterPlanMode {
                    e.planning = true;
                }
                e.pending.push(PendingCall {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    summary: summary.clone(),
                    started: ts,
                    big_read,
                    question: None,
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
                            if e.interrupted_at.is_some_and(|at| ts > at) {
                                e.interrupted_at = None;
                                let room = home_room(e);
                                e.set_activity(Activity::new(
                                    "작업 재개 중",
                                    room,
                                    ts,
                                    Confidence::Confirmed,
                                ));
                            }
                            // 앞선 ts 의 중복 Spawn(meta.json 선발견, Codex 중복 started)은 재출근이 아니다
                            if rehires(e, ev.ts) {
                                e.retired_at = None;
                                e.departed_at = None;
                                e.spawn_logged = false;
                                // 옛 텍스트 응답이 남으면 다음 tick 이 60초 규칙으로 다시 퇴근시킨다 (docs/CONTRACTS.md C8)
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
                // 허가 대기나 다른 진행 중 호출이 있으면 그쪽이 앞선다 (docs/CONTRACTS.md C2)
                let e = &mut self.employees[i];
                let busy = e.asking.is_some()
                    || e.pending.last().is_some_and(|c| c.tool != ToolKind::Spawn);
                if busy && fresh {
                    self.conflict(i, ts, conflict::SPAWN, cues);
                } else if fresh {
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
                // 완료 알림을 받은 쪽은 이어서 응답한다. 턴 종료(결재 대기)가 아니다 (docs/CONTRACTS.md C2)
                let e = &mut self.employees[i];
                e.waiting_user_since = None;
                e.quiet_since = Some(ts);
                if let Some(k) = self.employees.iter().position(|e| &e.id == child_id) {
                    self.employees[k].interrupted_at = None;
                    self.retire(k, ts, Confidence::Confirmed, cues);
                }
            }
            EventKind::SubagentInterrupted { child_id } => {
                let e = &mut self.employees[i];
                e.waiting_user_since = None;
                e.quiet_since = Some(ts);
                let k = self
                    .employees
                    .iter()
                    .position(|e| &e.id == child_id)
                    .unwrap();
                let calls = std::mem::take(&mut self.employees[k].pending);
                for call in calls {
                    self.work.result(&call.call_id, ToolOutcome::Unknown);
                    self.give_up(k, call, ts, false);
                }
                let child = &mut self.employees[k];
                child.interrupted_at = Some(ts);
                child.last_event = ts;
                child.asking = None;
                child.waiting_user_since = None;
                child.quiet_since = None;
                child.last_was_text = false;
                child.rejected_at = None;
                let room = home_room(child);
                child.set_activity(Activity::new(
                    "작업 중단됨 · 재개 대기",
                    room,
                    ts,
                    Confidence::Confirmed,
                ));
            }
            EventKind::PermissionRequest { question, plan } => {
                // ExitPlanMode 는 계획 본문 대신 정해진 문구를 단다 (design/HANDOFF.md 5.1절 허가 대기)
                let q = if *plan {
                    sentence::PLAN.to_string()
                } else {
                    question.clone()
                };
                let e = &mut self.employees[i];
                if let Some(call) = e.pending.last_mut() {
                    call.question = Some(q.clone());
                }
                e.asking = Some(q);
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
                cues.push(Cue::Compaction {
                    agent: e.id.clone(),
                    at: ts,
                });
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
                match (self.refresh(i, ts), boss) {
                    (false, true) => self.employees[i].log_activity(Activity::new(
                        sentence::REPOT,
                        Room::TeamLead,
                        ts,
                        Confidence::Confirmed,
                    )),
                    (true, true) => self.conflict(i, ts, conflict::REPOT, cues),
                    _ => {}
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
        if let Some(at) = self.employees[i].interrupted_at
            && matches!(ev.kind, EventKind::ToolResult { .. })
        {
            let e = &mut self.employees[i];
            let room = home_room(e);
            e.set_activity(Activity::new(
                "작업 중단됨 · 재개 대기",
                room,
                at,
                Confidence::Confirmed,
            ));
        }
        if fresh {
            self.record(i, ev, ts);
        }
    }

    /// 세션 등록부 값 (docs/CONTRACTS.md C5). 등록부에 없으면 둘 다 None 이라 jsonl 제목으로 돌아간다.
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

    /// 제목처럼 동도 값 두 개(jsonl·등록부)에서 여기서만 정한다. 등록부가 사라지면 재구성과 같은 동으로 돌아간다 (docs/CONTRACTS.md C4).
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

    /// 같은 제목이 턴마다 다시 쓰인다 (docs/CONTRACTS.md C7). 바뀔 때만 액자를 간다
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
            EventKind::SubagentSpawn { child_id, .. } => !self.employees.iter().any(|e| {
                &e.id == child_id
                    && e.spawn_logged
                    && !rehires(e, ev.ts)
                    && !e
                        .interrupted_at
                        .is_some_and(|at| ev.ts.is_some_and(|ts| ts > at))
            }),
            _ => true,
        }
    }

    /// 팝업(docs/CONTRACTS.md C9) 재료를 쌓는다. 이벤트 순서만 보므로 재구성과 실행 중 적용이 같은 값을 낸다
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

    /// 시간 기반 전이 (docs/CONTRACTS.md C8). `now` 는 벽시계 또는 논리 시계(docs/CONTRACTS.md C4)다.
    /// `local` 은 야근 판정용 현지 시간대. 테스트가 실행 환경의 시간대에 기대지 않게 받는다.
    /// 전이가 남기는 시각은 이 틱이 아니라 문턱 시각이다. 그래야 틱 간격과 무관하다 (docs/CONTRACTS.md C4 재구성 규칙).
    /// 끝에 `note_gaps` 를 본다
    pub fn tick(&mut self, now: DateTime<Utc>, local: FixedOffset, cues: &mut Vec<Cue>) {
        let before = self.words();
        let from = cues.len();
        self.tick_to(now, local, cues);
        self.note_gaps(&before, from, "tick", cues);
    }

    pub fn tests_today(&self) -> TestCounts {
        self.daily
            .get(&self.date)
            .map_or(TestCounts::default(), |d| d.0)
    }

    pub fn commits_today(&self) -> u32 {
        self.daily.get(&self.date).map_or(0, |d| d.1)
    }

    /// 현재 OS 시간대가 바뀌면 확정 결과의 UTC 시각으로 오늘 누계를 다시 묶는다.
    pub fn set_local_offset(&mut self, local: FixedOffset, now: DateTime<Utc>) -> bool {
        let before = (
            self.date,
            self.daylight,
            self.props.night,
            self.tests_today(),
            self.commits_today(),
        );
        if self.local != local {
            self.daily.clear();
            for &(at, counts, commits) in self.daily_events.iter() {
                let entry = self
                    .daily
                    .entry(at.with_timezone(&local).date_naive())
                    .or_default();
                entry.0.passed = entry.0.passed.saturating_add(counts.passed);
                entry.0.failed = entry.0.failed.saturating_add(counts.failed);
                entry.1 = entry.1.saturating_add(commits);
            }
            self.local = local;
        }
        self.date = now.with_timezone(&local).date_naive();
        self.daylight = daylight(now, local);
        self.props.night = self.phase != TeamPhase::Off && self.daylight == Daylight::Night;
        before
            != (
                self.date,
                self.daylight,
                self.props.night,
                self.tests_today(),
                self.commits_today(),
            )
    }

    pub fn daylight(&self, now: DateTime<Utc>) -> Daylight {
        daylight(now, self.local)
    }

    fn tick_to(&mut self, now: DateTime<Utc>, local: FixedOffset, cues: &mut Vec<Cue>) {
        if self.local != local {
            self.set_local_offset(local, now);
        }
        self.local = local;
        self.date = self.date.max(now.with_timezone(&local).date_naive());
        for e in &mut self.employees {
            if e.medal_until.is_some_and(|until| now >= until) {
                e.medal_until = None;
            }
        }
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
        // 팀 단계가 뒤에 와야 졸음이 결재 대기를 이긴다 (docs/CONTRACTS.md C2 우선순위)
        let quiet = now - self.last_activity;
        let idle = self
            .employees
            .iter()
            .filter(|e| e.retired_at.is_none())
            .all(|e| e.pending.is_empty() && e.background.is_empty() && !thinking(e, now));
        if quiet > DOZE_AFTER && idle && working {
            self.phase = TeamPhase::Dozing;
            for e in &mut self.employees {
                e.waiting_user_since = None;
            }
            // 마지막 활동 3분 뒤. 응답 대기(러버덕)나 호출 정리가 그보다 늦게 끝났으면 그때
            let at = self
                .employees
                .iter()
                .map(|e| e.activity.since)
                .fold(self.last_activity + DOZE_AFTER, DateTime::max);
            self.employees[0].set_activity(Activity {
                pose: Pose::Sleep,
                ..Activity::new(sentence::DOZING, Room::Pantry, at, Confidence::Confirmed)
            });
        }
        if quiet > OFF_AFTER && !self.alive_in_registry && self.phase != TeamPhase::Off {
            self.phase = TeamPhase::Off;
            // 사장님을 기다리던 호출은 30분 포기에서 빠져 퇴근이 덮는다 (docs/CONTRACTS.md C8)
            if self
                .employees
                .iter()
                .any(|e| e.asking.is_some() || e.pending.iter().any(waits_for_boss))
            {
                cues.push(Cue::Unhandled(Unhandled::StateConflict {
                    winner: word::OFF,
                    masked: word::ASKING,
                }));
            }
            // 끝내 결과가 안 올 호출과 작업표의 뒷정리 (docs/CONTRACTS.md C8)
            let at = self.last_activity + OFF_AFTER;
            for i in 0..self.employees.len() {
                self.employees[i].waiting_user_since = None;
                self.give_up_all(i, at, cues);
            }
        }
        // 서브 퇴근은 팀 단계와 무관하다. 위에서 막 종료로 본 서브도 문턱이 지났으면 같은 틱에 나간다 (docs/CONTRACTS.md C8).
        // 졸음 시각이 직원 활동 시각의 최댓값이라, 팀 단계보다 앞에 두면 한 틱으로 따라잡을 때만 퇴근 시각이 끼어든다
        for e in &mut self.employees {
            let Some(r) = e
                .retired_at
                .filter(|&r| e.departed_at.is_none() && now - r > DEPART_AFTER)
            else {
                continue;
            };
            let at = r + DEPART_AFTER;
            e.departed_at = Some(at);
            // 종료가 추정이면 퇴근도 추정이다
            let c = e.activity.confidence;
            e.set_activity(Activity::new(sentence::departed(c), Room::Door, at, c));
            cues.push(Cue::Depart {
                agent: e.id.clone(),
            });
        }
        for e in &mut self.employees {
            if self.phase == TeamPhase::Off
                || e.departed_at.is_some()
                || e.activity.pose != Pose::Sleep
            {
                e.blanket = false;
            } else if now - e.activity.since >= Duration::minutes(10) {
                // 늦은 사건의 과거 tick은 같은 수면에서 이미 넘은 경계를 되돌리지 않는다.
                e.blanket = true;
            }
        }
        // 퇴근 문턱을 넘고도 프로세스가 살아 졸음에 남았다 (S58). 넘는 틱에 팀이 바뀌어 스냅샷이 나간다
        self.idle_alive =
            self.phase == TeamPhase::Dozing && quiet > OFF_AFTER && self.alive_in_registry;
        self.daylight = self.daylight(now);
        self.props.night = self.phase != TeamPhase::Off && self.daylight == Daylight::Night;
        if self.phase == TeamPhase::Working {
            // 졸거나 퇴근한 동안은 쌓지 않는다
            let due = self.pizza_at_wake + pizza_count(now - self.awake_since);
            self.props.pizza = self.props.pizza.max(due);
        }
    }

    /// 결재 대기 또는 응답 대기 (design/HANDOFF.md 4.2절 상태 점)
    pub fn awaiting(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        thinking(e, now)
            || e.waiting_user_since
                .is_some_and(|t| now - t >= WAIT_USER_AFTER)
    }

    /// 허가 대기: 질문했거나 Codex 승인 창으로 짐작한다 (design/HANDOFF.md 4.2절 결재판 아이콘)
    pub fn asking(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        e.asking.is_some() || escalated_call(e, now).is_some()
    }

    /// 사장님 손이 필요한 상태: 허가 대기, 결재 대기, 반려 직후 (docs/CONTRACTS.md C8).
    /// 말풍선·토스트·장애 판정·탕비실 연출 금지가 모두 이것을 쓴다
    pub fn needs_boss(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        needs_boss(e, now)
    }

    /// 장애 해제 (docs/CONTRACTS.md C8). 상황실에 있던 부장은 자리로 돌아간다 (S27)
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
                e.departed_at = None;
                cues.push(Cue::Rehire {
                    agent: e.id.clone(),
                });
            }
            if let Some(at) = e.interrupted_at {
                if ev.ts.is_none_or(|ts| ts <= at) {
                    return None;
                }
                if matches!(
                    ev.kind,
                    EventKind::UserMessage { .. }
                        | EventKind::Thinking
                        | EventKind::AssistantText { .. }
                        | EventKind::ToolCall { .. }
                        | EventKind::PermissionRequest { .. }
                        | EventKind::SubagentSpawn { .. }
                ) {
                    e.interrupted_at = None;
                }
            }
            return Some(i);
        }
        // 서브 파일을 Spawn 보다 먼저 읽었다. 직급·설명은 뒤의 SubagentSpawn 이 채운다 (docs/CONTRACTS.md C4)
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

    /// 이벤트 자기 연출(`hidden`)이 허가 대기·진행 중 호출에 가려졌다 (docs/CONTRACTS.md C2 우선순위, docs/CONTRACTS.md C11 state_conflict).
    /// 이긴 쪽은 화면의 문 앞과 같은 근거로 정한다
    fn conflict(&self, i: usize, ts: DateTime<Utc>, hidden: &'static str, cues: &mut Vec<Cue>) {
        let winner = if self.asking(&self.employees[i], ts) {
            word::ASKING
        } else {
            conflict::PENDING
        };
        cues.push(Cue::Unhandled(Unhandled::StateConflict {
            winner,
            masked: hidden,
        }));
    }

    /// 화면에 보이는 상태 단어: 팀 단계와 직원마다 (docs/CONTRACTS.md C11 scenario_gap 의 직전 상태)
    fn words(&self) -> (TeamPhase, Vec<&'static str>) {
        let words = self
            .employees
            .iter()
            .map(|e| state_word(self.phase, e))
            .collect();
        (self.phase, words)
    }

    /// `apply`·`tick` 한 번에서 연출 cue 가 하나도 안 나갔는데 상태가 바뀌었고 표에 없는 전이면
    /// 미대응으로 cue 길에 함께 낸다 (docs/CONTRACTS.md C11 scenario_gap). 재구성이 cue 를 버릴 때 같이 버려져 다시 세지 않는다.
    /// 이 호출에서 새로 생긴 직원은 출근 연출이나 임시 직원이라 전이가 아니다
    fn note_gaps(
        &self,
        before: &(TeamPhase, Vec<&'static str>),
        from: usize,
        via: &'static str,
        cues: &mut Vec<Cue>,
    ) {
        if cues[from..].iter().any(Cue::shown) {
            return;
        }
        let gap = |team, from, to| {
            Cue::Unhandled(Unhandled::ScenarioGap {
                team,
                from,
                to,
                via,
            })
        };
        let (phase, words) = before;
        if *phase != self.phase && !TEAM_FOLLOWS.contains(&(*phase, self.phase)) {
            cues.push(gap(true, phase_word(*phase), phase_word(self.phase)));
        }
        for (e, &was) in self.employees.iter().zip(words) {
            let now = state_word(self.phase, e);
            if was != now && !follows(was, now) {
                cues.push(gap(false, was, now));
            }
        }
    }

    /// 허가 대기와 진행 중 호출은 다른 무엇보다 앞선다 (docs/CONTRACTS.md C2 우선순위). 그 문장을 썼으면 true
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
        if let Some(call) = escalated_call(e, ts) {
            e.set_activity(Activity::new(
                sentence::escalated(call),
                Room::Door,
                call.started + ESCALATION_WAIT,
                Confidence::Inferred,
            ));
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

    /// 호출 하나에 붙는 말풍선 (design/HANDOFF.md 5.1절 "말풍선" 열). 서브의 첫 일이면 직급 말투가
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
        let greet =
            !e.is_boss() && !e.greeted && !matches!(tool, ToolKind::Ask | ToolKind::ExitPlanMode);
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

    /// 결과가 영영 오지 않는 호출을 Unknown 으로 정리한다 (docs/CONTRACTS.md C8). 실적·평균은 건드리지 않는다
    fn give_up(&mut self, i: usize, call: PendingCall, at: DateTime<Utc>, bubble: bool) {
        if matches!(call.tool, ToolKind::Ask | ToolKind::ExitPlanMode) {
            let e = &mut self.employees[i];
            e.asking = e.pending.iter().rev().find_map(|c| c.question.clone());
        }
        self.restore_light(&call);
        let e = &mut self.employees[i];
        if call.tool == ToolKind::Test {
            e.pass_streak = 0;
        }
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
        // 남은 호출이 팀 졸음을 막지 않게 (docs/CONTRACTS.md C8)
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
        if e.retired_at.is_some() || e.interrupted_at.is_some() || e.asking.is_some() {
            return;
        }
        if !e.pending.is_empty() {
            // Codex 승인 창은 레코드가 없어 무결과 시간으로 짐작한다. 결과가 오면 refresh 가 푼다
            if let Some(call) = escalated_call(e, now) {
                e.set_activity(Activity::new(
                    sentence::escalated(call),
                    Room::Door,
                    call.started + ESCALATION_WAIT,
                    Confidence::Inferred,
                ));
                return;
            }
            // 함께 도는 호출 하나만 평균 2배를 넘어도 쪼그린다 (docs/CONTRACTS.md C8). 말풍선은 넘은 문턱 시각에
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
        if e.rejection_held(now) {
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
        // 장애 중 부장은 상황실에 있다 (design/HANDOFF.md 5.3절, S26). 사장님 손이 필요한 상태가 먼저다
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
        if e.planning && working {
            e.set_activity(Activity::new(
                sentence::PLANNING,
                Room::Conference,
                e.activity.since,
                Confidence::Confirmed,
            ));
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
                e.quiet_since = None;
                e.set_activity(Activity {
                    pose: Pose::Sleep,
                    ..Activity::new(
                        sentence::SLEEP,
                        room,
                        t + THINK_GIVE_UP,
                        Confidence::Inferred,
                    )
                });
            }
        }
        // 완료 기록을 못 본 서브: 마지막이 텍스트 응답이고 60초 무변경 (docs/CONTRACTS.md C8)
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

/// 여러 파일의 이벤트를 적용 순서로 늘어놓는다 (docs/CONTRACTS.md C4 "순서"). `files[0]` 이 메인 파일이다.
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

/// 퇴근한 직원은 그보다 뒤 ts 의 이벤트로만 돌아온다. 늦게 읽힌 옛 줄은 확인된 종료를 풀지 않는다 (docs/CONTRACTS.md C8)
fn rehires(e: &Employee, ts: Option<DateTime<Utc>>) -> bool {
    e.retired_at.is_some_and(|r| ts.is_some_and(|t| t > r))
}

/// docs/CONTRACTS.md C2 상태 단어. 미대응 기록(docs/CONTRACTS.md C11)의 key 라서 정해진 낱말만 쓴다
mod word {
    pub const ARRIVING: &str = "출근";
    pub const WORKING: &str = "근무 중";
    /// 끝난 일만 행적에 남기고 진행 중인 일이 없다
    pub const IDLE: &str = "한가함";
    pub const THINKING: &str = "응답 대기";
    pub const WAITING_USER: &str = "결재 대기";
    pub const ASKING: &str = "허가 대기";
    pub const REJECTED: &str = "반려";
    pub const INCIDENT: &str = "상황실";
    pub const DOZING: &str = "졸음";
    pub const OFF: &str = "퇴근";
    pub const SUB_DONE: &str = "서브 종료";
    pub const INTERRUPTED: &str = "중단";
}

/// state_conflict 의 이긴 쪽(허가 대기·퇴근은 상태 단어)과 가려진 연출 이름 (docs/CONTRACTS.md C11).
/// 완료는 없다. 병렬 호출(S53)에서 먼저 끝난 결과가 남은 호출에 가리는 것은 이미 대응한 시나리오다
mod conflict {
    pub const PENDING: &str = "진행 중 호출";
    pub const REQUEST: &str = "요구 접수";
    pub const REPORT: &str = "보고";
    pub const REPOT: &str = "분갈이";
    pub const SPAWN: &str = "지시";
    pub const REJECTED: &str = "반려";
    pub const ACCIDENT: &str = "사고";
}

/// 상태만으로 화면이 따라가는 전이: (도착 상태, 떠나온 상태들). 프론트는 방·자세를 상태로 그려서 cue 가 없어도 된다.
/// 출근에서는 입장 연출(Enter) 뒤 지금 상태로 간다. 출근 중 사건은 뒤에 재생한다 (S02·S19, docs/CONTRACTS.md C2). 퇴근에서는 불이 켜지며 돌아온다 (S37).
/// 반려·서브 종료로 들어가는 전이와 서브 종료에서 나오는 전이는 cue(Reject·Retire·Rehire)가 늘 붙어 표에 없다.
const FOLLOWS: &[(&str, &[&str])] = {
    use word::*;
    &[
        // S07~S31: 호출마다 그 방으로 걸어간다
        (
            WORKING,
            &[
                ARRIVING,
                IDLE,
                THINKING,
                WAITING_USER,
                REJECTED,
                INCIDENT,
                DOZING,
                OFF,
            ],
        ),
        // S06·S12·S24: 결과·응답 뒤 자리에서 행적을 남긴다
        (
            IDLE,
            &[
                ARRIVING,
                WORKING,
                THINKING,
                WAITING_USER,
                REJECTED,
                INCIDENT,
                DOZING,
                OFF,
            ],
        ),
        // S04·S05: 침묵은 자세(의자 돌림, 러버덕)로 그린다
        (THINKING, &[ARRIVING, WORKING, IDLE, REJECTED, DOZING, OFF]),
        // S32: 화면보호기, 서성임
        (WAITING_USER, &[ARRIVING, IDLE, OFF]),
        // S34: 결재판 들고 문 앞
        (
            ASKING,
            &[
                ARRIVING,
                WORKING,
                IDLE,
                THINKING,
                WAITING_USER,
                REJECTED,
                INCIDENT,
                DOZING,
                OFF,
            ],
        ),
        // S26: 경광등(Siren)은 사고 결과 때 나가고 부장은 다음 틱에 상황실로 간다
        (INCIDENT, &[WORKING, IDLE, THINKING, REJECTED, OFF]),
        // S33·S35: 탕비실 소파. 출근만 하고 3분 조용해도 간다 (S02)
        (
            DOZING,
            &[
                ARRIVING,
                WORKING,
                IDLE,
                THINKING,
                WAITING_USER,
                REJECTED,
                OFF,
            ],
        ),
        // S36·S51: 조명이 꺼지고 직원이 나간다. 허가 대기가 덮이는 것은 state_conflict 가 센다
        (
            OFF,
            &[
                ARRIVING,
                WORKING,
                IDLE,
                THINKING,
                WAITING_USER,
                ASKING,
                REJECTED,
                INCIDENT,
                DOZING,
            ],
        ),
        (ARRIVING, &[OFF]),
    ]
};

fn follows(from: &str, to: &str) -> bool {
    // 중단은 성공·퇴근 전이가 아니라 현재 실행을 멈춘 신호다. 다음 실제 활동은 어느 방에서든 재개할 수 있다.
    if from == word::INTERRUPTED || to == word::INTERRUPTED {
        return true;
    }
    FOLLOWS
        .iter()
        .any(|(t, froms)| *t == to && froms.contains(&from))
}

const TEAM_FOLLOWS: &[(TeamPhase, TeamPhase)] = &[
    // S35
    (TeamPhase::Working, TeamPhase::Dozing),
    // S36
    (TeamPhase::Working, TeamPhase::Off),
    (TeamPhase::Dozing, TeamPhase::Off),
    // S37
    (TeamPhase::Off, TeamPhase::Working),
];

fn daylight(now: DateTime<Utc>, local: FixedOffset) -> Daylight {
    match now.with_timezone(&local).hour() {
        6..17 => Daylight::Day,
        17..20 => Daylight::Evening,
        _ => Daylight::Night,
    }
}

/// 이미 정한 출력(활동·퇴장·팀 단계)에서 읽은 docs/CONTRACTS.md C2 상태 단어. 상태 기계의 판정을 다시 하지 않는다
fn state_word(phase: TeamPhase, e: &Employee) -> &'static str {
    let a = &e.activity;
    if e.departed_at.is_some() {
        word::OFF
    } else if e.retired_at.is_some() {
        word::SUB_DONE
    } else if e.interrupted_at.is_some() {
        word::INTERRUPTED
    } else if phase == TeamPhase::Off {
        word::OFF
    } else if a.room == Room::Door {
        word::ASKING
    } else if a.room == Room::Incident {
        word::INCIDENT
    } else if a.pose == Pose::Sleep {
        word::DOZING
    } else if a.pose == Pose::Pace {
        word::WAITING_USER
    } else if matches!(a.pose, Pose::Think | Pose::Duck) {
        word::THINKING
    } else if sentence::is_rejected(&a.sentence) {
        word::REJECTED
    } else if a.sentence == sentence::ARRIVAL || a.sentence == sentence::ARRIVAL_BOSS {
        word::ARRIVING
    } else if e.logged {
        word::IDLE
    } else {
        word::WORKING
    }
}

fn phase_word(p: TeamPhase) -> &'static str {
    match p {
        TeamPhase::Working => word::WORKING,
        TeamPhase::Dozing => word::DOZING,
        TeamPhase::Off => word::OFF,
    }
}

/// scenario_gap 의 via
fn event_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::UserMessage { .. } => "UserMessage",
        EventKind::Thinking => "Thinking",
        EventKind::AssistantText { .. } => "AssistantText",
        EventKind::ToolCall { .. } => "ToolCall",
        EventKind::ToolResult { .. } => "ToolResult",
        EventKind::SubagentSpawn { .. } => "SubagentSpawn",
        EventKind::SubagentDone { .. } => "SubagentDone",
        EventKind::SubagentInterrupted { .. } => "SubagentInterrupted",
        EventKind::PermissionRequest { .. } => "PermissionRequest",
        EventKind::ContextCompaction { .. } => "ContextCompaction",
        EventKind::TitleChanged { .. } => "TitleChanged",
        EventKind::PrLinked { .. } => "PrLinked",
        EventKind::TodoChanged { .. } => "TodoChanged",
        EventKind::TokenUsage { .. } => "TokenUsage",
        EventKind::SessionMeta { .. } => "SessionMeta",
        EventKind::Reset => "Reset",
    }
}

/// 사장님 결재를 기다리는 호출. 자리를 비우면 30분을 넘길 수 있어 포기하지 않는다.
/// 죽은 세션은 팀 Off 가 정리한다 (docs/CONTRACTS.md C8)
fn waits_for_boss(c: &PendingCall) -> bool {
    matches!(c.tool, ToolKind::Ask | ToolKind::ExitPlanMode) || c.summary.escalated
}

/// 신호등을 노랑으로 켜는 호출. Mixed 는 결과를 보증하지 못해 신호등을 건드리지 않는다 (docs/CONTRACTS.md C7)
fn lights(tool: &ToolKind, s: &ToolSummary) -> bool {
    matches!(tool, ToolKind::Build | ToolKind::Test) && s.compound != Compound::Mixed
}

/// `Team::needs_boss` 의 몸. 팀 상태를 보지 않아 직원 말풍선도 이것을 쓴다
fn needs_boss(e: &Employee, now: DateTime<Utc>) -> bool {
    e.asking.is_some()
        || escalated_call(e, now).is_some()
        || e.waiting_user_since
            .is_some_and(|t| now - t >= WAIT_USER_AFTER)
        || e.rejection_held(now)
}

/// Codex 승인 창 추정 (docs/CONTRACTS.md C8): 결과 없이 35초 지난 escalated 호출 중 가장 오래된 것
fn escalated_call(e: &Employee, now: DateTime<Utc>) -> Option<&PendingCall> {
    e.pending
        .iter()
        .filter(|c| c.summary.escalated && now - c.started >= ESCALATION_WAIT)
        .min_by_key(|c| c.started)
}

/// 활성 2시간을 넘기면 1상자, 그 뒤 60분마다 1상자 (docs/CONTRACTS.md C8, S42)
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

/// 리뷰 승인 문구 (design/HANDOFF.md 5.2절 승인 표식). 대소문자는 가리지 않는다
fn approves(text: &str) -> bool {
    let t = text.to_lowercase();
    let words: Vec<_> = t
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().enumerate().any(|(i, word)| {
        matches!(
            *word,
            "lgtm"
                | "approve"
                | "approved"
                | "approves"
                | "승인"
                | "승인합니다"
                | "승인했습니다"
                | "승인됐습니다"
                | "승인됨"
                | "승인함"
                | "승인해요"
                | "승인완료"
        ) && !matches!(
            i.checked_sub(1).and_then(|n| words.get(n)).copied(),
            Some("not" | "no" | "never" | "without" | "cannot" | "cant" | "미" | "불")
        ) && !(i >= 2 && words[i - 2] == "can" && words[i - 1] == "t")
            && !matches!(
                words.get(i + 1).copied(),
                Some("불가" | "아님" | "취소" | "거부" | "안함" | "안")
            )
    })
}

/// 테스트 결과판: 실패가 하나라도 있으면 실패. 도장(프론트)과 같은 기준
fn verdict(failed: u32) -> TestVerdict {
    if failed > 0 {
        TestVerdict::Fail
    } else {
        TestVerdict::Pass
    }
}

/// 리뷰 지적 수: `auth.rs:42` 처럼 확장자 있는 파일 이름 뒤에 `:숫자` 가 붙은 토큰
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

/// 응답 대기 중인가 (docs/CONTRACTS.md C2 우선순위)
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
        | ToolKind::EnterPlanMode
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
        ToolKind::Ask | ToolKind::ExitPlanMode => Room::Door,
    }
}

fn pose_for(call: &PendingCall) -> Pose {
    match call.tool {
        ToolKind::FileRead if !call.big_read => Pose::Read,
        ToolKind::FileWrite | ToolKind::FileEdit => Pose::Type,
        _ => Pose::Sit,
    }
}

/// 카드 실적 칸. Other 는 세지 않는다 (docs/CONTRACTS.md C8)
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
        ToolKind::Spawn
        | ToolKind::Ask
        | ToolKind::EnterPlanMode
        | ToolKind::ExitPlanMode
        | ToolKind::Todo
        | ToolKind::Other(_) => {}
    }
}

/// 컨텍스트 창 (docs/CONTRACTS.md C8): compaction 의 preTokens 를 200k·1M 중 가까운 위로 올린다
fn window_from_pre_tokens(pre: u64, known: Option<u64>) -> u64 {
    let w = if pre <= 200_000 { 200_000 } else { 1_000_000 };
    known.map_or(w, |k| k.max(w))
}

/// 서브 meta.json 에서 읽은 것 (docs/CONTRACTS.md C4 직원 사전 등록)
#[derive(Debug, Clone)]
pub struct SubMeta {
    pub agent_id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub description: Option<String>,
    pub depth: u8,
}

/// 파일을 처음부터 읽어 팀을 다시 만든다 (docs/CONTRACTS.md C4 재구성 규칙). `files[0]` 이 메인 파일이다
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

/// "지금 하는 일" 문장 (docs/CONTRACTS.md C3). 경과 시간은 `{elapsed}` 자리만 두고 프론트가 채운다 (docs/CONTRACTS.md C9).
/// 경로·명령·패턴은 백틱으로 감싼다. 명령은 파서가 이미 40자로 잘랐다
pub(crate) mod sentence;
