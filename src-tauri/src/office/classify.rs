//! 이벤트 → 방·행동·문장 (PLAN 4.7.1), 시간 전이 (4.7.2)

use std::collections::VecDeque;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};

use super::rank::rank_of;
use super::stats::TeamStats;
use super::{
    Activity, Counters, Cue, Employee, Incident, Light, PendingCall, Pose, Rank, Room, Team,
    TeamPhase, TeamProps,
};
use crate::discover::Product;
use crate::parse::{Compound, Confidence, Event, EventKind, TestResult, ToolKind, ToolOutcome};

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
        Self {
            id: id.to_string(),
            name: String::new(),
            rank,
            model: None,
            description: None,
            activity: Activity {
                sentence: arrival_sentence(boss),
                room: if boss { Room::TeamLead } else { Room::Dev },
                since: at,
                expected_secs: None,
                pose: Pose::Sit,
                confidence,
            },
            recent: VecDeque::new(),
            counters: Counters::default(),
            last_say: None,
            hired_at: at,
            retired_at: None,
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
        }
    }

    fn set_activity(&mut self, a: Activity) {
        if a.sentence != self.activity.sentence || a.room != self.activity.room {
            let old = std::mem::replace(&mut self.activity, a);
            self.recent.push_front(old);
            self.recent.truncate(RECENT);
        } else {
            self.activity = a;
        }
    }

    fn is_boss(&self) -> bool {
        self.rank == Rank::Bujang
    }
}

impl TeamProps {
    fn new(at: DateTime<Utc>) -> Self {
        Self {
            boss_visit: None,
            plant: None,
            light: Light::Off,
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
            title: String::new(),
            cwd: PathBuf::new(),
            git_branch: None,
            started,
            last_activity: started,
            phase: TeamPhase::Working,
            alive_in_registry: false,
            employees: vec![boss],
            props: TeamProps::new(started),
            incident: None,
            stats: TeamStats::default(),
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

    /// 이벤트 하나를 적용한다 (4.7.1). 시각은 언제나 `ev.ts` 다. 없으면 직전 이벤트 시각
    pub fn apply(&mut self, ev: &Event, cues: &mut Vec<Cue>) {
        if ev.kind == EventKind::Reset {
            // 팀 전체를 다시 만드는 것은 부르는 쪽 몫이다 (4.3 재구성 규칙)
            return;
        }
        let ts = ev.ts.unwrap_or(self.last_activity);
        self.last_activity = self.last_activity.max(ts);
        self.phase = TeamPhase::Working;
        let i = self.employee_for(&ev.agent_id, ev.parent_agent.as_deref(), ts, cues);
        self.employees[i].last_event = ts;
        match &ev.kind {
            EventKind::SessionMeta {
                cwd,
                model,
                product,
                git_branch,
                ..
            } => {
                self.cwd = cwd.clone();
                self.product = *product;
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
                if e.is_boss() {
                    e.set_activity(boss_activity(sentence::request(), Room::TeamLead, ts));
                    self.props.boss_visit = Some(text.clone());
                    cues.push(Cue::BossVisit { text: text.clone() });
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
                let room = home_room(e);
                e.set_activity(Activity {
                    sentence: sentence::report(text),
                    room,
                    since: ts,
                    expected_secs: None,
                    pose: Pose::Type,
                    confidence: Confidence::Confirmed,
                });
            }
            EventKind::ToolCall {
                call_id,
                tool,
                summary,
            } => {
                let e = &mut self.employees[i];
                e.waiting_user_since = None;
                e.quiet_since = None;
                e.last_was_text = false;
                count(&mut e.counters, tool);
                e.pending.push(PendingCall {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    summary: summary.clone(),
                    started: ts,
                });
                match tool {
                    ToolKind::Spawn => cues.push(Cue::Clap {
                        agent: e.id.clone(),
                    }),
                    ToolKind::Build | ToolKind::Test => self.props.light = Light::Yellow,
                    _ => {}
                }
                self.refresh(i, ts);
            }
            EventKind::ToolResult {
                call_id,
                outcome,
                outcome_confidence,
                preview,
                test_result,
            } => self.on_result(
                i,
                call_id,
                *outcome,
                *outcome_confidence,
                preview,
                *test_result,
                ts,
                cues,
            ),
            EventKind::SubagentSpawn {
                child_id,
                model,
                description,
                depth,
                ..
            } => {
                let parent = ev.agent_id.clone();
                match self.employees.iter_mut().find(|e| &e.id == child_id) {
                    Some(e) => {
                        if e.retired_at.take().is_some() {
                            cues.push(Cue::Rehire {
                                agent: e.id.clone(),
                            });
                        }
                        // 임시 직원이었으면 이제야 직급을 안다
                        if e.confidence == Confidence::Inferred || model.is_some() {
                            e.rank = rank_of(model.as_deref(), None);
                        }
                        e.confidence = Confidence::Confirmed;
                        e.model = model.clone().or(e.model.take());
                        e.description = description.clone().or(e.description.take());
                        e.parent = Some(parent);
                    }
                    None => {
                        let mut e = Employee::new(
                            child_id,
                            rank_of(model.as_deref(), None),
                            Some(parent),
                            *depth,
                            ts,
                            Confidence::Confirmed,
                        );
                        e.model = model.clone();
                        e.description = description.clone();
                        cues.push(Cue::Enter {
                            agent: e.id.clone(),
                        });
                        self.employees.push(e);
                    }
                }
            }
            EventKind::SubagentDone { child_id } => {
                if let Some(e) = self.employees.iter_mut().find(|e| &e.id == child_id) {
                    retire(e, ts, Confidence::Confirmed, cues);
                }
            }
            EventKind::PermissionRequest { question } => {
                self.employees[i].asking = Some(question.clone());
                self.refresh(i, ts);
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
                if e.is_boss() {
                    self.props.plant = Some(0.0);
                    cues.push(Cue::Repot);
                    e.set_activity(boss_activity(sentence::repot(), Room::TeamLead, ts));
                } else {
                    e.last_say = Some(sentence::repot_say());
                }
            }
            EventKind::TitleChanged { title } => {
                // 같은 제목이 턴마다 다시 쓰인다 (4.6.1). 바뀔 때만 액자를 간다
                if *title != self.title {
                    self.title = title.clone();
                    cues.push(Cue::FrameSwap {
                        title: title.clone(),
                    });
                }
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
    }

    /// 시간 기반 전이 (4.7.2). `now` 는 벽시계 또는 논리 시계(4.3)다
    pub fn tick(&mut self, now: DateTime<Utc>, cues: &mut Vec<Cue>) {
        if self
            .incident
            .as_ref()
            .is_some_and(|inc| now - inc.last_error > INCIDENT_CLEAR)
        {
            self.incident = None;
            cues.push(Cue::Siren { on: false });
        }
        let quiet = now - self.last_activity;
        let idle = self
            .employees
            .iter()
            .all(|e| e.pending.is_empty() && e.background.is_empty() && !thinking(e, now));
        if quiet > OFF_AFTER && !self.alive_in_registry {
            if self.phase != TeamPhase::Off {
                self.phase = TeamPhase::Off;
                // 끝내 결과가 안 올 호출의 뒷정리 (4.7.2)
                for e in &mut self.employees {
                    give_up_calls(e, now);
                }
            }
        } else if quiet > DOZE_AFTER && idle && self.phase == TeamPhase::Working {
            self.phase = TeamPhase::Dozing;
            self.employees[0].set_activity(Activity {
                sentence: sentence::dozing(),
                room: Room::Pantry,
                since: now,
                expected_secs: None,
                pose: Pose::Sleep,
                confidence: Confidence::Confirmed,
            });
        }
        // 팀 단계를 먼저 정해야 졸음이 결재 대기를 이긴다 (2.5 우선순위 4·5번)
        let working = self.phase == TeamPhase::Working;
        for i in 0..self.employees.len() {
            self.tick_employee(i, now, working, cues);
        }
    }

    /// 사장님 손이 필요한 상태: 허가 대기, 결재 대기, 반려 직후 (4.7.2 끝 문단).
    /// 말풍선·토스트·장애 판정·탕비실 연출 금지가 모두 이것을 쓴다
    pub fn needs_boss(&self, e: &Employee, now: DateTime<Utc>) -> bool {
        e.asking.is_some()
            || e.waiting_user_since
                .is_some_and(|t| now - t >= WAIT_USER_AFTER)
            || e.rejected_at.is_some_and(|t| now - t < REJECT_HOLD)
    }

    fn employee_for(
        &mut self,
        agent_id: &str,
        parent: Option<&str>,
        ts: DateTime<Utc>,
        cues: &mut Vec<Cue>,
    ) -> usize {
        if let Some(i) = self.employees.iter().position(|e| e.id == agent_id) {
            let e = &mut self.employees[i];
            // 종료로 본 서브가 다시 움직이면 재출근 (S60)
            if e.retired_at.take().is_some() {
                cues.push(Cue::Rehire {
                    agent: e.id.clone(),
                });
            }
            return i;
        }
        // 서브 파일을 Spawn 보다 먼저 읽었다. 직급·설명은 뒤의 SubagentSpawn 이 채운다 (4.3)
        let e = Employee::new(
            agent_id,
            Rank::Sawon,
            parent.map(str::to_string),
            1,
            ts,
            Confidence::Inferred,
        );
        self.employees.push(e);
        self.employees.len() - 1
    }

    #[allow(clippy::too_many_arguments)]
    fn on_result(
        &mut self,
        i: usize,
        call_id: &str,
        outcome: ToolOutcome,
        confidence: Confidence,
        preview: &str,
        test_result: Option<TestResult>,
        ts: DateTime<Utc>,
        cues: &mut Vec<Cue>,
    ) {
        let e = &mut self.employees[i];
        // 짝 없는 결과는 무시한다 (4.7.1 공통 전제)
        let (call, was_background) =
            if let Some(k) = e.pending.iter().position(|c| c.call_id == call_id) {
                (e.pending.remove(k), false)
            } else if let Some(k) = e.background.iter().position(|c| c.call_id == call_id) {
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
        if outcome != ToolOutcome::Started {
            e.quiet_since = Some(ts);
        }
        let home = home_room(e);
        let confirmed = confidence == Confidence::Confirmed;
        let declares = call.summary.compound != Compound::Mixed;
        let activity = |sentence: String, confidence| Activity {
            sentence,
            room: home,
            since: ts,
            expected_secs: None,
            pose: Pose::Sit,
            confidence,
        };
        match outcome {
            ToolOutcome::Started => {
                e.background.push(call.clone());
                cues.push(Cue::Paper { agent, on: true });
                e.set_activity(activity(sentence::background(&call), Confidence::Confirmed));
            }
            ToolOutcome::Ok if confirmed => {
                let secs = (ts - call.started).num_milliseconds() as f32 / 1000.0;
                if let Some(key) = &call.summary.command_key {
                    self.stats.record_duration(key, secs);
                }
                self.stats.record_ok(call.summary.command_norm.as_deref());
                let e = &mut self.employees[i];
                match (&call.tool, declares) {
                    (ToolKind::Test, true) => {
                        self.props.light = Light::Green;
                        let t = test_result.unwrap_or(TestResult {
                            passed: 0,
                            failed: 0,
                        });
                        cues.push(Cue::Stamp {
                            passed: t.passed,
                            failed: t.failed,
                            estimated: false,
                        });
                        // 테스트 전부 통과는 장애 해제 조건이다 (4.7.3)
                        if self.incident.take().is_some() {
                            cues.push(Cue::Siren { on: false });
                        }
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
                    (ToolKind::Build, true) => self.props.light = Light::Green,
                    (ToolKind::GitPush | ToolKind::PrCreate, true) => cues.push(Cue::Truck),
                    (ToolKind::Release, true) => cues.push(Cue::Fireworks),
                    _ => {}
                }
                e.set_activity(activity(
                    sentence::done(&call, declares),
                    Confidence::Confirmed,
                ));
            }
            ToolOutcome::Err if confirmed => {
                e.counters.errors += 1;
                cues.push(Cue::Shake { agent });
                self.props.trash += 1;
                self.props.safe_since = ts;
                match &call.tool {
                    ToolKind::Build => self.props.light = Light::Red,
                    ToolKind::Test => {
                        self.props.light = Light::Red;
                        if let Some(t) = test_result {
                            cues.push(Cue::Stamp {
                                passed: t.passed,
                                failed: t.failed,
                                estimated: false,
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
                let e = &mut self.employees[i];
                e.set_activity(activity(
                    sentence::accident(&call, preview),
                    Confidence::Confirmed,
                ));
            }
            ToolOutcome::Rejected => {
                e.rejected_at = Some(ts);
                cues.push(Cue::Reject { agent });
                e.set_activity(activity(sentence::rejected(&call), Confidence::Confirmed));
            }
            // Unknown, 또는 본문으로 짐작한 Ok/Err. 신호등·무재해·출고·평균을 건드리지 않는다 (S59)
            _ => {
                e.set_activity(activity(sentence::unknown(&call), Confidence::Inferred));
            }
        }
        self.refresh(i, ts);
    }

    /// 허가 대기와 진행 중 호출은 다른 무엇보다 앞선다 (2.5 우선순위 1·2번)
    fn refresh(&mut self, i: usize, ts: DateTime<Utc>) {
        let boss = self.employees[i].is_boss();
        let expected = self.employees[i]
            .pending
            .last()
            .and_then(|c| c.summary.command_key.as_ref())
            .and_then(|k| self.stats.avg.get(k).copied());
        let e = &mut self.employees[i];
        if let Some(q) = &e.asking {
            let a = Activity {
                sentence: sentence::asking(q),
                room: Room::Door,
                since: ts,
                expected_secs: None,
                pose: Pose::Sit,
                confidence: Confidence::Confirmed,
            };
            e.set_activity(a);
            return;
        }
        if let Some(call) = e.pending.last() {
            let a = Activity {
                sentence: sentence::call(call, e.pending.len() - 1),
                room: room_for(&call.tool, boss),
                since: call.started,
                expected_secs: expected,
                pose: pose_for(&call.tool),
                confidence: Confidence::Confirmed,
            };
            e.set_activity(a);
        }
    }

    fn tick_employee(&mut self, i: usize, now: DateTime<Utc>, working: bool, cues: &mut Vec<Cue>) {
        let avg = &self.stats.avg;
        let e = &mut self.employees[i];
        if e.retired_at.is_some() || e.asking.is_some() {
            return;
        }
        if let Some(call) = e.pending.last().cloned() {
            let elapsed = now - call.started;
            if elapsed > CALL_GIVE_UP {
                give_up_calls(e, now);
                return;
            }
            let slow = call
                .summary
                .command_key
                .as_ref()
                .and_then(|k| avg.get(k))
                .is_some_and(|&a| elapsed.num_milliseconds() as f32 / 1000.0 > a * 2.0);
            if elapsed > LONG_RUN {
                e.activity.sentence = sentence::long_run(&call);
            }
            if slow || elapsed > LONG_RUN {
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
            let a = Activity {
                sentence: sentence::waiting_user(),
                room: Room::TeamLead,
                since: t,
                expected_secs: None,
                pose: Pose::Sit,
                confidence: Confidence::Inferred,
            };
            e.set_activity(a);
            return;
        }
        if let Some(t) = e.quiet_since.filter(|_| working) {
            let quiet = now - t;
            if quiet > THINK_GIVE_UP {
                // 응답이 끊긴 것으로 보고 졸음 규칙에 맡긴다
                e.quiet_since = None;
            } else if quiet >= THINK_AFTER {
                let duck = quiet > DUCK_AFTER;
                let room = home_room(e);
                e.set_activity(Activity {
                    sentence: sentence::thinking(duck),
                    room,
                    since: t,
                    expected_secs: None,
                    pose: if duck { Pose::Duck } else { Pose::Think },
                    confidence: Confidence::Inferred,
                });
                return;
            }
        }
        // 완료 기록을 못 본 서브: 마지막이 텍스트 응답이고 60초 무변경 (4.7.2)
        if !e.is_boss()
            && e.background.is_empty()
            && e.last_was_text
            && now - e.last_event > SUB_DONE_AFTER
        {
            retire(e, now, Confidence::Inferred, cues);
        }
    }
}

/// 여러 파일의 이벤트를 적용 순서로 늘어놓는다 (4.3 "순서"). `files[0]` 이 메인 파일이다.
/// ts 가 같거나 없으면 파일 순서, 그다음 줄 순서. ts 없는 이벤트는 같은 파일의 직전 ts 를 쓴다
pub fn merge(files: Vec<Vec<Event>>) -> Vec<Event> {
    let mut keyed = Vec::new();
    for (f, events) in files.into_iter().enumerate() {
        let mut last = None;
        for (n, ev) in events.into_iter().enumerate() {
            last = ev.ts.or(last);
            keyed.push((last, f, n, ev));
        }
    }
    keyed.sort_by_key(|k| (k.0, k.1, k.2));
    keyed.into_iter().map(|(.., ev)| ev).collect()
}

fn retire(e: &mut Employee, at: DateTime<Utc>, confidence: Confidence, cues: &mut Vec<Cue>) {
    if e.retired_at.is_some() {
        return;
    }
    e.retired_at = Some(at);
    e.set_activity(Activity {
        sentence: sentence::retired(confidence),
        room: Room::Pantry,
        since: at,
        expected_secs: None,
        pose: Pose::Sit,
        confidence,
    });
    cues.push(Cue::Retire {
        agent: e.id.clone(),
    });
}

/// 결과가 영영 오지 않는 호출을 Unknown 으로 정리한다 (4.7.2)
fn give_up_calls(e: &mut Employee, now: DateTime<Utc>) {
    let Some(last) = e.pending.last().or(e.background.last()).cloned() else {
        return;
    };
    e.pending.clear();
    e.background.clear();
    let room = home_room(e);
    e.set_activity(Activity {
        sentence: sentence::unknown(&last),
        room,
        since: now,
        expected_secs: None,
        pose: Pose::Sit,
        confidence: Confidence::Inferred,
    });
}

/// 응답 대기 중인가 (2.5 우선순위 4번)
fn thinking(e: &Employee, now: DateTime<Utc>) -> bool {
    e.pending.is_empty()
        && e.waiting_user_since.is_none()
        && e.quiet_since
            .is_some_and(|t| now - t >= THINK_AFTER && now - t <= THINK_GIVE_UP)
}

fn boss_activity(sentence: String, room: Room, at: DateTime<Utc>) -> Activity {
    Activity {
        sentence,
        room,
        since: at,
        expected_secs: None,
        pose: Pose::Sit,
        confidence: Confidence::Confirmed,
    }
}

fn home_room(e: &Employee) -> Room {
    if e.is_boss() {
        Room::TeamLead
    } else {
        Room::Dev
    }
}

/// 4.7.1 `room_for`
fn room_for(tool: &ToolKind, boss: bool) -> Room {
    match tool {
        ToolKind::FileRead | ToolKind::FileSearch | ToolKind::GitQuery => Room::Archive,
        ToolKind::FileWrite
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

fn pose_for(tool: &ToolKind) -> Pose {
    match tool {
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

fn arrival_sentence(boss: bool) -> String {
    if boss {
        sentence::arrival_boss()
    } else {
        sentence::arrival()
    }
}

/// "지금 하는 일" 문장 (3.5 표). 경과 시간은 프론트가 붙인다
mod sentence {
    use super::{Confidence, PendingCall};

    pub fn arrival_boss() -> String {
        "출근 중".to_string()
    }
    pub fn arrival() -> String {
        "출근 중 · 자리 찾는 중".to_string()
    }
    pub fn request() -> String {
        "요구사항 접수".to_string()
    }
    pub fn report(text: &str) -> String {
        format!(
            "보고서 제출함: \"{}\"",
            text.chars().take(30).collect::<String>()
        )
    }
    pub fn call(call: &PendingCall, extra: usize) -> String {
        let base = format!("{:?}", call.tool);
        if extra > 0 {
            format!("{base} (외 {extra}건)")
        } else {
            base
        }
    }
    pub fn background(call: &PendingCall) -> String {
        format!("백그라운드: {:?}", call.tool)
    }
    pub fn done(call: &PendingCall, _declares: bool) -> String {
        format!("{:?} 완료", call.tool)
    }
    pub fn accident(_call: &PendingCall, preview: &str) -> String {
        format!("사고: {}", preview.lines().next().unwrap_or_default())
    }
    pub fn rejected(call: &PendingCall) -> String {
        format!("반려됨: {:?}", call.tool)
    }
    pub fn unknown(call: &PendingCall) -> String {
        format!("결과 확인 불가: {:?}", call.tool)
    }
    pub fn long_run(call: &PendingCall) -> String {
        format!("서버실에서 땀 흘리는 중 ({:?})", call.tool)
    }
    pub fn asking(q: &str) -> String {
        format!("결재판 들고 문 앞 대기: 「{q}」")
    }
    pub fn thinking(duck: bool) -> String {
        if duck {
            "러버덕과 상담 중인 듯"
        } else {
            "턱 괴고 고민 중인 듯"
        }
        .to_string()
    }
    pub fn waiting_user() -> String {
        "요구사항 대기 중인 듯".to_string()
    }
    pub fn dozing() -> String {
        "졸음".to_string()
    }
    pub fn retired(c: Confidence) -> String {
        match c {
            Confidence::Confirmed => "탕비실에서 쉬는 중".to_string(),
            Confidence::Inferred => "탕비실에서 게임 중인 듯".to_string(),
        }
    }
    pub fn repot() -> String {
        "분갈이 중 (서류 정리)".to_string()
    }
    pub fn repot_say() -> String {
        "분갈이".to_string()
    }
}
