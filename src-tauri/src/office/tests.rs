//! 상태 기계 단위 테스트(4.12)와 fixture 스냅샷. 기대값은 PLAN 규칙으로 손으로 정했다

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, Utc};

use super::classify::{SubMeta, merge, rebuild};
use super::{Cue, Employee, Light, Rank, Room, Team, TeamPhase};
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

/// 이벤트마다 그 ts 로 tick 한다 (재구성과 같은 시계)
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
        self.team.apply(&ev(agent, secs, kind), &mut self.cues);
        self.tick(secs)
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
    assert_ne!(s.team.props.light, Light::Green);
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
                        last = e.ts.or(last);
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
    let t = c.at("2026-03-02T09:01:54.874Z", 3);
    is(boss(&t), Room::TeamLead, "요구사항 접수", Confirmed);
    assert!(t.needs_boss(boss(&t), ts("2026-03-02T09:01:57.874Z")));
    let t = c.at("2026-03-02T09:01:54.874Z", 5);
    is(boss(&t), Room::TeamLead, "턱 괴고 고민 중인 듯", Inferred);
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
    // 완료 알림은 49.840 의 응답으로 턴이 끝난 뒤라 부장은 결재 대기다
    let t = c.at("2026-03-02T09:01:30.259Z", 0);
    assert!(boss(&t).background.is_empty());
    is(boss(&t), Room::TeamLead, "요구사항 대기 중인 듯", Inferred);

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
    Vec<(String, Room, String, Confidence, bool, usize)>,
    u32,
    Light,
    Option<f32>,
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
                e.retired_at.is_some(),
                e.pending.len(),
            )
        })
        .collect();
    (emps, t.props.trash, t.props.light, t.props.plant)
}

#[test]
fn order_invariance_async_subagent() {
    let c = Case::claude("async-subagent");
    let base = rebuild(&c.id, c.product, &c.subs, c.files.clone(), utc());
    let want = final_state(&base);
    let canonical = merge(c.files.clone());
    let mut moved = 0;
    for seed in 0..40 {
        let order = shuffled(&c.files, seed);
        assert_eq!(order.len(), canonical.len());
        moved += usize::from(order != canonical);
        let mut team = Team::new(&c.id, c.product, base.started);
        for s in &c.subs {
            team.preregister(
                &s.agent_id,
                s.model.as_deref(),
                s.effort.as_deref(),
                s.description.as_deref(),
                s.depth,
                base.started,
            );
        }
        let mut cues = Vec::new();
        for e in &order {
            team.apply(e, &mut cues);
            team.tick(e.ts.unwrap_or(team.last_activity), utc(), &mut cues);
        }
        assert_eq!(final_state(&team), want, "seed {seed}");
    }
    assert!(moved > 0, "섞인 순서가 하나도 없다");
}
