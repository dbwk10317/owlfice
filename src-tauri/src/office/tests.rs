//! 상태 기계 단위 테스트(4.12)와 fixture 스냅샷. 기대값은 PLAN 규칙으로 손으로 정했다

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, Utc};

use super::classify::{SubMeta, merge, rebuild, replay};
use super::{Cue, Employee, Light, Pose, Rank, Room, Team, TeamPhase};
use crate::discover::Product;
use crate::parse::claude::ClaudeParser;
use crate::parse::codex::CodexParser;
use crate::parse::{
    Compound, Confidence, Event, EventKind, Parser, RawRef, TestResult, ToolKind, ToolOutcome,
    ToolSummary,
};

use Confidence::{Confirmed, Inferred};

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

// ---------- 합성 이벤트 ----------

const MAIN: &str = "main";
const SUB: &str = "sub1";

fn t0() -> DateTime<Utc> {
    "2026-03-02T09:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> DateTime<Utc> {
    t0() + Duration::seconds(secs)
}

fn ev(agent: &str, secs: i64, kind: EventKind) -> Event {
    Event {
        ts: Some(at(secs)),
        session_id: MAIN.into(),
        agent_id: agent.into(),
        parent_agent: (agent != MAIN).then(|| MAIN.to_string()),
        kind,
        raw_ref: RawRef {
            path: PathBuf::new(),
            generation: 0,
            byte_offset: 0,
            line_no: 0,
        },
    }
}

fn summary() -> ToolSummary {
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

fn cmd(c: &str) -> ToolSummary {
    ToolSummary {
        command: Some(c.into()),
        command_key: Some(c.split(' ').take(2).collect::<Vec<_>>().join(" ")),
        command_norm: Some(c.into()),
        ..summary()
    }
}

fn file(p: &str) -> ToolSummary {
    ToolSummary {
        path: Some(p.into()),
        ..summary()
    }
}

fn call(id: &str, tool: ToolKind, s: ToolSummary) -> EventKind {
    EventKind::ToolCall {
        call_id: id.into(),
        tool,
        summary: s,
    }
}

fn result(id: &str, outcome: ToolOutcome) -> EventKind {
    EventKind::ToolResult {
        call_id: id.into(),
        outcome,
        outcome_confidence: Confirmed,
        preview: String::new(),
        test_result: None,
    }
}

/// 이벤트마다 적용하기 전에 그 ts 로 tick 한다 (재구성과 같은 시계)
struct Sim {
    team: Team,
    cues: Vec<Cue>,
}

impl Sim {
    fn new() -> Self {
        Self {
            team: Team::new(MAIN, Product::ClaudeCli, t0()),
            cues: Vec::new(),
        }
    }

    fn on(&mut self, agent: &str, secs: i64, kind: EventKind) -> &mut Self {
        self.tick(secs);
        self.team.apply(&ev(agent, secs, kind), &mut self.cues);
        self
    }

    fn tick(&mut self, secs: i64) -> &mut Self {
        self.team.tick(at(secs), utc(), &mut self.cues);
        self
    }

    fn boss(&self) -> &Employee {
        &self.team.employees[0]
    }

    fn sub(&self) -> &Employee {
        emp(&self.team, SUB)
    }
}

fn emp<'a>(t: &'a Team, id: &str) -> &'a Employee {
    t.employees
        .iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("직원 {id} 없음"))
}

#[track_caller]
fn is(e: &Employee, room: Room, sentence: &str, confidence: Confidence) {
    let a = &e.activity;
    assert_eq!(
        (a.room, a.sentence.as_str(), a.confidence),
        (room, sentence, confidence),
        "직원 {}",
        e.id
    );
}

// ---------- 4.12 단위 ----------

#[test]
fn unpaired_results_are_ignored() {
    let mut s = Sim::new();
    s.on(MAIN, 1, result("nope", ToolOutcome::Err))
        .on(MAIN, 2, result("nope", ToolOutcome::Ok))
        .on(MAIN, 3, call("c1", ToolKind::Shell, cmd("ls")))
        .on(MAIN, 4, result("c1", ToolOutcome::Ok))
        // 이미 처리한 결과의 중복
        .on(MAIN, 5, result("c1", ToolOutcome::Err));
    assert_eq!(s.team.props.trash, 0);
    assert_eq!(s.boss().counters.errors, 0);
    assert!(s.team.incident.is_none());
    assert!(!s.cues.iter().any(|c| matches!(c, Cue::Shake { .. })));
    is(s.boss(), Room::TeamLead, "`ls` 완료", Confirmed);
}

fn fail(s: &mut Sim, id: &str, secs: i64, c: &str) {
    s.on(MAIN, secs, call(id, ToolKind::Shell, cmd(c))).on(
        MAIN,
        secs,
        result(id, ToolOutcome::Err),
    );
}

#[test]
fn incident_same_command_three_in_a_row() {
    // 61초 간격이라 "60초 3건" 규칙은 걸리지 않는다
    let mut s = Sim::new();
    fail(&mut s, "a", 0, "cargo build");
    fail(&mut s, "b", 61, "cargo build");
    assert!(s.team.incident.is_none());
    fail(&mut s, "c", 122, "cargo build");
    assert!(s.team.incident.is_some());
    s.tick(123);
    is(
        s.boss(),
        Room::Incident,
        "장애 상황실 소집, 무재해 기록 0분",
        Confirmed,
    );

    // 성공이 사이에 끼면 연속이 끊긴다
    let mut s = Sim::new();
    fail(&mut s, "a", 0, "cargo build");
    s.on(MAIN, 61, call("ok", ToolKind::Shell, cmd("cargo build")))
        .on(MAIN, 61, result("ok", ToolOutcome::Ok));
    fail(&mut s, "b", 122, "cargo build");
    fail(&mut s, "c", 183, "cargo build");
    assert!(s.team.incident.is_none());
}

#[test]
fn incident_three_errors_in_a_minute_then_clears_after_120s() {
    let mut s = Sim::new();
    fail(&mut s, "a", 0, "ls a");
    fail(&mut s, "b", 20, "ls b");
    assert!(s.team.incident.is_none());
    fail(&mut s, "c", 40, "ls c");
    assert!(s.team.incident.is_some());
    s.tick(160);
    assert!(s.team.incident.is_some(), "120초 무에러 전");
    s.tick(161);
    assert!(s.team.incident.is_none());
    assert_eq!(s.boss().activity.room, Room::TeamLead);
    assert!(s.boss().recent.iter().any(|a| a.sentence == "장애 해제"));
}

#[test]
fn incident_ignores_rejected_unknown_and_inferred() {
    let mut s = Sim::new();
    let outcomes = [
        ToolOutcome::Rejected,
        ToolOutcome::Rejected,
        ToolOutcome::Rejected,
        ToolOutcome::Unknown,
        ToolOutcome::Unknown,
        ToolOutcome::Unknown,
    ];
    for (n, outcome) in outcomes.into_iter().enumerate() {
        let id = format!("c{n}");
        s.on(MAIN, n as i64, call(&id, ToolKind::Shell, cmd("make")))
            .on(MAIN, n as i64, result(&id, outcome));
    }
    for n in 10..13 {
        let id = format!("i{n}");
        let inferred_err = EventKind::ToolResult {
            call_id: id.clone(),
            outcome: ToolOutcome::Err,
            outcome_confidence: Inferred,
            preview: "error".into(),
            test_result: None,
        };
        s.on(MAIN, n, call(&id, ToolKind::Shell, cmd("make")))
            .on(MAIN, n, inferred_err);
    }
    assert!(s.team.incident.is_none());
    assert_eq!(s.team.props.trash, 0);
    assert_eq!(s.team.props.safe_since, t0());
}

#[test]
fn incident_clears_when_tests_pass() {
    let mut s = Sim::new();
    for (n, id) in ["a", "b", "c"].into_iter().enumerate() {
        fail(&mut s, id, n as i64, "ls");
    }
    assert!(s.team.incident.is_some());
    s.on(MAIN, 10, call("t", ToolKind::Test, cmd("cargo test")))
        .on(MAIN, 30, result("t", ToolOutcome::Ok));
    assert!(s.team.incident.is_none());
    assert_eq!(s.team.props.light, Light::Green);
}

#[test]
fn pending_reversed_started_and_unknown() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("a", ToolKind::FileRead, file("a.rs")))
        .on(MAIN, 1, call("b", ToolKind::FileRead, file("b.rs")));
    is(
        s.boss(),
        Room::Archive,
        "`b.rs` 열람 중 (외 1건)",
        Confirmed,
    );
    s.on(MAIN, 2, result("b", ToolOutcome::Ok));
    is(s.boss(), Room::Archive, "`a.rs` 열람 중", Confirmed);
    s.on(MAIN, 2, result("a", ToolOutcome::Ok));
    assert!(s.boss().pending.is_empty());
    is(s.boss(), Room::TeamLead, "`a.rs` 열람", Confirmed);

    s.on(MAIN, 3, call("bg", ToolKind::Shell, cmd("npm run dev")))
        .on(MAIN, 4, result("bg", ToolOutcome::Started));
    assert_eq!(s.boss().background.len(), 1);
    is(
        s.boss(),
        Room::TeamLead,
        "백그라운드: `npm run dev` 돌아가는 중 (작업표 1장)",
        Confirmed,
    );

    s.on(MAIN, 5, call("u", ToolKind::Shell, cmd("./deploy.sh")))
        .on(MAIN, 6, result("u", ToolOutcome::Unknown));
    is(
        s.boss(),
        Room::TeamLead,
        "결과 확인 불가: `./deploy.sh`",
        Inferred,
    );
    assert_eq!(s.team.props.light, Light::Off);
    assert_eq!(s.team.props.trash, 0);
    assert!(!s.team.stats.avg.contains_key("./deploy.sh"));

    // 완료 알림
    s.on(MAIN, 20, result("bg", ToolOutcome::Ok));
    assert!(s.boss().background.is_empty());
    assert!(s.cues.contains(&Cue::Paper {
        agent: MAIN.into(),
        on: false
    }));
}

#[test]
fn mixed_success_sends_no_truck_and_no_light() {
    let mixed = |c: &str| ToolSummary {
        compound: Compound::Mixed,
        ..cmd(c)
    };
    let tested = EventKind::ToolResult {
        call_id: "t".into(),
        outcome: ToolOutcome::Ok,
        outcome_confidence: Confirmed,
        preview: String::new(),
        test_result: Some(TestResult {
            passed: 3,
            failed: 1,
        }),
    };
    let mut s = Sim::new();
    s.on(
        MAIN,
        0,
        call("p", ToolKind::GitPush, mixed("git push; echo")),
    )
    .on(MAIN, 1, result("p", ToolOutcome::Ok))
    .on(
        MAIN,
        2,
        call("b", ToolKind::Build, mixed("cargo build | tee")),
    )
    .on(MAIN, 3, result("b", ToolOutcome::Ok))
    .on(
        MAIN,
        4,
        call("t", ToolKind::Test, mixed("cargo test | tail")),
    )
    .on(MAIN, 5, tested);
    assert!(!s.cues.contains(&Cue::Truck));
    assert_eq!(s.team.props.light, Light::Off);
    assert!(s.cues.contains(&Cue::Stamp {
        passed: 3,
        failed: 1,
        estimated: true
    }));
    is(
        s.boss(),
        Room::TeamLead,
        "`cargo test | tail` 완료",
        Confirmed,
    );
}

fn spawn_sub(s: &mut Sim, secs: i64, model: &str, effort: Option<&str>) {
    let spawn = EventKind::SubagentSpawn {
        child_id: SUB.into(),
        prompt: "긴 지시문".into(),
        model: Some(model.into()),
        description: Some("로그 포맷 조사해줘".into()),
        effort: effort.map(str::to_string),
        depth: 1,
    };
    s.on(MAIN, secs, spawn);
}

#[test]
fn sub_token_usage_keeps_the_plant() {
    let usage = |input_total, window| EventKind::TokenUsage {
        input_total,
        output: 0,
        context_window: Some(window),
    };
    let mut s = Sim::new();
    spawn_sub(&mut s, 0, "claude-haiku-4-5", None);
    s.on(MAIN, 1, usage(100_000, 1_000_000))
        .on(SUB, 2, usage(20_000, 200_000));
    assert_eq!(s.team.props.plant, Some(0.1));
    assert_eq!(s.sub().context_used, Some(20_000));
}

#[test]
fn sub_user_message_is_not_a_boss_visit() {
    let mut s = Sim::new();
    spawn_sub(&mut s, 0, "claude-haiku-4-5", None);
    let before = s.boss().activity.sentence.clone();
    s.cues.clear();
    s.on(
        SUB,
        1,
        EventKind::UserMessage {
            text: "지시".into(),
        },
    );
    assert!(s.team.props.boss_visit.is_none());
    assert!(!s.cues.iter().any(|c| matches!(c, Cue::BossVisit { .. })));
    assert_eq!(s.boss().activity.sentence, before);
}

// ---------- 뼈대가 비운 칸 ----------

#[test]
fn spawn_rank_uses_effort_and_sentence_uses_description() {
    let mut s = Sim::new();
    spawn_sub(&mut s, 0, "gpt-5", Some("high"));
    assert_eq!(s.sub().rank, Rank::Gwajang);
    is(
        s.boss(),
        Room::TeamLead,
        "과장에게 \"로그 포맷 조사해줘\" 지시",
        Confirmed,
    );
}

#[test]
fn codex_escalated_call_waits_at_the_door_after_35s() {
    let escalated = ToolSummary {
        escalated: true,
        ..cmd("rm -rf dist")
    };
    let mut s = Sim::new();
    s.on(MAIN, 0, call("x", ToolKind::Shell, escalated));
    s.tick(34);
    assert_eq!(s.boss().activity.room, Room::Build);
    s.tick(35);
    is(
        s.boss(),
        Room::Door,
        "`rm -rf dist` 결재판 들고 문 앞 대기 중인 듯",
        Inferred,
    );
    assert!(s.team.needs_boss(s.boss(), at(35)));
    s.on(MAIN, 40, result("x", ToolOutcome::Ok));
    assert_eq!(s.boss().activity.room, Room::TeamLead);
    assert!(!s.team.needs_boss(s.boss(), at(40)));
}

#[test]
fn night_and_pizza() {
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    let mut team = Team::new(MAIN, Product::ClaudeCli, t0());
    let mut cues = Vec::new();
    let mut step = |team: &mut Team, secs: i64| {
        team.apply(&ev(MAIN, secs, EventKind::Thinking), &mut cues);
        team.tick(at(secs), kst, &mut cues);
    };
    // 09:00Z 는 서울 18시
    step(&mut team, 0);
    assert!(!team.props.night);
    let h = 3600;
    for (secs, pizza) in [(2 * h, 0), (2 * h + 60, 1), (3 * h, 1), (3 * h + 60, 2)] {
        step(&mut team, secs);
        assert_eq!(team.props.pizza, pizza, "{secs}초");
    }
    step(&mut team, 4 * h);
    assert!(team.props.night, "22시");
}

#[test]
fn going_off_clears_background_jobs() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("bg", ToolKind::Shell, cmd("npm run dev")))
        .on(MAIN, 1, result("bg", ToolOutcome::Started));
    s.cues.clear();
    s.tick(1 + 31 * 60);
    assert_eq!(s.team.phase, TeamPhase::Off);
    assert!(s.boss().background.is_empty());
    assert!(s.cues.contains(&Cue::Paper {
        agent: MAIN.into(),
        on: false
    }));
    is(
        s.boss(),
        Room::TeamLead,
        "결과 확인 불가: `npm run dev`",
        Inferred,
    );
}

#[test]
fn review_pens_and_repeated_edits() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("d", ToolKind::GitDiff, cmd("git diff")));
    is(s.boss(), Room::Review, "PR 리뷰 중 · 빨간펜 0개", Confirmed);
    let review = EventKind::AssistantText {
        text: "`a.rs:3` 경계 오류, src/b.rs:10 도 같음. 회의는 12:30".into(),
    };
    s.on(MAIN, 1, result("d", ToolOutcome::Ok))
        .on(MAIN, 2, review);
    is(s.boss(), Room::Review, "PR 리뷰 중 · 빨간펜 2개", Confirmed);

    let edit = || ToolSummary {
        lines_changed: Some(2),
        ..file("a.rs")
    };
    s.on(MAIN, 10, call("e1", ToolKind::FileEdit, edit()));
    is(
        s.boss(),
        Room::TeamLead,
        "`a.rs` 수정 중 (2줄 교체)",
        Confirmed,
    );
    s.on(MAIN, 11, result("e1", ToolOutcome::Ok))
        .on(MAIN, 20, call("e2", ToolKind::FileEdit, edit()))
        .on(MAIN, 21, result("e2", ToolOutcome::Ok))
        .on(MAIN, 30, call("e3", ToolKind::FileEdit, edit()));
    is(
        s.boss(),
        Room::TeamLead,
        "`a.rs` 리팩터링 중 (오늘 3번째 수정)",
        Confirmed,
    );
}

fn text(t: &str) -> EventKind {
    EventKind::AssistantText { text: t.into() }
}

fn with_tests(id: &str, outcome: ToolOutcome, passed: u32, failed: u32) -> EventKind {
    EventKind::ToolResult {
        call_id: id.into(),
        outcome,
        outcome_confidence: Confirmed,
        preview: String::new(),
        test_result: Some(TestResult { passed, failed }),
    }
}

fn in_file(mut e: Event, path: &str) -> Event {
    e.raw_ref.path = path.into();
    e
}

fn ask(question: &str, plan: bool) -> EventKind {
    EventKind::PermissionRequest {
        question: question.into(),
        plan,
    }
}

#[test]
fn rebuild_ticks_before_each_event() {
    // 백그라운드는 40분 뒤 메시지보다 먼저 팀 Off 로 정리된다
    let bg = [
        ev(MAIN, 0, call("bg", ToolKind::Shell, cmd("npm run dev"))),
        ev(MAIN, 1, result("bg", ToolOutcome::Started)),
        ev(
            MAIN,
            40 * 60,
            EventKind::UserMessage {
                text: "다시".into(),
            },
        ),
    ];
    // 30분 넘게 결과가 없어 포기한 호출이라 늦은 실패는 짝이 없다
    let build = [
        ev(MAIN, 0, call("b", ToolKind::Build, cmd("cargo build"))),
        ev(MAIN, 31 * 60, result("b", ToolOutcome::Err)),
    ];
    for events in [&bg[..], &build[..]] {
        let end = events.last().unwrap().ts.unwrap();
        let t = replayed(MAIN, events, end);
        assert_eq!(final_state(&t), final_state(&stepwise(MAIN, events, end)));
        assert!(boss(&t).background.is_empty());
        assert_eq!(t.props.trash, 0);
    }
    // 포기한 뒤 졸음을 거쳐 퇴근했다
    let t = replayed(MAIN, &build, at(31 * 60));
    let given_up = &boss(&t).recent[0];
    assert_eq!(given_up.sentence, "결과 확인 불가: `cargo build`");
    assert_eq!(given_up.since, at(30 * 60));
    assert_eq!(t.props.light, Light::Off);
}

#[test]
fn merge_keeps_line_order_within_a_file() {
    // 결과 줄의 ts 가 호출보다 이르다 (표본의 queued_command 처럼)
    let main = vec![
        ev(MAIN, 10, call("c", ToolKind::Shell, cmd("ls"))),
        ev(MAIN, 9, result("c", ToolOutcome::Ok)),
    ];
    let sub = vec![ev(SUB, 9, EventKind::Thinking)];
    let order = merge(vec![main.clone(), sub.clone()]);
    assert_eq!(order, [sub[0].clone(), main[0].clone(), main[1].clone()]);
    let t = rebuild(MAIN, Product::ClaudeCli, &[], vec![main, sub], utc());
    assert!(boss(&t).pending.is_empty());
    is(boss(&t), Room::TeamLead, "`ls` 완료", Confirmed);
    assert_eq!(
        final_state(&replayed(MAIN, &order, at(20))),
        final_state(&stepwise(MAIN, &order, at(20)))
    );
}

#[test]
fn retired_sub_rehires_only_for_later_events() {
    let mut s = Sim::new();
    spawn_sub(&mut s, 0, "claude-haiku-4-5", None);
    s.on(SUB, 5, text("끝")).on(
        MAIN,
        10,
        EventKind::SubagentDone {
            child_id: SUB.into(),
        },
    );
    s.cues.clear();
    // 서브 파일의 마지막 줄을 부모의 Done 보다 늦게 읽었다
    s.on(SUB, 6, text("늦은 줄"));
    let untimed = Event {
        ts: None,
        ..ev(SUB, 0, EventKind::Thinking)
    };
    s.team.apply(&untimed, &mut s.cues);
    // meta.json 선발견처럼 앞선 ts 의 중복 Spawn
    spawn_sub(&mut s, 3, "claude-haiku-4-5", None);
    s.tick(200);
    assert_eq!(s.sub().retired_at, Some(at(10)));
    assert!(!s.cues.iter().any(|c| matches!(c, Cue::Rehire { .. })));
    // 재출근하지 못한 옛 줄은 상태를 바꾸지 않는다
    is(s.sub(), Room::Pantry, "탕비실에서 쉬는 중", Confirmed);
    assert_eq!(s.sub().last_say.as_deref(), Some("끝"));
    s.on(SUB, 201, EventKind::Thinking);
    assert!(s.sub().retired_at.is_none());
    assert!(s.cues.contains(&Cue::Rehire { agent: SUB.into() }));
}

#[test]
fn untimed_events_use_their_own_file_clock() {
    let untimed = |agent: &str, path: &str| Event {
        ts: None,
        ..in_file(ev(agent, 0, EventKind::Thinking), path)
    };
    let apply = |s: &mut Sim, e: Event| s.team.apply(&e, &mut s.cues);
    let mut s = Sim::new();
    spawn_sub(&mut s, 0, "claude-haiku-4-5", None);
    apply(
        &mut s,
        in_file(ev(SUB, 5, EventKind::Thinking), "sub.jsonl"),
    );
    apply(
        &mut s,
        in_file(ev(MAIN, 100, EventKind::Thinking), "main.jsonl"),
    );
    apply(&mut s, untimed(SUB, "sub.jsonl"));
    assert_eq!(s.sub().last_event, at(5));
    // 조는 팀은 ts 없는 줄이나 늦게 읽힌 옛 줄로 깨지 않는다
    s.tick(100 + 4 * 60);
    assert_eq!(s.team.phase, TeamPhase::Dozing);
    apply(&mut s, untimed(MAIN, "main.jsonl"));
    apply(
        &mut s,
        in_file(ev(SUB, 50, EventKind::Thinking), "sub.jsonl"),
    );
    assert_eq!(s.team.phase, TeamPhase::Dozing);
    apply(
        &mut s,
        in_file(ev(SUB, 400, EventKind::Thinking), "sub.jsonl"),
    );
    assert_eq!(s.team.phase, TeamPhase::Working);
}

#[test]
fn retiring_settles_calls_so_the_team_can_doze() {
    let mut s = Sim::new();
    s.team.alive_in_registry = true;
    spawn_sub(&mut s, 0, "claude-haiku-4-5", None);
    s.on(SUB, 1, call("x", ToolKind::Shell, cmd("sleep 999")))
        .on(
            MAIN,
            2,
            EventKind::SubagentDone {
                child_id: SUB.into(),
            },
        )
        .tick(2 * 3600);
    assert!(s.sub().pending.is_empty());
    assert_eq!(s.sub().recent[0].sentence, "결과 확인 불가: `sleep 999`");
    assert_eq!(s.team.phase, TeamPhase::Dozing);
}

#[test]
fn late_call_give_up_keeps_background_jobs() {
    let mut s = Sim::new();
    s.team.alive_in_registry = true;
    s.on(MAIN, 0, call("bg", ToolKind::Shell, cmd("npm run dev")))
        .on(MAIN, 1, result("bg", ToolOutcome::Started))
        .on(MAIN, 2, call("p", ToolKind::Shell, cmd("sleep 9999")))
        .tick(2 + 31 * 60);
    assert!(s.boss().pending.is_empty());
    assert_eq!(s.boss().background.len(), 1);
    is(
        s.boss(),
        Room::TeamLead,
        "결과 확인 불가: `sleep 9999`",
        Inferred,
    );
    assert_eq!(s.boss().activity.since, at(2 + 30 * 60));
}

#[test]
fn every_pending_call_counts_for_give_up_and_squat() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("a", ToolKind::Shell, cmd("sleep 9999")))
        .on(MAIN, 20 * 60, call("b", ToolKind::Shell, cmd("ls -R")))
        .tick(31 * 60);
    let ids: Vec<_> = s
        .boss()
        .pending
        .iter()
        .map(|c| c.call_id.as_str())
        .collect();
    assert_eq!(ids, ["b"]);
    is(
        s.boss(),
        Room::Build,
        "서버실에서 땀 흘리는 중 (`ls -R`)",
        Confirmed,
    );

    // 평균 10초인 빌드가 대표 호출 뒤에 있어도 쪼그린다
    let mut s = Sim::new();
    s.on(MAIN, 0, call("w", ToolKind::Build, cmd("cargo build")))
        .on(MAIN, 10, result("w", ToolOutcome::Ok))
        .on(MAIN, 100, call("b", ToolKind::Build, cmd("cargo build")))
        .on(MAIN, 101, call("l", ToolKind::Shell, cmd("ls")))
        .tick(125);
    assert_eq!(s.boss().activity.pose, Pose::Squat);
}

#[test]
fn long_run_is_told_by_the_oldest_call() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("a", ToolKind::Shell, cmd("cargo run")))
        .on(MAIN, 100, call("b", ToolKind::FileRead, file("a.rs")))
        .tick(130);
    assert_eq!(
        s.boss().activity.sentence,
        "서버실에서 땀 흘리는 중 (`cargo run`)"
    );
    assert_eq!(s.boss().activity.pose, Pose::Squat);
}

#[test]
fn question_waits_past_the_call_give_up() {
    let mut s = Sim::new();
    s.team.alive_in_registry = true;
    s.on(MAIN, 0, call("q", ToolKind::Ask, summary()))
        .on(MAIN, 0, ask("배포할까요?", false))
        .on(
            MAIN,
            5,
            EventKind::UserMessage {
                text: "잠깐".into(),
            },
        );
    // 같은 허가 대기가 이어지면 경과를 처음부터 세지 않는다
    assert_eq!(s.boss().activity.since, at(0));
    s.tick(40 * 60);
    assert_eq!(s.boss().pending.len(), 1);
    is(
        s.boss(),
        Room::Door,
        "결재판 들고 문 앞 대기: 「배포할까요?」",
        Confirmed,
    );
    assert!(s.team.needs_boss(s.boss(), at(40 * 60)));
}

#[test]
fn going_off_drops_the_question() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("q", ToolKind::Ask, summary()))
        .on(MAIN, 0, ask("배포할까요?", false));
    assert!(s.team.needs_boss(s.boss(), at(1)));
    s.tick(31 * 60);
    assert_eq!(s.team.phase, TeamPhase::Off);
    assert!(s.boss().asking.is_none());
    assert!(!s.team.needs_boss(s.boss(), at(31 * 60)));
}

#[test]
fn completion_after_turn_end_is_not_waiting_for_the_boss() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("bg", ToolKind::Shell, cmd("npm run dev")))
        .on(MAIN, 1, result("bg", ToolOutcome::Started))
        .on(MAIN, 2, text("돌려 둘게요"))
        .on(MAIN, 30, result("bg", ToolOutcome::Ok))
        .tick(33);
    is(s.boss(), Room::TeamLead, "턱 괴고 고민 중인 듯", Inferred);
    assert!(!s.team.needs_boss(s.boss(), at(33)));

    let mut s = Sim::new();
    spawn_sub(&mut s, 0, "claude-haiku-4-5", None);
    s.on(MAIN, 1, text("맡겨 뒀어요"))
        .on(
            MAIN,
            30,
            EventKind::SubagentDone {
                child_id: SUB.into(),
            },
        )
        .tick(33);
    is(s.boss(), Room::TeamLead, "턱 괴고 고민 중인 듯", Inferred);
    assert!(!s.team.needs_boss(s.boss(), at(33)));
}

#[test]
fn light_returns_when_a_build_ends_without_a_verdict() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("g", ToolKind::Build, cmd("cargo build")))
        .on(MAIN, 1, result("g", ToolOutcome::Ok));
    assert_eq!(s.team.props.light, Light::Green);
    let guessed = EventKind::ToolResult {
        call_id: "i".into(),
        outcome: ToolOutcome::Ok,
        outcome_confidence: Inferred,
        preview: String::new(),
        test_result: None,
    };
    let ends = [
        result("i", ToolOutcome::Rejected),
        result("i", ToolOutcome::Unknown),
        guessed,
    ];
    for (n, end) in ends.into_iter().enumerate() {
        let t = 10 + n as i64 * 10;
        s.on(MAIN, t, call("i", ToolKind::Test, cmd("cargo test")));
        assert_eq!(s.team.props.light, Light::Yellow);
        s.on(MAIN, t + 1, end);
        assert_eq!(s.team.props.light, Light::Green, "{n}번째");
    }
    // 30분 포기도 같다
    s.on(MAIN, 50, call("z", ToolKind::Build, cmd("cargo build")))
        .tick(50 + 31 * 60);
    assert_eq!(s.team.props.light, Light::Green);
}

#[test]
fn messages_do_not_hide_a_question_or_a_running_call() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("q", ToolKind::Ask, summary()))
        .on(MAIN, 0, ask("배포할까요?", false))
        .on(
            MAIN,
            1,
            EventKind::UserMessage {
                text: "잠깐".into(),
            },
        );
    is(
        s.boss(),
        Room::Door,
        "결재판 들고 문 앞 대기: 「배포할까요?」",
        Confirmed,
    );

    let mut s = Sim::new();
    s.on(MAIN, 0, call("c", ToolKind::Shell, cmd("make")))
        .on(MAIN, 1, text("빌드 중입니다"));
    is(s.boss(), Room::Build, "`make` 실행 중", Confirmed);
    let compaction = EventKind::ContextCompaction {
        pre_tokens: Some(150_000),
        post_tokens: Some(20_000),
    };
    s.on(MAIN, 2, compaction);
    is(s.boss(), Room::Build, "`make` 실행 중", Confirmed);
}

#[test]
fn plan_request_uses_the_plan_sentence() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("p", ToolKind::Ask, summary())).on(
        MAIN,
        0,
        ask("# 계획\n1. 파서 고치기", true),
    );
    is(
        s.boss(),
        Room::Door,
        "결재판 들고 문 앞 대기: 「계획 결재 올립니다」",
        Confirmed,
    );
}

#[test]
fn pizza_counts_from_the_last_wake() {
    let step = |s: &mut Sim, secs: i64| {
        s.on(MAIN, secs, EventKind::Thinking).tick(secs);
    };
    let h = 3600;
    let mut s = Sim::new();
    for m in (0..=180).step_by(20) {
        step(&mut s, m * 60);
    }
    step(&mut s, 3 * h + 60);
    assert_eq!(s.team.props.pizza, 2);
    s.tick(3 * h + 60 + 31 * 60);
    assert_eq!(s.team.phase, TeamPhase::Off);
    // 깨어난 때부터 다시 센다. 세션 시작부터 세면 여기서 4상자가 된다
    step(&mut s, 5 * h);
    assert_eq!(s.team.props.pizza, 2);
    for m in (5 * 60..=7 * 60).step_by(20) {
        step(&mut s, m * 60);
    }
    assert_eq!(s.team.props.pizza, 2);
    step(&mut s, 7 * h + 60);
    assert_eq!(s.team.props.pizza, 3);
}

#[test]
fn failed_test_stamp_is_estimated_only_for_mixed() {
    let mixed = ToolSummary {
        compound: Compound::Mixed,
        ..cmd("cargo test | tail")
    };
    let mut s = Sim::new();
    s.on(MAIN, 0, call("s", ToolKind::Test, cmd("cargo test")))
        .on(MAIN, 1, with_tests("s", ToolOutcome::Err, 1, 2))
        .on(MAIN, 2, call("m", ToolKind::Test, mixed))
        .on(MAIN, 3, with_tests("m", ToolOutcome::Err, 1, 2));
    let stamps: Vec<_> = s
        .cues
        .iter()
        .filter_map(|c| match c {
            Cue::Stamp { estimated, .. } => Some(*estimated),
            _ => None,
        })
        .collect();
    assert_eq!(stamps, [false, true]);
}

#[test]
fn only_passing_tests_clear_red_and_incident() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("f", ToolKind::Test, cmd("cargo test")))
        .on(MAIN, 1, result("f", ToolOutcome::Err));
    fail(&mut s, "a", 2, "ls a");
    fail(&mut s, "b", 3, "ls b");
    assert!(s.team.incident.is_some());
    // 종료 코드가 0 이어도 본문에 반려가 있으면 장애는 이어진다
    s.on(MAIN, 10, call("t", ToolKind::Test, cmd("cargo test")))
        .on(MAIN, 11, with_tests("t", ToolOutcome::Ok, 3, 1));
    assert!(s.team.incident.is_some());
    assert!(s.boss().red.is_some());
    // Mixed 라도 본문이 전부 통과면 빨간불을 끄고 장애를 푼다
    let mixed = ToolSummary {
        compound: Compound::Mixed,
        ..cmd("cargo test | tail")
    };
    s.on(MAIN, 20, call("m", ToolKind::Test, mixed)).on(
        MAIN,
        21,
        with_tests("m", ToolOutcome::Ok, 4, 0),
    );
    assert!(s.boss().red.is_none());
    assert!(s.team.incident.is_none());
}

#[test]
fn background_runs_stay_out_of_the_average() {
    let mut s = Sim::new();
    s.on(MAIN, 0, call("bg", ToolKind::Build, cmd("cargo build")))
        .on(MAIN, 1, result("bg", ToolOutcome::Started))
        .on(MAIN, 600, result("bg", ToolOutcome::Ok));
    assert!(s.team.stats.avg.is_empty());
}

// ---------- fixture 재구성 ----------

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(rel)
}

/// TailReader처럼 `\n` 으로 자르고 `\r` 을 뗀 줄을 넣는다
fn parse_file(path: &Path, p: &mut dyn Parser) -> Vec<Event> {
    let bytes = std::fs::read(path).unwrap();
    let (mut out, mut unhandled) = (Vec::new(), Vec::new());
    for (i, line) in bytes.split(|&b| b == b'\n').enumerate() {
        let raw = RawRef {
            path: path.to_path_buf(),
            generation: 0,
            byte_offset: 0,
            line_no: i as u64 + 1,
        };
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        out.extend(p.parse_line(line, &raw, &mut unhandled));
    }
    out
}

fn jsonl_in(dir: &Path) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .unwrap()
}

struct Case {
    id: String,
    product: Product,
    subs: Vec<SubMeta>,
    /// 메인 먼저
    files: Vec<Vec<Event>>,
}

impl Case {
    fn claude(scenario: &str) -> Self {
        let main = jsonl_in(&fixture(&format!("claude/{scenario}")));
        let id = main.file_stem().unwrap().to_str().unwrap().to_string();
        let mut files = vec![parse_file(&main, &mut ClaudeParser::new(0, None))];
        let mut subs = Vec::new();
        let dir = main.with_extension("").join("subagents");
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_str().unwrap();
            let Some(agent) = name
                .strip_prefix("agent-")
                .and_then(|n| n.strip_suffix(".meta.json"))
            else {
                continue;
            };
            let meta: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let s = |k: &str| meta.get(k).and_then(|v| v.as_str()).map(str::to_string);
            let depth = meta.get("spawnDepth").and_then(|v| v.as_u64()).unwrap() as u8;
            let log = dir.join(format!("agent-{agent}.jsonl"));
            let mut parser = ClaudeParser::new(depth, Some(id.clone()));
            files.push(parse_file(&log, &mut parser));
            subs.push(SubMeta {
                agent_id: agent.to_string(),
                model: s("model"),
                effort: s("effort"),
                description: s("description"),
                depth,
            });
        }
        Self {
            id,
            product: Product::ClaudeDesktop,
            subs,
            files,
        }
    }

    fn codex(scenario: &str) -> Self {
        let path = jsonl_in(&fixture(&format!("codex/{scenario}")));
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let id = stem[stem.len() - 36..].to_string();
        let files = vec![parse_file(&path, &mut CodexParser::new(&id))];
        Self {
            id,
            product: Product::CodexApp,
            subs: Vec::new(),
            files,
        }
    }

    /// `until` 까지의 이벤트로 재구성하고, 그 뒤 `plus` 초 지난 논리 시각으로 tick 한다
    fn at(&self, until: &str, plus: i64) -> Team {
        let until: DateTime<Utc> = until.parse().unwrap();
        let files = self
            .files
            .iter()
            .map(|f| {
                let mut last = None;
                f.iter()
                    .filter(|e| {
                        last = last.max(e.ts);
                        last.is_none_or(|t| t <= until)
                    })
                    .cloned()
                    .collect()
            })
            .collect();
        let mut team = rebuild(&self.id, self.product, &self.subs, files, utc());
        team.tick(until + Duration::seconds(plus), utc(), &mut Vec::new());
        team
    }
}

fn boss(t: &Team) -> &Employee {
    &t.employees[0]
}

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

#[test]
fn snapshot_parallel_reversed() {
    let c = Case::claude("parallel-reversed");
    let t = c.at("2026-03-02T09:00:23.232Z", 0);
    is(
        boss(&t),
        Room::Archive,
        "`dir3/file5.md` 열람 중 (외 1건)",
        Confirmed,
    );
    // 나중 호출의 결과가 먼저 온다
    let t = c.at("2026-03-02T09:00:23.822Z", 0);
    is(
        boss(&t),
        Room::Archive,
        "`dir3/file4.md` 열람 중",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:00:23.829Z", 0);
    assert!(boss(&t).pending.is_empty());
    is(boss(&t), Room::TeamLead, "`dir3/file4.md` 열람", Confirmed);
    let t = c.at("2026-03-02T09:00:23.829Z", 3);
    is(boss(&t), Room::TeamLead, "턱 괴고 고민 중인 듯", Inferred);
    let t = c.at("2026-03-02T09:00:23.829Z", 11);
    is(boss(&t), Room::TeamLead, "러버덕과 상담 중인 듯", Inferred);
}

#[test]
fn snapshot_reject_holds_four_seconds() {
    let c = Case::claude("reject");
    let t = c.at("2026-03-02T09:01:54.802Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "반려됨: `prog1 arg2 -q \"arg3\" \"C:/dir3/dir4/dir5/` 안 됩니다",
        Confirmed,
    );
    assert_eq!(t.props.trash, 0);
    // 반려 직후 4초는 응답 대기보다 앞선다
    let t = c.at("2026-03-02T09:01:54.802Z", 3);
    is(
        boss(&t),
        Room::TeamLead,
        "반려됨: `prog1 arg2 -q \"arg3\" \"C:/dir3/dir4/dir5/` 안 됩니다",
        Confirmed,
    );
    assert!(t.needs_boss(boss(&t), ts("2026-03-02T09:01:57.802Z")));
    // 사장님이 답하면 반려 직후가 끝나고 응답 대기다
    let t = c.at("2026-03-02T09:01:54.874Z", 3);
    is(boss(&t), Room::TeamLead, "턱 괴고 고민 중인 듯", Inferred);
    assert!(!t.needs_boss(boss(&t), ts("2026-03-02T09:01:57.874Z")));
}

#[test]
fn snapshot_compaction() {
    let c = Case::claude("compaction");
    let t = c.at("2026-03-02T09:01:40.215Z", 0);
    is(boss(&t), Room::TeamLead, "분갈이 중 (서류 정리)", Confirmed);
    assert_eq!(t.props.plant, Some(0.0));
    assert_eq!(boss(&t).context_window, Some(1_000_000));
}

const SYNC_SUB: &str = "ac0c27afe76bccd54";

#[test]
fn snapshot_sync_subagent_preregistered() {
    let c = Case::claude("sync-subagent");
    // 자식 파일은 부모 결과보다 먼저 끝난다. 사전 등록 덕에 처음부터 확인된 과장이다
    let t = c.at("2026-03-02T09:03:16.532Z", 0);
    let s = emp(&t, SYNC_SUB);
    assert_eq!((s.rank, s.confidence), (Rank::Gwajang, Confirmed));
    is(
        s,
        Room::Dev,
        "반려됨: `bash /c/dir3/dir4/dir5/dir6/dir7/dir8/di` 안 됩니다",
        Confirmed,
    );
    // Spawn 과 Done 이 같은 결과 레코드에서 온다
    let t = c.at("2026-03-02T09:03:17.133Z", 0);
    assert_eq!(t.employees.len(), 2);
    let s = emp(&t, SYNC_SUB);
    assert!(s.retired_at.is_some());
    assert_eq!(s.parent.as_deref(), Some(c.id.as_str()));
    is(s, Room::Pantry, "탕비실에서 쉬는 중", Confirmed);
    is(
        boss(&t),
        Room::TeamLead,
        "과장에게 \"Tqj xzzdv RQZ tvptrdxbxf-nsceq…\" 지시",
        Confirmed,
    );
}

#[test]
fn snapshot_sync_subagent_without_meta() {
    let mut c = Case::claude("sync-subagent");
    c.subs.clear();
    let t = c.at("2026-03-02T09:03:16.532Z", 0);
    let s = emp(&t, SYNC_SUB);
    assert_eq!((s.rank, s.confidence), (Rank::Sawon, Inferred));
    let t = c.at("2026-03-02T09:03:17.133Z", 0);
    let s = emp(&t, SYNC_SUB);
    // 동기 결과에는 resolvedModel 이 없어 호출의 별칭 "opus" 로 직급을 정한다
    assert_eq!((s.rank, s.confidence), (Rank::Gwajang, Confirmed));
    assert_eq!(s.model.as_deref(), Some("opus"));
    assert!(s.retired_at.is_some());
}

const ASYNC_SUB: &str = "a4fa9a3e07b34f715";

#[test]
fn snapshot_async_subagent() {
    let c = Case::claude("async-subagent");
    let t = c.at("2026-03-02T09:00:44.418Z", 0);
    assert_eq!(boss(&t).background.len(), 1);
    is(
        boss(&t),
        Room::TeamLead,
        "백그라운드: `VAR1=\"C:/dir3/dir4/dir5/dir6/dir7/dir8/d` 돌아가는 중 (작업표 1장)",
        Confirmed,
    );
    // 49.840 의 응답으로 턴이 끝났지만 완료 알림이 다음 응답을 부른다 (2.4 응답 대기)
    let t = c.at("2026-03-02T09:01:30.259Z", 0);
    assert!(boss(&t).background.is_empty());
    is(
        boss(&t),
        Room::TeamLead,
        "`VAR1=\"C:/dir3/dir4/dir5/dir6/dir7/dir8/d` 완료",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:01:30.259Z", 3);
    is(boss(&t), Room::TeamLead, "턱 괴고 고민 중인 듯", Inferred);
    assert!(!t.needs_boss(boss(&t), ts("2026-03-02T09:01:33.259Z")));

    let t = c.at("2026-03-02T09:03:16.532Z", 0);
    is(
        boss(&t),
        Room::Archive,
        "`dir15/dir16/dir17/dir18/file19.exe run v` 전사 수색 중",
        Confirmed,
    );
    let s = emp(&t, ASYNC_SUB);
    assert_eq!((s.rank, s.confidence), (Rank::Gwajang, Confirmed));
    is(
        s,
        Room::Dev,
        "반려됨: `bash /c/dir3/dir4/dir5/dir6/dir7/dir8/di` 안 됩니다",
        Confirmed,
    );
    // 자식 파일의 반려는 사고가 아니다. 휴지통은 자식의 셸 실패 두 건
    assert_eq!(t.props.trash, 2);
    assert_eq!(s.background.len(), 1);
    assert!(s.retired_at.is_none());
    let t = c.at("2026-03-02T09:03:16.532Z", 5);
    is(
        emp(&t, ASYNC_SUB),
        Room::Dev,
        "턱 괴고 고민 중인 듯",
        Inferred,
    );
}

#[test]
fn rebuild_from_main_file_only() {
    let mut c = Case::claude("async-subagent");
    c.subs.clear();
    c.files.truncate(1);
    let t = c.at("2026-03-02T09:01:48.718Z", 0);
    assert_eq!(t.employees.len(), 2);
    let s = emp(&t, ASYNC_SUB);
    assert_eq!((s.rank, s.confidence), (Rank::Gwajang, Confirmed));
    is(s, Room::Dev, "출근 중 · 자리 찾는 중", Confirmed);
}

#[test]
fn snapshot_test_fail_fix_pass() {
    let c = Case::claude("test-fail-fix-pass");
    let t = c.at("2026-03-02T09:00:18Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "사고: `test tests::count_ok ... FAILED`",
        Confirmed,
    );
    assert_eq!((t.props.light, t.props.trash), (Light::Red, 1));
    let t = c.at("2026-03-02T09:00:23Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "빨간불 끄는 중: `tests::count_ok` 2번째 시도",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:00:27Z", 0);
    is(
        boss(&t),
        Room::Test,
        "빨간불 끄는 중: `tests::count_ok` 2번째 시도",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:00:39Z", 0);
    is(boss(&t), Room::TeamLead, "합격 4 / 반려 0", Confirmed);
    assert_eq!((t.props.light, t.props.trash), (Light::Green, 1));
    let t = c.at("2026-03-02T09:00:42Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "보고서 제출함: \"All four tests pass now.\"",
        Confirmed,
    );
}

#[test]
fn snapshot_bg_fail_reject_compact() {
    let c = Case::claude("bg-fail-reject-compact");
    let t = c.at("2026-03-02T09:00:37.607Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "백그라운드: `node -e \"pnoNypnmib(() => vzbkmla.npax(1` 돌아가는 중 (작업표 1장)",
        Confirmed,
    );
    // 실패한 백그라운드 알림은 사고다
    let t = c.at("2026-03-02T09:00:42.776Z", 0);
    assert!(boss(&t).background.is_empty());
    assert_eq!(t.props.trash, 1);
    is(
        boss(&t),
        Room::TeamLead,
        "사고: `Qjmcizmomn bqbepne \"Jch i Bwha ulpbcb al…`",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:01:13.446Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "반려됨: `npx --flag1 arg2 arg3` 안 됩니다",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:02:06.045Z", 0);
    is(boss(&t), Room::TeamLead, "분갈이 중 (서류 정리)", Confirmed);
    assert_eq!(t.props.plant, Some(0.0));
    assert_eq!(t.props.trash, 1);
    let t = c.at("2026-03-02T09:02:19.284Z", 3);
    is(boss(&t), Room::TeamLead, "요구사항 대기 중인 듯", Inferred);
}

#[test]
fn snapshot_codex_ask_reject_compact() {
    let c = Case::codex("ask-reject-compact");
    let t = c.at("2026-03-02T09:00:04.029Z", 0);
    is(boss(&t), Room::Door, "결재판 들고 문 앞 대기", Confirmed);
    assert!(t.needs_boss(boss(&t), ts("2026-03-02T09:00:04.029Z")));
    let t = c.at("2026-03-02T09:00:10.568Z", 0);
    is(boss(&t), Room::TeamLead, "결재 받음", Confirmed);
    // escalated 호출이지만 35초 전에 반려가 왔다
    let t = c.at("2026-03-02T09:00:39Z", 0);
    is(
        boss(&t),
        Room::Build,
        "`prog1 arg2 arg3 arg4 'C:\\file2.txt'` 실행 중",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:00:39.925Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "반려됨: `prog1 arg2 arg3 arg4 'C:\\file2.txt'` 안 됩니다",
        Confirmed,
    );
    let t = c.at("2026-03-02T09:01:50.532Z", 0);
    is(boss(&t), Room::TeamLead, "분갈이 중 (서류 정리)", Confirmed);
    assert_eq!(t.props.plant, Some(0.0));
    let t = c.at("2026-03-02T09:01:59.825Z", 0);
    is(
        boss(&t),
        Room::TeamLead,
        "보고서 제출함: \"칑찚궤뛍!\"",
        Confirmed,
    );
    assert_eq!(t.props.trash, 0);
}

#[test]
fn snapshot_codex_aborted() {
    let c = Case::codex("aborted");
    let t = c.at("2026-03-02T09:00:00.633Z", 0);
    is(boss(&t), Room::TeamLead, "요구사항 접수", Confirmed);
    let t = c.at("2026-03-02T09:00:04.054Z", 3);
    is(boss(&t), Room::TeamLead, "요구사항 대기 중인 듯", Inferred);
    assert!(boss(&t).pending.is_empty());
    let t = c.at("2026-03-02T09:00:04.054Z", 181);
    assert_eq!(t.phase, TeamPhase::Dozing);
    is(boss(&t), Room::Pantry, "졸음", Confirmed);
    let t = c.at("2026-03-02T09:00:04.054Z", 31 * 60);
    assert_eq!(t.phase, TeamPhase::Off);
}

// ---------- 순서 불변 (4.12 통합) ----------

/// ts 있는 이벤트 하나와 그 뒤 같은 파일의 ts 없는 이벤트를 한 묶음으로 두고,
/// ts 가 같은 묶음끼리 순서를 바꾼다 (merge 가 허용하는 범위). 한 줄에서 나온 이벤트도 섞인다
fn shuffled(files: &[Vec<Event>], seed: u64) -> Vec<Event> {
    let mut rng = seed;
    let mut next = || {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        rng >> 33
    };
    let mut units: Vec<(Option<DateTime<Utc>>, u64, Vec<Event>)> = Vec::new();
    for f in files {
        let first = units.len();
        for e in f {
            if units.len() == first || e.ts.is_some() {
                units.push((e.ts, next(), Vec::new()));
            }
            units.last_mut().unwrap().2.push(e.clone());
        }
    }
    units.sort_by_key(|u| (u.0, u.1));
    units.into_iter().flat_map(|u| u.2).collect()
}

type Final = (
    Vec<(
        String,
        Room,
        String,
        Confidence,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
        usize,
        usize,
    )>,
    u32,
    Light,
    Option<f32>,
    TeamPhase,
);

fn final_state(t: &Team) -> Final {
    let emps = t
        .employees
        .iter()
        .map(|e| {
            (
                e.id.clone(),
                e.activity.room,
                e.activity.sentence.clone(),
                e.activity.confidence,
                e.activity.since,
                e.retired_at,
                e.hired_at,
                e.pending.len(),
                e.background.len(),
            )
        })
        .collect();
    (emps, t.props.trash, t.props.light, t.props.plant, t.phase)
}

/// 재구성하고 `end` 에 한 번 tick 한다
fn replayed(id: &str, events: &[Event], end: DateTime<Utc>) -> Team {
    let mut t = replay(id, Product::ClaudeCli, &[], events, utc());
    t.tick(end, utc(), &mut Vec::new());
    t
}

/// 하나씩 적용하며 1초마다 tick 한다. 실제 간격으로 append 한 것과 같은 시계다 (4.3)
fn stepwise(id: &str, events: &[Event], end: DateTime<Utc>) -> Team {
    let sec = Duration::seconds(1);
    let mut clock = events.iter().find_map(|e| e.ts).unwrap();
    let mut team = Team::new(id, Product::ClaudeCli, clock);
    let mut cues = Vec::new();
    for e in events {
        if let Some(t) = e.ts {
            while clock + sec < t {
                clock += sec;
                team.tick(clock, utc(), &mut cues);
            }
            team.tick(t, utc(), &mut cues);
            clock = clock.max(t);
        }
        team.apply(e, &mut cues);
    }
    while clock < end {
        clock = (clock + sec).min(end);
        team.tick(clock, utc(), &mut cues);
    }
    team
}

#[test]
fn stepwise_ticks_match_rebuild_on_fixtures() {
    let claude = [
        "async-subagent",
        "bg-fail-reject-compact",
        "compaction",
        "internal-tool-failure",
        "parallel-reversed",
        "reject",
        "rewrite-shrink",
        "sync-subagent",
        "test-fail-fix-pass",
    ]
    .map(Case::claude);
    let codex = ["aborted", "ask-reject-compact", "codex-app"].map(Case::codex);
    for c in claude.iter().chain(&codex) {
        let events = merge(c.files.clone());
        let last = events.iter().filter_map(|e| e.ts).max().unwrap();
        for end in [last, last + Duration::minutes(40)] {
            assert_eq!(
                final_state(&replayed(&c.id, &events, end)),
                final_state(&stepwise(&c.id, &events, end)),
                "{} {end}",
                c.id
            );
        }
    }
}

#[test]
fn order_invariance_async_subagent() {
    let c = Case::claude("async-subagent");
    let want = final_state(&rebuild(&c.id, c.product, &c.subs, c.files.clone(), utc()));
    let canonical = merge(c.files.clone());
    let mut moved = 0;
    for seed in 0..40 {
        let order = shuffled(&c.files, seed);
        assert_eq!(order.len(), canonical.len());
        moved += usize::from(order != canonical);
        let team = replay(&c.id, c.product, &c.subs, &order, utc());
        assert_eq!(final_state(&team), want, "seed {seed}");
    }
    assert!(moved > 0, "섞인 순서가 하나도 없다");
}

#[test]
fn preregistered_sub_is_hired_when_it_first_appears() {
    for scenario in ["sync-subagent", "async-subagent"] {
        let c = Case::claude(scenario);
        let with = rebuild(&c.id, c.product, &c.subs, c.files.clone(), utc());
        let without = rebuild(&c.id, c.product, &[], c.files.clone(), utc());
        for s in &c.subs {
            let hired = emp(&with, &s.agent_id).hired_at;
            assert_eq!(hired, emp(&without, &s.agent_id).hired_at, "{scenario}");
            assert_ne!(hired, with.started, "{scenario}");
        }
    }
}

// ---------- 담당 (4.7.5) · 오늘의 업무 (4.7.6) ----------

use super::work::{Role, WorkType};

type Step<'a> = (ToolKind, Option<&'a str>, Option<ToolOutcome>);

/// 첫 메시지와 (도구, 경로, 결과) 열로 팀을 만든다. 호출마다 결과를 바로 준다
fn work_sim(msg: Option<&str>, calls: &[Step]) -> Sim {
    let mut s = Sim::new();
    let mut secs = 0;
    if let Some(m) = msg {
        s.on(MAIN, secs, EventKind::UserMessage { text: m.into() });
    }
    for (i, (tool, path, outcome)) in calls.iter().enumerate() {
        secs += 1;
        let id = format!("c{i}");
        let sum = match path {
            Some(p) => file(p),
            None => summary(),
        };
        s.on(MAIN, secs, call(&id, tool.clone(), sum));
        if let Some(o) = outcome {
            s.on(MAIN, secs, result(&id, *o));
        }
    }
    s
}

fn touch(paths: &[&str]) -> Role {
    let calls: Vec<Step> = paths
        .iter()
        .map(|p| (ToolKind::FileRead, Some(*p), None))
        .collect();
    work_sim(None, &calls).boss().role
}

#[test]
fn role_is_trainee_below_five_distinct_files() {
    assert_eq!(touch(&["a.rs", "b.rs", "c.rs", "d.rs"]), Role::Trainee);
    // 같은 파일을 여러 번 읽어도 하나다
    assert_eq!(
        touch(&["a.rs", "a.rs", "a.rs", "b.rs", "b.rs", "c.rs", "d.rs"]),
        Role::Trainee
    );
    assert_eq!(
        touch(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]),
        Role::Backend
    );
}

#[test]
fn role_per_category() {
    let five = |ext: &str| -> Vec<String> { (0..5).map(|i| format!("x/f{i}.{ext}")).collect() };
    for (ext, want) in [
        ("go", Role::Backend),
        ("php", Role::Backend),
        ("tsx", Role::Frontend),
        ("scss", Role::Frontend),
        ("yml", Role::DevOps),
        ("ps1", Role::DevOps),
        ("mdx", Role::Writer),
        ("rst", Role::Writer),
    ] {
        let v = five(ext);
        let refs: Vec<&str> = v.iter().map(String::as_str).collect();
        assert_eq!(touch(&refs), want, "{ext}");
    }
    // 경로 규칙: QA 는 확장자보다 앞선다, 데브옵스는 Dockerfile·.github·ci
    assert_eq!(
        touch(&[
            "tests/a.rs",
            "src/__tests__/b.tsx",
            "x.spec.ts",
            "fixtures/c.json",
            "t/test_d.py"
        ]),
        Role::Qa
    );
    assert_eq!(
        touch(&[
            "Dockerfile",
            ".github/workflows/a.json",
            "ci/b.rs",
            "C:\\p\\.github\\c.md",
            "d.sh"
        ]),
        Role::DevOps
    );
    // `ci` 는 단어 단위라 decision 에 걸리지 않는다
    assert_eq!(
        touch(&["decision/a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]),
        Role::Backend
    );
}

#[test]
fn role_ts_js_follow_path() {
    let fe = ["src/a.ts", "src/b.js", "ui/c.ts", "ui/d.js", "e.ts"];
    assert_eq!(touch(&fe), Role::Frontend);
    let be = [
        "src/server/a.ts",
        "api/b.js",
        "x/backend/c.ts",
        "src\\server\\d.js",
        "e.rs",
    ];
    assert_eq!(touch(&be), Role::Backend);
}

#[test]
fn role_ties_unknown_files_and_tool_kinds() {
    // 프론트 2, 백엔드 2 → 선언 순서가 앞선 백엔드
    assert_eq!(
        touch(&["a.tsx", "b.tsx", "c.rs", "d.rs", "e.md"]),
        Role::Backend
    );
    // 다섯 개를 채웠지만 분류되는 것이 없으면 수습
    assert_eq!(
        touch(&["a.bin", "b.bin", "c.bin", "d.bin", "e.bin"]),
        Role::Trainee
    );
    // 쓰기·검색 도구도 센다, 경로가 있어도 파일 도구가 아니면 안 센다
    let s = work_sim(
        None,
        &[
            (ToolKind::FileWrite, Some("a.md"), None),
            (ToolKind::FileEdit, Some("b.md"), None),
            (ToolKind::FileSearch, Some("c.md"), None),
            (ToolKind::Shell, Some("d.md"), None),
            (ToolKind::FileRead, Some("e.md"), None),
        ],
    );
    assert_eq!(s.boss().role, Role::Trainee);
}

use ToolKind::{
    FileEdit as Ed, FileRead as Rd, FileWrite as Wr, GitDiff, PrView, Test as Tst, WebSearch,
};
use ToolOutcome::{Err as Bad, Ok as Good};

fn kind_of(msg: Option<&str>, calls: &[Step]) -> WorkType {
    work_sim(msg, calls).team.work_type
}

#[test]
fn work_keywords_case_insensitive_and_in_table_order() {
    for (msg, want) in [
        ("Please REVIEW this", WorkType::CodeReview),
        ("코드 리뷰해 줘", WorkType::CodeReview),
        ("버그가 있어", WorkType::BugFix),
        ("빌드가 안 돼", WorkType::BugFix),
        ("Fix the login", WorkType::BugFix),
        ("에러 나", WorkType::BugFix),
        ("오류 확인", WorkType::BugFix),
        ("Refactor the parser", WorkType::Refactor),
        ("코드 정리", WorkType::Refactor),
        ("README 써 줘", WorkType::Docs),
        ("Update the DOCS", WorkType::Docs),
        ("문서 작업", WorkType::Docs),
        ("이게 가능한지 조사", WorkType::Research),
        ("파일 찾아 줘", WorkType::Research),
        ("알아봐", WorkType::Research),
        ("새 기능 만들어", WorkType::NewDev),
        // 우선순위: 리뷰 > 버그 > 리팩터 > 문서 > 조사
        ("review and fix", WorkType::CodeReview),
        ("fix 하고 refactor", WorkType::BugFix),
        ("refactor the docs", WorkType::Refactor),
        ("docs 조사", WorkType::Docs),
    ] {
        assert_eq!(kind_of(Some(msg), &[]), want, "{msg}");
    }
}

#[test]
fn work_without_evidence_keeps_current_value() {
    assert_eq!(work_sim(None, &[]).team.work_type, WorkType::NewDev);
    // 근거가 못 되는 이벤트(짝 없는 결과)는 값을 바꾸지 않는다
    let mut s = work_sim(Some("fix it"), &[]);
    s.on(MAIN, 50, result("nope", Good));
    assert_eq!(s.team.work_type, WorkType::BugFix);
}

#[test]
fn work_tool_patterns() {
    // 코드 리뷰: 쓰기 전에 GitDiff·PrView
    assert_eq!(
        kind_of(
            None,
            &[(GitDiff, None, Some(Good)), (Rd, Some("a.rs"), None)]
        ),
        WorkType::CodeReview
    );
    assert_eq!(kind_of(None, &[(PrView, None, None)]), WorkType::CodeReview);
    // 편집 뒤의 diff 는 리뷰가 아니다
    assert_eq!(
        kind_of(None, &[(Ed, Some("a.rs"), None), (GitDiff, None, None)]),
        WorkType::NewDev
    );
    // 버그 수정: 첫 Test 가 실패
    assert_eq!(
        kind_of(None, &[(Tst, None, Some(Bad)), (Ed, Some("a.rs"), None)]),
        WorkType::BugFix
    );
    // 첫 Test 가 성공이면 뒤의 실패는 "먼저"가 아니다
    assert_eq!(
        kind_of(None, &[(Tst, None, Some(Good)), (Tst, None, Some(Bad))]),
        WorkType::NewDev
    );
    // 리팩터링: 편집이 절반 이상이고 테스트가 계속 통과
    let refactor: Vec<Step> = vec![
        (Ed, Some("a.rs"), None),
        (Ed, Some("b.rs"), None),
        (Ed, Some("c.rs"), None),
        (Tst, None, Some(Good)),
    ];
    assert_eq!(kind_of(None, &refactor), WorkType::Refactor);
    let mut failed = refactor.clone();
    failed.push((Tst, None, Some(Bad)));
    assert_eq!(kind_of(None, &failed), WorkType::NewDev);
    assert_eq!(
        kind_of(None, &refactor[..2]),
        WorkType::NewDev,
        "편집 2번은 리팩터링 근거가 못 된다"
    );
    // 문서: .md 쓰기가 과반
    assert_eq!(
        kind_of(
            None,
            &[
                (Wr, Some("a.md"), None),
                (Ed, Some("b.md"), None),
                (Ed, Some("c.rs"), None)
            ]
        ),
        WorkType::Docs
    );
    assert_eq!(
        kind_of(None, &[(Wr, Some("a.md"), None), (Ed, Some("c.rs"), None)]),
        WorkType::NewDev,
        "반반은 위주가 아니다"
    );
    // 기술 조사: 읽기·검색 위주, 쓰기 없음
    let research: Vec<Step> = vec![
        (WebSearch, None, None),
        (Rd, Some("a.rs"), None),
        (Rd, Some("b.rs"), None),
    ];
    assert_eq!(kind_of(None, &research), WorkType::Research);
    let mut edited = research.clone();
    edited.push((Ed, Some("a.rs"), None));
    assert_eq!(kind_of(None, &edited), WorkType::NewDev);
    // 우선순위: 리뷰 패턴이 버그 패턴보다 앞선다
    assert_eq!(
        kind_of(None, &[(GitDiff, None, None), (Tst, None, Some(Bad))]),
        WorkType::CodeReview
    );
}

#[test]
fn work_freezes_after_twenty_calls() {
    // 20개 동안은 조사 패턴, 그 뒤 편집·diff·실패·키워드는 무시
    let mut calls: Vec<Step> = (0..20).map(|_| (Rd, Some("a.rs"), None)).collect();
    calls.extend([
        (Ed, Some("a.rs"), None),
        (GitDiff, None, None),
        (Tst, None, Some(Bad)),
    ]);
    let mut s = work_sim(None, &calls);
    assert_eq!(s.team.work_type, WorkType::Research);
    s.on(
        MAIN,
        100,
        EventKind::UserMessage {
            text: "fix this bug".into(),
        },
    );
    assert_eq!(s.team.work_type, WorkType::Research);
    // 20번째 호출이 Test 면 그 결과는 창 안이라 센다
    let mut calls: Vec<Step> = (0..19).map(|_| (Rd, Some("a.rs"), None)).collect();
    calls.push((Tst, None, Some(Bad)));
    assert_eq!(kind_of(None, &calls), WorkType::BugFix);
}

#[test]
fn work_only_first_boss_message_counts() {
    let mut s = work_sim(Some("새 기능"), &[]);
    s.on(
        MAIN,
        10,
        EventKind::UserMessage {
            text: "fix bug".into(),
        },
    );
    assert_eq!(s.team.work_type, WorkType::NewDev);
    // 서브 파일의 지시는 키워드가 아니다
    let mut s = Sim::new();
    s.on(
        SUB,
        1,
        EventKind::UserMessage {
            text: "fix bug".into(),
        },
    );
    assert_eq!(s.team.work_type, WorkType::NewDev);
}

#[test]
fn work_and_role_from_fixture_match_hand_expectation() {
    // 첫 메시지 "Run the tests and fix what fails." → fix. 만진 파일은 하나뿐이라 수습
    let c = Case::claude("test-fail-fix-pass");
    let t = rebuild(&c.id, c.product, &c.subs, c.files.clone(), utc());
    assert_eq!(t.work_type, WorkType::BugFix);
    assert_eq!(boss(&t).role, Role::Trainee);
    // 이벤트를 하나씩 먹여도 같다
    let mut s = Team::new(&c.id, c.product, t.started);
    for e in merge(c.files.clone()) {
        s.apply(&e, &mut Vec::new());
    }
    assert_eq!((s.work_type, boss(&s).role), (t.work_type, boss(&t).role));
}
