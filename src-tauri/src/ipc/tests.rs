use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset};
use ts_rs::{Config, TS};

use super::*;
use crate::discover::windows_host;
use crate::office::classify::rebuild;
use crate::office::names::NameBook;
use crate::office::{BubbleSay, PendingCall, Pose, Room};
use crate::parse::{
    Compound, Event, EventKind, RawRef, TestResult, TodoStatus, ToolKind, ToolOutcome, ToolSummary,
};
use crate::settings::{Animal, ManualPath, NightMode};
use crate::startup::{StartupDetail, StartupProgressDto, StartupSummary, StepState};
use crate::tail::scheduler::{codex_id, make_parser, sub_meta};

fn t0() -> DateTime<Utc> {
    "2026-03-02T09:00:00Z".parse().unwrap()
}

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn key(host: &str, vendor: Vendor, session: &str) -> TeamKey {
    TeamKey {
        host: host.into(),
        vendor,
        session: session.into(),
    }
}

/// 이름·방 파일은 임시 폴더에. 폴더가 테스트 동안 살아 있어야 저장이 된다
struct Env {
    _dir: tempfile::TempDir,
    world: Arc<Mutex<OfficeWorld>>,
    ipc: Ipc,
    diag: Mutex<Diag>,
    runtime: Runtime,
}

fn wsl_host() -> Host {
    Host {
        id: "wsl:Test".into(),
        kind: HostKind::Wsl {
            distro: "Test".into(),
        },
        label: "Test".into(),
    }
}

/// 호스트 스레드가 하는 것처럼 두 호스트를 등록한 world
fn world_with_hosts(scale: Scale, dir: &Path) -> OfficeWorld {
    let mut w = OfficeWorld::new(scale, NameBook::load(dir), utc());
    for h in [windows_host(), wsl_host()] {
        w.hosts.insert(h.id.clone(), h);
    }
    w
}

fn env(scale: Scale) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let world = Arc::new(Mutex::new(world_with_hosts(scale, dir.path())));
    let ipc = Ipc::new(world.clone(), RoomBook::load(dir.path()));
    Env {
        diag: Mutex::new(Diag::open(dir.path(), false)),
        runtime: Runtime::default(),
        _dir: dir,
        world,
        ipc,
    }
}

fn call(started: DateTime<Utc>) -> PendingCall {
    PendingCall {
        call_id: "c1".into(),
        tool: ToolKind::Build,
        summary: ToolSummary {
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
        },
        started,
        big_read: false,
        question: None,
    }
}

/// 부장 + 서브 하나. 아무 대기도 없는 근무 중 팀
fn team(session: &str) -> Team {
    let mut t = Team::new(session, Product::ClaudeCli, t0());
    t.preregister(&format!("{session}-s"), None, None, None, 1, t0());
    t
}

#[test]
fn projected_departure_is_consistent_in_summary_employees_and_shared_props() {
    for confirmed in [false, true] {
        let e = env(Scale::Small);
        let mut w = lock(&e.world);
        let k = key("windows", Vendor::Claude, "projected");
        let mut t = team("projected");
        t.employees[0].pending.push(call(t0()));
        t.employees[0].asking = Some("질문".into());
        t.props.light = Light::Yellow;
        t.props.night = true;
        w.put_team(k.clone(), t, t0(), true);
        if confirmed {
            w.registry_alive(&k, true, t0());
            w.registry_alive(&k, false, t0() + Duration::seconds(1));
        } else {
            w.set_read_failed(&k, true);
        }
        let t = &w.teams[&k];
        assert_eq!(t.employees[0].pending.len(), 1);
        let summary = team_summary(&w, &k, t, t0());
        assert_eq!(
            (summary.phase, summary.dot, summary.border),
            (TeamPhase::Off, Dot::Off, Border::Plain)
        );
        assert_eq!(summary.asking_count, 0);
        let dto = office_dto(&w, &w.office_of(&k), false, Vec::new(), t0()).unwrap();
        assert_eq!(dto.shared.light, Light::Off);
        assert!(!dto.shared.night && !dto.shared.incident);
        let t = &dto.teams[0];
        assert_eq!(t.phase, TeamPhase::Off);
        assert!(t.desk_slot.is_none());
        assert_eq!(t.props.light, Light::Off);
        for employee in &t.employees {
            assert_eq!(employee.state, EmployeeState::Off);
            assert_eq!(employee.pending_count, 0);
            assert_eq!(
                employee.confidence,
                if confirmed {
                    Confidence::Confirmed
                } else {
                    Confidence::Inferred
                }
            );
        }
    }
}

#[test]
fn daylight_dto_and_night_props_follow_local_boundaries_and_clock_jumps() {
    let e = env(Scale::Large);
    let mut w = lock(&e.world);
    w.local = FixedOffset::east_opt(9 * 3600).unwrap();
    let k = key("windows", Vendor::Claude, "daylight");
    let at = |hour: i64, minute: i64| {
        "2026-03-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
            + Duration::hours(hour - 9)
            + Duration::minutes(minute)
    };
    for (hour, expected) in [
        (6, Daylight::Day),
        (17, Daylight::Evening),
        (20, Daylight::Night),
        (22, Daylight::Night),
    ] {
        for minute in [-1, 0, 1] {
            let now = at(hour, minute);
            let mut t = Team::new("daylight", Product::ClaudeCli, now);
            t.employees[0].pending.push(call(now));
            w.put_team(k.clone(), t, now, true);
            w.tick(now, "windows");
            let dto = office_dto(&w, &w.office_of(&k), false, Vec::new(), now).unwrap();
            let expected = if minute < 0 {
                match hour {
                    6 => Daylight::Night,
                    17 => Daylight::Day,
                    20 => Daylight::Evening,
                    _ => Daylight::Night,
                }
            } else {
                expected
            };
            assert_eq!(dto.teams[0].daylight, expected, "{hour}:{minute}");
            assert_eq!(dto.teams[0].props.night, expected == Daylight::Night);
            assert_eq!(
                serde_json::to_value(&dto.teams[0]).unwrap()["daylight"],
                serde_json::to_value(expected).unwrap()
            );
        }
    }
    let now = at(16, 59);
    let mut t = Team::new("daylight", Product::ClaudeCli, now);
    t.alive_in_registry = true;
    t.employees[0].pending.push(call(now));
    w.put_team(k.clone(), t, now, true);
    for (now, expected) in [
        (now, Daylight::Day),
        (at(20, 1), Daylight::Night),
        (at(30, 1), Daylight::Day),
    ] {
        w.tick(now, "windows");
        let dto = office_dto(&w, &w.office_of(&k), false, Vec::new(), now).unwrap();
        assert_eq!(dto.teams[0].daylight, expected);
        assert_eq!(
            dto.teams[0].props.night,
            dto.teams[0].daylight == Daylight::Night
        );
    }
}

#[test]
fn evening_boundary_publishes_state_without_night_prop_change() {
    let e = env(Scale::Large);
    let k = key("windows", Vendor::Claude, "daylight");
    let now: DateTime<Utc> = "2026-03-02T16:59:00Z".parse().unwrap();
    {
        let mut w = lock(&e.world);
        let mut t = Team::new("daylight", Product::ClaudeCli, now - Duration::hours(1));
        t.alive_in_registry = true;
        w.put_team(k.clone(), t, now, true);
        w.tick(now, "windows");
        let id = w.office_of(&k);
        w.select(Some(id));
    }
    let mut sent = Sent::default();
    let before = e.ipc.poll(&mut sent, now).1.unwrap();
    let next = now + Duration::minutes(1);
    lock(&e.world).tick(next, "windows");
    let after = e.ipc.poll(&mut sent, next).1.expect("저녁 진입 상태 발행");
    assert!(after.state_seq > before.state_seq);
    assert_eq!(before.teams[0].daylight, Daylight::Day);
    assert_eq!(after.teams[0].daylight, Daylight::Evening);
    assert!(!before.teams[0].props.night && !after.teams[0].props.night);
}

#[test]
fn office_id_round_trips_and_rejects_garbage() {
    let ids = [
        OfficeId::Team(key("windows", Vendor::Claude, "a1b2")),
        OfficeId::Team(key("wsl:Ubuntu-24.04", Vendor::Codex, "x-y_z")),
        OfficeId::Vendor(Vendor::Gemini),
    ];
    for id in ids {
        let s = office_id_str(&id);
        assert_eq!(parse_office_id(&s), Some(id), "{s}");
    }
    for bad in [
        "",
        "team",
        "team/claude/s",
        "team/claude//windows",
        "team/nope/s/windows",
        "vendor/claude/extra",
        "vendor/",
        "office/claude",
    ] {
        assert_eq!(parse_office_id(bad), None, "{bad}");
    }
}

/// design/HANDOFF.md 4.2 범례의 각 경우
#[test]
fn status_dot_legend() {
    let now = t0() + Duration::minutes(1);
    let w = world_with_hosts(Scale::Large, tempfile::tempdir().unwrap().path());
    let k = key("windows", Vendor::Claude, "s");
    let row = |t: &Team| team_summary(&w, &k, t, now);

    let base = team("s");
    let r = row(&base);
    assert_eq!(
        (r.dot, r.asking, r.border, r.delayed, r.host_badge),
        (Dot::Working, false, Border::Plain, false, None)
    );

    // 결재 대기는 3초 디바운스 뒤부터
    let mut t = base.clone();
    t.employees[0].waiting_user_since = Some(now - Duration::seconds(1));
    assert_eq!(row(&t).dot, Dot::Working);
    t.employees[0].waiting_user_since = Some(now - Duration::seconds(5));
    assert_eq!(row(&t).dot, Dot::Waiting);
    // 서브가 일하는 동안은 근무 중
    t.employees[1].pending.push(call(now));
    assert_eq!(row(&t).dot, Dot::Working);

    // 응답 대기 (고민 중)
    let mut t = base.clone();
    t.employees[0].quiet_since = Some(now - Duration::seconds(5));
    assert_eq!(row(&t).dot, Dot::Waiting);

    let mut t = base.clone();
    t.phase = TeamPhase::Dozing;
    assert_eq!(row(&t).dot, Dot::Dozing);
    t.phase = TeamPhase::Off;
    assert_eq!(row(&t).dot, Dot::Off);
    // 장애는 단계보다 앞선다
    t.incident = Some(Incident {
        since: now,
        last_error: now,
    });
    assert_eq!(row(&t).dot, Dot::Incident);

    // 허가 대기는 점 옆 결재판. 점은 그대로
    let mut t = base.clone();
    t.employees[1].asking = Some("q".into());
    let r = row(&t);
    assert_eq!((r.dot, r.asking), (Dot::Working, true));

    let mut t = base.clone();
    t.props.light = Light::Red;
    assert_eq!(row(&t).border, Border::Failed);
    t.props.light = Light::Yellow;
    assert_eq!(row(&t).border, Border::Running);
    t.props.light = Light::Green;
    assert_eq!(row(&t).border, Border::Plain);

    let mut w = w;
    let wk = key("wsl:Test", Vendor::Claude, "s");
    w.set_suspect("wsl:Test", true);
    let r = team_summary(&w, &wk, &base, now);
    assert_eq!((r.delayed, r.host_badge.as_deref()), (true, Some("WSL")));
}

#[test]
fn asking_count_excludes_retired_and_off_and_updates_at_escalation_boundary() {
    let now = t0() + Duration::minutes(1);
    let w = world_with_hosts(Scale::Large, tempfile::tempdir().unwrap().path());
    let k = key("windows", Vendor::Claude, "s");
    let mut t = team("s");
    t.preregister("s-retired", None, None, None, 1, t0());
    t.employees[0].asking = Some("q".into());
    let mut escalated = call(now - Duration::seconds(35));
    escalated.summary.escalated = true;
    t.employees[1].pending.push(escalated);
    t.employees[2].asking = Some("old q".into());
    t.employees[2].retired_at = Some(now - Duration::seconds(1));

    let before = team_summary(&w, &k, &t, now - Duration::seconds(1));
    assert_eq!((before.asking_count, before.asking), (1, true));
    let at = team_summary(&w, &k, &t, now);
    assert_eq!((at.asking_count, at.asking), (2, true));
    t.phase = TeamPhase::Off;
    let off = team_summary(&w, &k, &t, now);
    assert_eq!((off.asking_count, off.asking), (0, false));
}

#[test]
fn employee_state_keeps_individual_waiting_and_finished_separate_from_team_dot() {
    let now = t0() + Duration::minutes(1);
    let w = world_with_hosts(Scale::Large, tempfile::tempdir().unwrap().path());
    let k = key("windows", Vendor::Claude, "s");
    let mut t = team("s");
    t.employees[0].quiet_since = Some(now - Duration::seconds(5));
    t.employees[0].activity.pose = Pose::Think;
    t.employees[1].pending.push(call(now));
    assert_eq!(team_summary(&w, &k, &t, now).dot, Dot::Working);
    assert_eq!(
        employee_dto(&t, &t.employees[0], now, None).state,
        EmployeeState::Waiting
    );
    assert_eq!(
        employee_dto(&t, &t.employees[1], now, None).state,
        EmployeeState::Working
    );

    t.employees[0].activity.pose = Pose::Duck;
    assert_eq!(
        employee_dto(&t, &t.employees[0], now, None).state,
        EmployeeState::Waiting
    );
    t.employees[0].quiet_since = None;
    t.employees[0].asking = Some("approval".into());
    assert_eq!(
        employee_dto(&t, &t.employees[0], now, None).state,
        EmployeeState::Waiting
    );

    t.employees[1].pending.clear();
    t.employees[1].interrupted_at = Some(now);
    assert_eq!(
        employee_dto(&t, &t.employees[1], now, None).state,
        EmployeeState::Interrupted
    );

    t.employees[1].retired_at = Some(now);
    assert_eq!(
        employee_dto(&t, &t.employees[1], now, None).state,
        EmployeeState::Finished
    );
    t.employees[1].departed_at = Some(now);
    assert_eq!(
        employee_dto(&t, &t.employees[1], now, None).state,
        EmployeeState::Off
    );
    t.phase = TeamPhase::Off;
    assert_eq!(
        employee_dto(&t, &t.employees[0], now, None).state,
        EmployeeState::Off
    );
}

#[test]
fn employee_asking_and_question_follow_evidence_even_when_room_disagrees() {
    let now = t0();
    let mut t = team("s");
    let e = &mut t.employees[0];
    e.activity.room = Room::Door;
    let dto = employee_dto(&t, &t.employees[0], now, None);
    assert!(!dto.asking);
    assert_eq!(dto.question, None);

    t.employees[0].activity.room = Room::Dev;
    t.employees[0].asking = Some("새 문구의 결재 질문입니다\n둘째 줄은 제외".into());
    let dto = employee_dto(&t, &t.employees[0], now, None);
    assert!(dto.asking);
    assert_eq!(dto.question.as_deref(), Some("새 문구의 결재 질문입니다…"));

    t.employees[0].activity.bad = true;
    let projected = employee_dto(&t, &t.employees[0], now, Some(Confidence::Inferred));
    assert_eq!(projected.state, EmployeeState::Off);
    assert!(!projected.asking);
    assert_eq!(projected.question, None);
    assert!(!projected.activity.bad);

    t.phase = TeamPhase::Off;
    let off = employee_dto(&t, &t.employees[0], now, None);
    assert!(!off.asking);
    assert_eq!(off.question, None);
}

#[test]
fn read_failure_and_recovery_publish_with_actual_last_activity() {
    let e = env(Scale::Large);
    let k = key("windows", Vendor::Claude, "s");
    let mut t = team("s");
    t.last_activity = t0() + Duration::minutes(4);
    lock(&e.world).put_team(k.clone(), t, t0(), true);
    let mut sent = Sent::default();
    let (initial, _) = e.ipc.poll(&mut sent, t0());
    let initial = initial.unwrap();
    let row = &initial.offices[0].teams[0];
    assert_eq!(row.last_activity, t0() + Duration::minutes(4));
    assert!(!row.read_failed);
    lock(&e.world).set_read_failed(&k, true);
    let (failed, _) = e.ipc.poll(&mut sent, t0());
    let failed = failed.unwrap();
    assert!(failed.list_seq > initial.list_seq);
    assert!(failed.offices[0].teams[0].read_failed);
    lock(&e.world).set_read_failed(&k, false);
    let (recovered, _) = e.ipc.poll(&mut sent, t0());
    let recovered = recovered.unwrap();
    assert!(recovered.list_seq > failed.list_seq);
    assert!(!recovered.offices[0].teams[0].read_failed);
    assert!(e.ipc.poll(&mut sent, t0()).0.is_none());
}

/// 중소기업 층 점: 장애 > 근무 > 결재·응답 대기 > 졸음 > 퇴근. 결재판은 근무 중인 어느 팀이든
#[test]
fn small_floor_dot_follows_most_urgent_team() {
    let e = env(Scale::Small);
    let now = Utc::now();
    let mut steps: Vec<(Team, Dot)> = Vec::new();
    let mut off = team("off");
    off.phase = TeamPhase::Off;
    steps.push((off, Dot::Off));
    let mut dozing = team("dozing");
    dozing.phase = TeamPhase::Dozing;
    steps.push((dozing, Dot::Dozing));
    let mut waiting = team("waiting");
    waiting.employees[0].waiting_user_since = Some(now - Duration::seconds(10));
    steps.push((waiting, Dot::Waiting));
    let mut working = team("working");
    working.employees[0].pending.push(call(now));
    steps.push((working, Dot::Working));
    let mut incident = team("incident");
    incident.phase = TeamPhase::Dozing;
    incident.incident = Some(Incident {
        since: now,
        last_error: now,
    });
    steps.push((incident, Dot::Incident));

    for (t, want) in steps {
        let k = key("windows", Vendor::Claude, &t.id.clone());
        {
            let mut w = lock(&e.world);
            // put_team 의 따라잡기 tick 이 단계를 바꾸지 않게 지금 활동한 것으로
            let mut t = t;
            t.last_activity = now;
            let phase = t.phase;
            w.put_team(k.clone(), t, now, true);
            w.teams.get_mut(&k).unwrap().phase = phase;
        }
        let list = e.ipc.snapshot(now).list;
        assert_eq!(list.offices.len(), 1);
        let floor = &list.offices[0];
        assert_eq!(floor.dot, want, "{:?}", floor.teams);
        assert_eq!(
            (floor.floor, floor.label.as_str(), floor.building.as_deref()),
            (Some(1), "1층 Claude 사무실", None)
        );
    }
    let mut w = lock(&e.world);
    let k = key("windows", Vendor::Claude, "off");
    w.teams.get_mut(&k).unwrap().employees[0].asking = Some("q".into());
    drop(w);
    assert!(!e.ipc.snapshot(now).list.offices[0].asking);

    let mut w = lock(&e.world);
    let k = key("windows", Vendor::Claude, "working");
    w.teams.get_mut(&k).unwrap().employees[0].asking = Some("q".into());
    drop(w);
    let list = e.ipc.snapshot(now).list;
    let floor = &list.offices[0];
    assert!(floor.asking);
    let working = floor
        .teams
        .iter()
        .find(|t| t.team_id == team_id_str(&k))
        .unwrap();
    assert_eq!((working.asking_count, working.asking), (1, true));
}

fn fixture_dir(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(rel)
}

fn parse_file(path: &Path, vendor: Vendor) -> Vec<Event> {
    let mut parser = make_parser(path, vendor);
    let mut unhandled = Vec::new();
    let mut out = Vec::new();
    let bytes = std::fs::read(path).unwrap();
    for (i, line) in bytes.split(|&b| b == b'\n').enumerate() {
        let raw = RawRef {
            path: path.to_path_buf(),
            generation: 0,
            byte_offset: 0,
            line_no: i as u64 + 1,
        };
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        out.extend(parser.parse_line(line, &raw, &mut unhandled));
    }
    out
}

/// 폴더 맨 위의 메인 파일과 `<세션>/subagents/` 로 팀을 다시 만든다
fn fixture_team(rel: &str) -> (TeamKey, Team) {
    let dir = fixture_dir(rel);
    let main = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .unwrap();
    let stem = main.file_stem().unwrap().to_str().unwrap().to_string();
    let (vendor, product, session) = if stem.starts_with("rollout-") {
        (
            Vendor::Codex,
            Product::CodexCli,
            codex_id(&stem).to_string(),
        )
    } else {
        (Vendor::Claude, Product::ClaudeCli, stem.clone())
    };
    let mut files = vec![parse_file(&main, vendor)];
    let mut subs = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join(&stem).join("subagents")) {
        let mut paths: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect();
        paths.sort();
        for p in paths {
            subs.extend(sub_meta(&p));
            files.push(parse_file(&p, vendor));
        }
    }
    let team = rebuild(&session, product, &subs, files, utc());
    (key("windows", vendor, &session), team)
}

const FIXTURES: [&str; 6] = [
    "claude/async-subagent",
    "claude/sync-subagent",
    "claude/test-fail-fix-pass",
    "claude/parallel-reversed",
    "codex/codex-app",
    "codex/ask-reject-compact",
];

#[test]
fn wall_room_events_merge_by_time_team_agent_and_existing_order() {
    let e = env(Scale::Small);
    let mut w = lock(&e.world);
    for session in ["b", "a"] {
        let mut t = team(session);
        t.preregister("0-sub", None, None, None, 1, t0());
        for employee in &mut t.employees {
            for n in 0..4 {
                let mut activity = employee.activity.clone();
                activity.room = Room::Dev;
                activity.sentence = format!("{session}:{}:{n}", employee.id);
                activity.since = t0()
                    + Duration::seconds(if session == "b" && employee.id == "b" && n == 3 {
                        20
                    } else {
                        10
                    });
                employee.room_recent.push_front(activity);
            }
            for room in [Room::Door, Room::TeamLead] {
                let mut activity = employee.activity.clone();
                activity.room = room;
                employee.room_recent.push_back(activity);
            }
        }
        w.put_team(
            key("windows", Vendor::Claude, session),
            t,
            t0() + Duration::seconds(20),
            true,
        );
    }
    let dto = office_dto(
        &w,
        &OfficeId::Vendor(Vendor::Claude),
        false,
        Vec::new(),
        t0(),
    )
    .unwrap();
    assert_eq!(
        dto.room_recent.iter().map(|r| r.room).collect::<Vec<_>>(),
        [Room::TeamLead, Room::Dev, Room::Door]
    );
    let events = &dto.room_recent[1].events;
    assert_eq!(events.len(), 10);
    assert_eq!(
        events
            .iter()
            .map(|e| e.activity.sentence.as_str())
            .collect::<Vec<_>>(),
        [
            "b:b:3",
            "a:0-sub:3",
            "a:0-sub:2",
            "a:0-sub:1",
            "a:0-sub:0",
            "a:a:3",
            "a:a:2",
            "a:a:1",
            "a:a:0",
            "a:a-s:3"
        ]
    );
    assert_eq!(events[0].team_id, "team/claude/b/windows");
    assert_eq!(events[0].agent_id, "b");
    assert_eq!(events[1].team_id, "team/claude/a/windows");
    assert_eq!(events[1].agent_id, "0-sub");
    for t in w.teams.values_mut() {
        t.employees.reverse();
        t.desk_slot = t.desk_slot.map(|slot| 3 - slot);
    }
    let reordered = office_dto(
        &w,
        &OfficeId::Vendor(Vendor::Claude),
        false,
        Vec::new(),
        t0(),
    )
    .unwrap();
    assert_eq!(dto.room_recent, reordered.room_recent);
}

#[test]
fn wall_stats_deduplicate_files_per_team_and_sum_command_counts() {
    let e = env(Scale::Small);
    let mut w = lock(&e.world);
    let mut a = team("a");
    let main = Arc::make_mut(&mut a.employees[0].record);
    for _ in 0..2 {
        main.files_edited.add(Some("src/shared.rs"));
        main.commands.add(Some("cargo check"));
    }
    let sub = Arc::make_mut(&mut a.employees[1].record);
    sub.files_edited.add(Some("src/shared.rs"));
    sub.files_edited.add(Some("src/other.rs"));
    for _ in 0..3 {
        sub.commands.add(Some("cargo check"));
    }
    a.turns = 2;
    a.commits = 1;
    let mut b = team("b");
    let main = Arc::make_mut(&mut b.employees[0].record);
    main.files_edited.add(Some("src/shared.rs"));
    main.commands.add(Some("cargo check"));
    b.turns = 3;
    b.commits = 2;
    for t in [a, b] {
        w.put_team(key("windows", Vendor::Claude, &t.id), t, t0(), true);
    }
    let id = OfficeId::Vendor(Vendor::Claude);
    let dto = office_dto(&w, &id, false, Vec::new(), t0()).unwrap();
    assert_eq!(
        dto.shared.stats,
        BoardStatsDto {
            commits: 3,
            files_edited: 3,
            commands: 6,
            turns: 5
        }
    );
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["shared"]["stats"]["filesEdited"], 3);
    assert_eq!(json["roomRecent"], serde_json::json!([]));
    let a = w
        .teams
        .get_mut(&key("windows", Vendor::Claude, "a"))
        .unwrap();
    a.commits = u32::MAX;
    a.turns = u32::MAX;
    Arc::make_mut(&mut a.employees[0].record).commands.items[0].1 = u32::MAX;
    let dto = office_dto(&w, &id, false, Vec::new(), t0()).unwrap();
    assert_eq!(
        dto.shared.stats,
        BoardStatsDto {
            commits: u32::MAX,
            files_edited: 3,
            commands: u32::MAX,
            turns: u32::MAX
        }
    );
}

/// 직원 문장·since·confidence 가 상태 기계 값 그대로 실린다
#[test]
fn employee_activity_is_carried_verbatim() {
    let e = env(Scale::Large);
    let mut checked = 0;
    for rel in FIXTURES {
        let (k, t) = fixture_team(rel);
        let now = t.last_activity;
        let mut w = lock(&e.world);
        w.put_team(k.clone(), t, now, true);
        let id = w.office_of(&k);
        let dto = office_dto(&w, &id, false, Vec::new(), t0()).unwrap();
        let team = &w.teams[&k];
        let json = serde_json::to_value(&dto).unwrap();
        for (i, emp) in team.employees.iter().enumerate() {
            let a = &json["teams"][0]["employees"][i]["activity"];
            assert_eq!(a["sentence"], emp.activity.sentence.as_str(), "{rel}");
            assert_eq!(
                a["since"]
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                Some(emp.activity.since),
                "{rel}"
            );
            assert_eq!(
                a["confidence"],
                serde_json::to_value(emp.activity.confidence).unwrap()
            );
            assert_eq!(json["teams"][0]["employees"][i]["agentId"], emp.id.as_str());
            assert_eq!(
                dto.teams[0].employees[i].recent,
                emp.recent.iter().cloned().collect::<Vec<_>>()
            );
            checked += 1;
        }
    }
    assert!(checked > FIXTURES.len(), "서브가 있는 fixture 가 빠졌다");
}

fn ev(session: &str, secs: i64, kind: EventKind) -> Event {
    Event {
        ts: Some(t0() + Duration::seconds(secs)),
        session_id: session.into(),
        agent_id: session.into(),
        parent_agent: None,
        kind,
        raw_ref: RawRef {
            path: PathBuf::new(),
            generation: 0,
            byte_offset: 0,
            line_no: 0,
        },
    }
}

/// 사장님 방문: 부장에게 사용자 메시지가 오면 BossVisit cue 가 난다
fn visit(session: &str, secs: i64) -> Event {
    ev(
        session,
        secs,
        EventKind::UserMessage {
            text: "지시".into(),
        },
    )
}

fn cue_of(k: &TeamKey, cue: Cue) -> CueDto {
    CueDto {
        team_id: team_id_str(k),
        cue,
    }
}

/// 번호가 바뀔 때만 내고, 연출은 선택 사무실 것만 한 번 싣는다. 조용한 재구성 뒤 첫 상태는 rebuilt 이고 출근만 싣는다
#[test]
fn publisher_emits_on_change_and_drains_cues() {
    let e = env(Scale::Large);
    let (a, b) = (
        key("windows", Vendor::Claude, "a"),
        key("windows", Vendor::Claude, "b"),
    );
    let now = t0();
    {
        let mut w = lock(&e.world);
        w.put_team(a.clone(), team("a"), now, true);
        w.put_team(b.clone(), team("b"), now, true);
        w.cue(&a, Cue::Truck);
        w.cue(&a, Cue::Enter { agent: "a".into() });
        let office = w.office_of(&a);
        assert!(w.select(Some(office)));
    }
    let mut sent = Sent::default();
    let (list, state) = e.ipc.poll(&mut sent, now);
    let list = list.expect("처음엔 목록을 낸다");
    assert_eq!(list.offices.len(), 2);
    assert!(list.offices.iter().all(|o| o.room.is_some()));
    let state = state.expect("처음엔 선택 사무실을 낸다");
    assert_eq!(state.office_id, office_id_str(&OfficeId::Team(a.clone())));
    assert!(state.rebuilt);
    assert_eq!(
        state.cues,
        vec![cue_of(&a, Cue::Enter { agent: "a".into() })]
    );

    // 바뀐 것이 없으면 아무것도 안 낸다
    let (list, state) = e.ipc.poll(&mut sent, now);
    assert!(list.is_none() && state.is_none());

    // 선택 안 된 사무실이 바뀌면 상태는 안 낸다. 그 사무실 연출은 버린다
    {
        let mut w = lock(&e.world);
        w.apply(&b, &[visit("b", 1)], now);
        assert!(!w.outbox.is_empty());
    }
    let (_, state) = e.ipc.poll(&mut sent, now);
    assert!(state.is_none());
    assert!(lock(&e.world).outbox.is_empty());

    // 선택 사무실이 바뀌면 낸다. apply 가 같은 잠금에서 번호를 올리고 연출을 넣는다
    let seq = {
        let mut w = lock(&e.world);
        let before = w.state_seq[&w.office_of(&a)];
        w.apply(&a, &[visit("a", 2)], now);
        let seq = w.state_seq[&w.office_of(&a)];
        assert!(seq > before && !w.outbox.is_empty());
        seq
    };
    let (_, state) = e.ipc.poll(&mut sent, now);
    let state = state.unwrap();
    assert_eq!((state.state_seq, state.rebuilt), (seq, false));
    assert_eq!(
        state.cues,
        vec![cue_of(
            &a,
            Cue::BossVisit {
                text: "지시".into()
            }
        )]
    );
    assert!(lock(&e.world).outbox.is_empty());
    assert!(e.ipc.poll(&mut sent, now).1.is_none());

    // 번호가 같아도 남은 연출은 낸다
    lock(&e.world).cue(&a, Cue::Enter { agent: "x".into() });
    let state = e.ipc.poll(&mut sent, now).1.unwrap();
    assert_eq!((state.state_seq, state.cues.len()), (seq, 1));

    // 미대응 기록(docs/CONTRACTS.md C11)은 office-state 에 실리지 않는다
    lock(&e.world).cue(
        &a,
        Cue::Unhandled(crate::parse::Unhandled::StateConflict {
            winner: "허가 대기",
            masked: "보고",
        }),
    );
    lock(&e.world).cue(&a, Cue::Enter { agent: "y".into() });
    let state = e.ipc.poll(&mut sent, now).1.unwrap();
    let json = serde_json::to_string(&state).unwrap();
    assert!(
        state.cues.len() == 1 && !json.contains("허가 대기"),
        "{json}"
    );
}

/// 서브가 보여 다시 넣은 팀(조용하지 않은 재구성)은 rebuilt 로 나가지 않고 연출도 버리지 않는다.
/// 규모를 바꾸면 그 전 연출은 버리고 rebuilt 로 내며 목록이 바뀐 선택을 알린다 (docs/CONTRACTS.md C8·C10)
#[test]
fn non_quiet_rebuild_keeps_cues_and_scale_change_is_rebuilt() {
    let e = env(Scale::Large);
    let a = key("windows", Vendor::Claude, "a");
    let now = t0();
    {
        let mut w = lock(&e.world);
        w.put_team(a.clone(), team("a"), now, true);
        let office = w.office_of(&a);
        assert!(w.select(Some(office)));
    }
    let mut sent = Sent::default();
    assert!(e.ipc.poll(&mut sent, now).1.unwrap().rebuilt);
    {
        let mut w = lock(&e.world);
        w.cue(&a, Cue::Truck);
        let mut t = team("a");
        t.preregister("a-s2", None, None, None, 1, now);
        w.put_team(a.clone(), t, now, false);
        w.cue(
            &a,
            Cue::Enter {
                agent: "a-s2".into(),
            },
        );
    }
    let state = e.ipc.poll(&mut sent, now).1.unwrap();
    assert!(!state.rebuilt);
    assert_eq!(state.cues.len(), 2, "{:?}", state.cues);
    {
        let mut w = lock(&e.world);
        w.cue(&a, Cue::Truck);
        w.set_scale(Scale::Small);
        assert!(w.outbox.is_empty());
    }
    let (list, state) = e.ipc.poll(&mut sent, now);
    let list = list.unwrap();
    assert!(list.rebuilt);
    assert_eq!(list.selected_id.as_deref(), Some("vendor/claude"));
    let state = state.unwrap();
    assert!(state.rebuilt && state.cues.is_empty());
}

/// 설정으로 규모를 바꾸면 world 가 재배치하고 목록·상태가 rebuilt 로 나간다 (docs/CONTRACTS.md C8).
/// 저장이 실패해도 규모는 바뀐다
#[test]
fn set_settings_scale_rebuilds_and_failed_save_still_applies() {
    let e = env(Scale::Large);
    let a = key("windows", Vendor::Claude, "a");
    let now = t0();
    {
        let mut w = lock(&e.world);
        w.put_team(a.clone(), team("a"), now, true);
        let office = w.office_of(&a);
        assert!(w.select(Some(office)));
    }
    let mut sent = Sent::default();
    e.ipc.poll(&mut sent, now);

    let dir = tempfile::tempdir().unwrap();
    let book = SettingsBook::load(dir.path());
    let small = SettingsDto {
        scale: Some(Scale::Small),
        ..SettingsDto::default()
    };
    // 빈 칸은 건드리지 않는다
    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, SettingsDto::default()).unwrap();
    assert!(e.ipc.poll(&mut sent, now).0.is_none());

    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, small.clone()).unwrap();
    assert_eq!(SettingsBook::load(dir.path()).scale(), Some(Scale::Small));
    let (list, state) = e.ipc.poll(&mut sent, now);
    let list = list.unwrap();
    assert!(list.rebuilt && list.scale == Scale::Small);
    assert_eq!(list.selected_id.as_deref(), Some("vendor/claude"));
    assert!(state.unwrap().rebuilt);

    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    let broken = SettingsBook::load(&file);
    let large = SettingsDto {
        scale: Some(Scale::Large),
        ..SettingsDto::default()
    };
    assert!(apply_settings(&e.ipc, &broken, &e.diag, &e.runtime, large).is_err());
    assert_eq!(broken.scale(), Some(Scale::Large));
    assert!(e.ipc.poll(&mut sent, now).0.unwrap().scale == Scale::Large);
}

/// 규모 없이 이름만 보내도 저장되고 규모는 그대로다. "" 는 기본 이름으로 되돌린다
#[test]
fn set_settings_company_name_alone() {
    let e = env(Scale::Large);
    let now = t0();
    let mut sent = Sent::default();
    e.ipc.poll(&mut sent, now);
    let dir = tempfile::tempdir().unwrap();
    let book = SettingsBook::load(dir.path());
    book.set_scale(Scale::Small).unwrap();
    let name = |n: &str| SettingsDto {
        company_name: Some(n.into()),
        ..SettingsDto::default()
    };

    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, name("야근상사")).unwrap();
    let want = SettingsDto {
        scale: Some(Scale::Small),
        company_name: Some("야근상사".into()),
        manual_paths: Some(Vec::new()),
        sound: Some(false),
        night_mode: Some(NightMode::Auto),
        os_notifications: Some(true),
        animal: None,
        boss_name: None,
        raw_diagnostics: Some(false),
    };
    assert_eq!(settings_dto(&SettingsBook::load(dir.path())), want);
    // 규모를 건드리지 않았으니 재배치도 없다
    assert!(e.ipc.poll(&mut sent, now).0.is_none());

    // 빈 칸(null)은 이름을 건드리지 않는다
    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, SettingsDto::default()).unwrap();
    assert_eq!(settings_dto(&book), want);

    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, name("")).unwrap();
    let dto = settings_dto(&SettingsBook::load(dir.path()));
    assert_eq!(dto.company_name, None);
    assert_eq!(dto.scale, Some(Scale::Small));
}

/// 부분 갱신·null·빈 값·기본값이 재시작 뒤에도 같은 계약을 유지한다.
#[test]
fn set_settings_round_trip_and_partial_updates() {
    let e = env(Scale::Large);
    let dir = tempfile::tempdir().unwrap();
    let book = SettingsBook::load(dir.path());
    let initial = settings_dto(&book);
    assert_eq!(initial.sound, Some(false));
    assert_eq!(initial.os_notifications, Some(true));
    assert_eq!(initial.raw_diagnostics, Some(false));
    assert_eq!(initial.boss_name, None);
    assert_eq!(initial.manual_paths, Some(Vec::new()));
    let patch: SettingsDto = serde_json::from_value(serde_json::json!({
        "scale": "Small", "companyName": "회사", "sound": true,
        "osNotifications": false, "rawDiagnostics": true,
        "bossName": format!("  {}  ", "가".repeat(21)),
        "manualPaths": [{"vendor": "Codex", "root": "D:/fixture/sessions"}]
    }))
    .unwrap();
    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, patch).unwrap();
    let restored = settings_dto(&SettingsBook::load(dir.path()));
    assert_eq!(restored.boss_name, Some("가".repeat(20)));
    assert_eq!(restored.sound, Some(true));
    assert_eq!(restored.os_notifications, Some(false));
    assert_eq!(restored.raw_diagnostics, Some(true));
    assert_eq!(
        restored.manual_paths.as_ref().unwrap()[0].root,
        "D:/fixture/sessions"
    );
    let unchanged: SettingsDto = serde_json::from_value(serde_json::json!({
        "bossName": null, "manualPaths": null, "sound": null,
        "osNotifications": null, "rawDiagnostics": null
    }))
    .unwrap();
    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, unchanged).unwrap();
    assert_eq!(settings_dto(&SettingsBook::load(dir.path())), restored);
    apply_settings(
        &e.ipc,
        &book,
        &e.diag,
        &e.runtime,
        SettingsDto {
            boss_name: Some("  ".into()),
            manual_paths: Some(Vec::new()),
            sound: Some(false),
            ..SettingsDto::default()
        },
    )
    .unwrap();
    let cleared = settings_dto(&SettingsBook::load(dir.path()));
    assert_eq!(cleared.boss_name, None);
    assert_eq!(cleared.manual_paths, Some(Vec::new()));
    assert_eq!(cleared.sound, Some(false));
    assert_eq!(cleared.raw_diagnostics, restored.raw_diagnostics);
    assert_eq!(cleared.os_notifications, restored.os_notifications);
    assert_eq!(cleared.company_name, restored.company_name);
    assert_eq!(cleared.scale, restored.scale);
}

/// 잘못된 경로가 있으면 다른 설정의 저장이나 world 재배치도 시작하지 않는다.
#[test]
fn invalid_manual_path_rejects_the_whole_settings_patch() {
    let e = env(Scale::Large);
    let dir = tempfile::tempdir().unwrap();
    let book = SettingsBook::load(dir.path());
    book.set_scale(Scale::Large).unwrap();
    let before = std::fs::read(dir.path().join("settings.json")).unwrap();
    let dto: SettingsDto = serde_json::from_value(serde_json::json!({
        "scale": "Small", "sound": true,
        "manualPaths": [{"vendor": "Claude", "root": "../projects"}]
    }))
    .unwrap();
    assert!(apply_settings(&e.ipc, &book, &e.diag, &e.runtime, dto).is_err());
    assert_eq!(book.scale(), Some(Scale::Large));
    assert_eq!(settings_dto(&book).sound, Some(false));
    assert_eq!(
        std::fs::read(dir.path().join("settings.json")).unwrap(),
        before
    );
    assert_eq!(e.ipc.snapshot(t0()).list.scale, Scale::Large);
}

/// 저장 실패에도 한 요청에 들어온 설정은 모두 메모리에 적용한다.
#[test]
fn settings_save_failure_keeps_all_changes_in_memory() {
    let e = env(Scale::Large);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not_a_directory");
    std::fs::write(&file, "x").unwrap();
    let book = SettingsBook::load(&file);
    let dto: SettingsDto = serde_json::from_value(serde_json::json!({
        "scale": "Small", "companyName": "회사", "bossName": "이름",
        "sound": true, "nightMode": "on", "osNotifications": false, "rawDiagnostics": true,
        "manualPaths": [{"vendor": "Claude", "root": "D:/fixture/projects"}]
    }))
    .unwrap();
    assert!(apply_settings(&e.ipc, &book, &e.diag, &e.runtime, dto.clone()).is_err());
    assert_eq!(settings_dto(&book), dto);
    assert_eq!(e.ipc.snapshot(t0()).list.scale, Scale::Small);
    assert!(e.runtime.rescan.load(std::sync::atomic::Ordering::Relaxed));
    let mut kind = tool("diagnostic", ToolKind::Shell, "custom SECRET");
    if let EventKind::ToolCall { summary, .. } = &mut kind {
        summary.command_key = Some("custom".into());
        summary.command_norm = Some("custom SECRET".into());
    }
    lock(&e.diag).observe_event(&ev("a", 0, kind), t0());
    lock(&e.diag).flush().unwrap();
    let log = e._dir.path().join("unhandled.jsonl");
    assert!(std::fs::read_to_string(&log).unwrap().contains("SECRET"));
    assert!(
        apply_settings(
            &e.ipc,
            &book,
            &e.diag,
            &e.runtime,
            SettingsDto {
                raw_diagnostics: Some(false),
                ..SettingsDto::default()
            }
        )
        .is_err()
    );
    lock(&e.diag).flush().unwrap();
    assert!(!std::fs::read_to_string(log).unwrap().contains("SECRET"));
}

/// 설정 파일 저장 성공과 원문 제거 성공은 별개다. 실패 뒤 재조회는 수집 중단 상태를 돌려준다.
#[test]
fn raw_purge_failure_is_returned_even_when_settings_save_succeeds() {
    let e = env(Scale::Large);
    let book = SettingsBook::load(e._dir.path());
    let patch = |raw| SettingsDto {
        raw_diagnostics: Some(raw),
        ..SettingsDto::default()
    };
    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, patch(true)).unwrap();
    let mut kind = tool("diagnostic", ToolKind::Shell, "custom SECRET");
    if let EventKind::ToolCall { summary, .. } = &mut kind {
        summary.command_key = Some("custom".into());
        summary.command_norm = Some("custom SECRET".into());
    }
    lock(&e.diag).observe_event(&ev("a", 0, kind), t0());
    lock(&e.diag).flush().unwrap();
    let log = e._dir.path().join("unhandled.jsonl");
    let blocker = log.with_extension("jsonl.tmp");
    std::fs::create_dir(&blocker).unwrap();

    let err = apply_settings(&e.ipc, &book, &e.diag, &e.runtime, patch(false)).unwrap_err();
    assert!(err.contains("원문 수집은 중단"), "{err}");
    assert!(std::fs::read_to_string(&log).unwrap().contains("SECRET"));
    assert_eq!(book.snapshot().raw_diagnostics, Some(false));
    assert_eq!(
        SettingsBook::load(e._dir.path()).snapshot().raw_diagnostics,
        Some(false)
    );

    std::fs::remove_dir(blocker).unwrap();
    apply_settings(&e.ipc, &book, &e.diag, &e.runtime, patch(false)).unwrap();
    assert!(!std::fs::read_to_string(log).unwrap().contains("SECRET"));
}

/// 중소기업 사무실의 연출에는 어느 팀 것인지 실린다
#[test]
fn cues_carry_team_id_in_shared_office() {
    let e = env(Scale::Small);
    let (a, b) = (
        key("windows", Vendor::Claude, "a"),
        key("windows", Vendor::Claude, "b"),
    );
    let now = t0();
    {
        let mut w = lock(&e.world);
        w.put_team(a.clone(), team("a"), now, true);
        w.put_team(b.clone(), team("b"), now, true);
        assert!(w.select(Some(OfficeId::Vendor(Vendor::Claude))));
    }
    let mut sent = Sent::default();
    e.ipc.poll(&mut sent, now);
    lock(&e.world).apply(&b, &[visit("b", 1)], now);
    let state = e.ipc.poll(&mut sent, now).1.unwrap();
    let json = serde_json::to_value(&state.cues).unwrap();
    assert_eq!(json[0]["teamId"], team_id_str(&b).as_str());
    assert_eq!(json[0]["kind"], "BossVisit");
}

/// 점처럼 시간으로 바뀌는 값은 world 의 목록 번호가 그대로여도 번호를 올려 낸다
#[test]
fn time_driven_list_change_bumps_list_seq() {
    let e = env(Scale::Large);
    let k = key("windows", Vendor::Claude, "a");
    let now = Utc::now();
    {
        let mut w = lock(&e.world);
        let mut t = team("a");
        t.last_activity = now;
        w.put_team(k.clone(), t, now, true);
        w.teams.get_mut(&k).unwrap().employees[0].waiting_user_since = Some(now);
    }
    let mut sent = Sent::default();
    let first = e.ipc.poll(&mut sent, now).0.unwrap();
    assert_eq!(first.offices[0].dot, Dot::Working);
    assert!(e.ipc.poll(&mut sent, now).0.is_none());
    let later = e.ipc.poll(&mut sent, now + Duration::seconds(4)).0.unwrap();
    assert_eq!(later.offices[0].dot, Dot::Waiting);
    assert!(later.list_seq > first.list_seq);
    // 스냅샷도 같은 번호 규칙을 쓴다
    assert_eq!(
        e.ipc.snapshot(now + Duration::seconds(4)).list.list_seq,
        later.list_seq
    );
}

#[test]
fn select_validates_and_returns_state_with_request_id() {
    let e = env(Scale::Large);
    let k = key("windows", Vendor::Claude, "a");
    lock(&e.world).put_team(k.clone(), team("a"), t0(), true);
    let id = office_id_str(&OfficeId::Team(k));
    let r = e.ipc.select(&id, 7).unwrap();
    assert_eq!(
        (r.request_id, r.office.office_id.as_str()),
        (7, id.as_str())
    );
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["requestId"], 7);
    assert!(json["stateSeq"].is_u64());
    assert!(
        e.ipc.select("vendor/claude", 8).is_err(),
        "대기업에 층 사무실은 없다"
    );
    assert!(e.ipc.select("../etc", 9).is_err());
    assert!(e.ipc.snapshot(t0()).selected.is_some());
}

/// 두 select 가 뒤바뀌어 오면 늦게 온 옛 요청은 무시하고, 목록이 선택과 그 요청 번호를 싣는다
#[test]
fn stale_select_is_ignored_and_list_reports_selection() {
    let e = env(Scale::Large);
    let (a, b) = (
        key("windows", Vendor::Claude, "a"),
        key("windows", Vendor::Claude, "b"),
    );
    {
        let mut w = lock(&e.world);
        w.put_team(a.clone(), team("a"), t0(), true);
        w.put_team(b.clone(), team("b"), t0(), true);
    }
    let (ia, ib) = (
        office_id_str(&OfficeId::Team(a.clone())),
        office_id_str(&OfficeId::Team(b.clone())),
    );
    let mut sent = Sent::default();
    e.ipc.poll(&mut sent, t0());
    assert!(e.ipc.select(&ib, 2).is_ok());
    assert!(e.ipc.select(&ia, 1).is_err());
    assert_eq!(lock(&e.world).selected, Some(OfficeId::Team(b.clone())));
    let list = e
        .ipc
        .poll(&mut sent, t0())
        .0
        .expect("선택이 바뀌면 목록을 낸다");
    assert_eq!(
        (list.selected_id.as_deref(), list.select_request),
        (Some(ib.as_str()), 2)
    );
    // 백엔드가 선택을 스스로 풀면(마지막 팀 퇴장) 목록이 알린다
    lock(&e.world).remove_team(&b);
    let list = e.ipc.poll(&mut sent, t0()).0.unwrap();
    assert_eq!((list.selected_id, list.select_request), (None, 2));
}

/// 공유 신호등은 가장 늦게 바뀐 빌드·테스트 결과다. 활동이 더 최근인 팀의 옛 빨강이 이기지 않는다
#[test]
fn shared_light_follows_latest_result() {
    let e = env(Scale::Small);
    let (a, b) = (
        key("windows", Vendor::Claude, "a"),
        key("windows", Vendor::Claude, "b"),
    );
    let now = t0() + Duration::hours(2);
    let mut red = team("a");
    red.props.light = Light::Red;
    red.props.light_at = t0();
    red.last_activity = now;
    let mut green = team("b");
    green.props.light = Light::Green;
    green.props.light_at = now - Duration::minutes(1);
    green.last_activity = now - Duration::minutes(1);
    let mut w = lock(&e.world);
    w.put_team(a, red, now, true);
    w.put_team(b.clone(), green, now, true);
    let office = OfficeId::Vendor(Vendor::Claude);
    let light = office_dto(&w, &office, false, Vec::new(), t0())
        .unwrap()
        .shared
        .light;
    assert_eq!(light, Light::Green);

    // 상태 기계가 결과 시각을 남긴다
    let mut s = call(t0()).summary;
    s.command_key = Some("cargo test".into());
    let at = 7200 + 30;
    let test_call = EventKind::ToolCall {
        call_id: "t1".into(),
        tool: ToolKind::Test,
        summary: s,
    };
    let test_fail = EventKind::ToolResult {
        call_id: "t1".into(),
        outcome: ToolOutcome::Err,
        outcome_confidence: Confidence::Confirmed,
        preview: String::new(),
        test_result: Some(TestResult {
            passed: 1,
            failed: 1,
        }),
    };
    w.apply(
        &b,
        &[ev("b", at - 5, test_call), ev("b", at, test_fail)],
        now,
    );
    let props = &w.teams[&b].props;
    assert_eq!(
        (props.light, props.light_at),
        (Light::Red, t0() + Duration::seconds(at))
    );
}

fn tool(id: &str, tool: ToolKind, command: &str) -> EventKind {
    let mut summary = call(t0()).summary;
    summary.command = Some(command.into());
    EventKind::ToolCall {
        call_id: id.into(),
        tool,
        summary,
    }
}

fn done(id: &str, outcome: ToolOutcome, test_result: Option<TestResult>) -> EventKind {
    EventKind::ToolResult {
        call_id: id.into(),
        outcome,
        outcome_confidence: Confidence::Confirmed,
        preview: String::new(),
        test_result,
    }
}

/// 작업표·미팅실·리뷰실 재료가 문장이 아니라 카드 값으로 실린다. 재구성해도 같다
#[test]
fn employee_carries_jobs_meeting_and_review() {
    let e = env(Scale::Large);
    let k = key("windows", Vendor::Claude, "a");
    let mut w = lock(&e.world);
    w.put_team(k.clone(), team("a"), t0(), true);
    let id = w.office_of(&k);
    let mut events = Vec::new();
    let mut step = |w: &mut OfficeWorld, secs: i64, kind: EventKind| {
        let ev = ev("a", secs, kind);
        w.apply(&k, std::slice::from_ref(&ev), t0());
        events.push(ev);
        office_dto(w, &id, false, Vec::new(), t0()).unwrap().teams[0].employees[0].clone()
    };
    let shell = || ToolKind::Shell;

    step(&mut w, 1, tool("bg1", shell(), "npm run dev"));
    assert_eq!(
        step(&mut w, 2, done("bg1", ToolOutcome::Started, None)).jobs,
        1
    );
    step(&mut w, 3, tool("bg2", shell(), "npm run watch"));
    assert_eq!(
        step(&mut w, 4, done("bg2", ToolOutcome::Started, None)).jobs,
        2
    );
    assert_eq!(step(&mut w, 5, done("bg1", ToolOutcome::Ok, None)).jobs, 1);

    let mcp = ToolKind::Mcp {
        server: "figma".into(),
        tool: "get_file".into(),
    };
    let b = step(&mut w, 10, tool("m", mcp, ""));
    assert_eq!(
        (b.meeting, b.review),
        (
            Some(Meeting::Mcp {
                server: "figma".into()
            }),
            None
        )
    );
    assert_eq!(
        step(&mut w, 11, done("m", ToolOutcome::Ok, None)).meeting,
        None
    );
    let skill = ToolKind::Skill { name: "pdf".into() };
    assert_eq!(
        step(&mut w, 12, tool("s", skill, "")).meeting,
        Some(Meeting::Skill)
    );
    assert_eq!(
        step(&mut w, 13, done("s", ToolOutcome::Ok, None)).meeting,
        None
    );

    let review = |pens, approved| Some(Review { pens, approved });
    let text = |t: &str| EventKind::AssistantText { text: t.into() };
    assert_eq!(
        step(&mut w, 20, tool("d", ToolKind::GitDiff, "git diff")).review,
        review(0, false)
    );
    step(&mut w, 21, done("d", ToolOutcome::Ok, None));
    assert_eq!(
        step(&mut w, 22, text("`a.rs:3` 경계 오류")).review,
        review(1, false)
    );
    assert_eq!(
        step(&mut w, 23, text("나머지는 lgtm")).review,
        review(1, true)
    );
    let live = step(&mut w, 24, text("`b.rs:9` 도 한 줄"));
    assert_eq!(live.review, review(2, true));

    // 리뷰실을 떠나면 없고, 새로 리뷰를 시작하면 승인이 내려간다
    assert_eq!(
        step(&mut w, 30, tool("x", ToolKind::FileEdit, "")).review,
        None
    );
    step(&mut w, 31, done("x", ToolOutcome::Ok, None));
    let live = step(&mut w, 32, tool("p", ToolKind::PrView, "gh pr view"));
    assert_eq!(live.review, review(0, false));

    let rebuilt = rebuild("a", Product::ClaudeCli, &[], vec![events], utc());
    let again = employee_dto(&rebuilt, &rebuilt.employees[0], rebuilt.last_activity, None);
    assert_eq!(
        (again.jobs, again.meeting, again.review),
        (live.jobs, live.meeting, live.review)
    );
}

/// 문장이 질문·허가 대기로 바뀌어도 진행 호출 수를 보존하고 백그라운드는 합치지 않는다.
#[test]
fn employee_pending_count_survives_waiting_and_rebuild() {
    let mut events = vec![
        ev("a", 1, tool("bg", ToolKind::Shell, "npm run dev")),
        ev("a", 2, done("bg", ToolOutcome::Started, None)),
        ev("a", 3, tool("one", ToolKind::FileRead, "")),
        ev("a", 4, tool("two", ToolKind::FileRead, "")),
    ];
    let mut team = rebuild("a", Product::ClaudeCli, &[], vec![events.clone()], utc());
    team.employees[0].asking = Some("계속할까요?".into());
    team.employees[0].pending[0].summary.escalated = true;
    let dto = employee_dto(&team, &team.employees[0], team.last_activity, None);
    assert_eq!((dto.pending_count, dto.jobs), (2, 1));
    events.push(ev("a", 5, done("one", ToolOutcome::Ok, None)));
    let restored = rebuild("a", Product::ClaudeCli, &[], vec![events.clone()], utc());
    let dto = employee_dto(
        &restored,
        &restored.employees[0],
        restored.last_activity,
        None,
    );
    assert_eq!((dto.pending_count, dto.jobs), (1, 1));
    events.push(ev("a", 6, done("two", ToolOutcome::Ok, None)));
    let restored = rebuild("a", Product::ClaudeCli, &[], vec![events], utc());
    assert_eq!(
        employee_dto(
            &restored,
            &restored.employees[0],
            restored.last_activity,
            None
        )
        .pending_count,
        0
    );
}

/// 결과판: 팀은 마지막 테스트 결과, 공유는 테스트를 가장 늦게 끝낸 팀 것. 숫자 없는 Mixed 는 바꾸지 않는다
#[test]
fn test_board_follows_the_latest_test() {
    let e = env(Scale::Small);
    let (a, b) = (
        key("windows", Vendor::Claude, "a"),
        key("windows", Vendor::Claude, "b"),
    );
    let mut w = lock(&e.world);
    w.put_team(a.clone(), team("a"), t0(), true);
    w.put_team(b.clone(), team("b"), t0(), true);
    let office = OfficeId::Vendor(Vendor::Claude);
    let shared = |w: &OfficeWorld| {
        office_dto(w, &office, false, Vec::new(), t0())
            .unwrap()
            .shared
            .test
    };
    let run = |w: &mut OfficeWorld, k: &TeamKey, secs: i64, mixed: bool, end: EventKind| {
        let mut call = tool("t", ToolKind::Test, "cargo test");
        if let EventKind::ToolCall { summary, .. } = &mut call
            && mixed
        {
            summary.compound = Compound::Mixed;
        }
        let s = k.session.as_str();
        w.apply(k, &[ev(s, secs - 1, call), ev(s, secs, end)], t0());
        w.teams[k].props.test
    };
    let numbers = |passed, failed| Some(TestResult { passed, failed });
    assert_eq!(shared(&w), None);

    let pass = done("t", ToolOutcome::Ok, numbers(3, 0));
    assert_eq!(run(&mut w, &a, 10, false, pass), Some(TestVerdict::Pass));
    let fail = done("t", ToolOutcome::Err, None);
    assert_eq!(run(&mut w, &b, 20, false, fail), Some(TestVerdict::Fail));
    assert_eq!(shared(&w), Some(TestVerdict::Fail));

    // 숫자 없는 Mixed 는 성공이든 실패든 결과판을 건드리지 않는다
    let mixed_ok = done("t", ToolOutcome::Ok, None);
    assert_eq!(run(&mut w, &a, 30, true, mixed_ok), Some(TestVerdict::Pass));
    let mixed_err = done("t", ToolOutcome::Err, None);
    assert_eq!(
        run(&mut w, &a, 32, true, mixed_err),
        Some(TestVerdict::Pass)
    );
    assert_eq!(w.teams[&a].props.test_at, t0() + Duration::seconds(10));
    assert_eq!(shared(&w), Some(TestVerdict::Fail));

    // 숫자가 있는 Mixed 는 추정 도장과 함께 바꾼다
    let mixed_fail = done("t", ToolOutcome::Ok, numbers(2, 1));
    assert_eq!(
        run(&mut w, &a, 40, true, mixed_fail),
        Some(TestVerdict::Fail)
    );
    let pass = done("t", ToolOutcome::Ok, numbers(3, 0));
    assert_eq!(run(&mut w, &a, 50, false, pass), Some(TestVerdict::Pass));
    // 활동이 더 늦은 팀이라도 테스트가 더 이르면 지지 않는다
    w.apply(&b, &[visit("b", 60)], t0());
    assert_eq!(shared(&w), Some(TestVerdict::Pass));
}

/// S58: 졸음이 30분을 넘었는데 프로세스가 살아 있으면 넘는 틱에 대기 중이 켜지고 상태가 나간다.
/// 다시 일하면 꺼지고, 프로세스가 없으면 그대로 퇴근한다
#[test]
fn idle_alive_turns_on_when_the_off_threshold_passes() {
    let e = env(Scale::Large);
    let (a, b) = (
        key("windows", Vendor::Claude, "a"),
        key("windows", Vendor::Claude, "b"),
    );
    {
        let mut w = lock(&e.world);
        let mut alive = team("a");
        alive.alive_in_registry = true;
        w.put_team(a.clone(), alive, t0(), true);
        w.put_team(b.clone(), team("b"), t0(), true);
        let office = w.office_of(&a);
        assert!(w.select(Some(office)));
    }
    let mut sent = Sent::default();
    let tick = |mins: i64, secs: i64| {
        let now = t0() + Duration::minutes(mins) + Duration::seconds(secs);
        lock(&e.world).tick(now, "windows");
        now
    };
    let now = tick(5, 0);
    e.ipc.poll(&mut sent, now);
    let now = tick(13, 0);
    let blanket = e.ipc.poll(&mut sent, now).1.expect("담요 경계를 먼저 발행");
    assert!(blanket.teams[0].employees[0].items.contains(&Item::Blanket));
    let now = tick(30, 0);
    assert!(e.ipc.poll(&mut sent, now).1.is_none(), "30분 정각은 아직");

    let now = tick(30, 1);
    let (list, state) = e.ipc.poll(&mut sent, now);
    let state = state.expect("넘는 틱에 상태가 나간다");
    assert_eq!(
        (state.teams[0].phase, state.teams[0].idle_alive),
        (TeamPhase::Dozing, true)
    );
    let dots: Vec<(String, Dot)> = list
        .unwrap()
        .offices
        .iter()
        .map(|o| (o.teams[0].team_id.clone(), o.dot))
        .collect();
    assert!(
        dots.contains(&(team_id_str(&a), Dot::IdleAlive)),
        "{dots:?}"
    );
    // 프로세스가 없으면 그대로 퇴근
    assert!(dots.contains(&(team_id_str(&b), Dot::Off)), "{dots:?}");
    assert!(!lock(&e.world).teams[&b].idle_alive);

    lock(&e.world).apply(&a, &[visit("a", 31 * 60)], now);
    let state = e.ipc.poll(&mut sent, now).1.unwrap();
    assert_eq!(
        (state.teams[0].phase, state.teams[0].idle_alive),
        (TeamPhase::Working, false)
    );
}

/// 방 번호는 팀 키 전체로 묶는다. 같은 세션 id 라도 제조사·호스트가 다르면 다른 방이다. 저장도 된다
#[test]
fn rooms_are_keyed_by_whole_team_key_and_saved() {
    let e = env(Scale::Large);
    let keys = [
        key("windows", Vendor::Claude, "same"),
        key("windows", Vendor::Codex, "same"),
        key("wsl:Test", Vendor::Claude, "same"),
    ];
    for k in &keys {
        lock(&e.world).put_team(k.clone(), team("same"), t0(), true);
    }
    let offices = e.ipc.snapshot(t0()).list.offices;
    let mut rooms: Vec<u32> = offices.iter().map(|o| o.room.unwrap()).collect();
    assert!(rooms.iter().any(|r| r / 100 == 2), "WSL 은 2층: {rooms:?}");
    // 다른 RoomBook 으로 다시 읽어도 같은 번호다 (잠금 밖에서 저장했다)
    let mut again = RoomBook::load(e._dir.path());
    for k in &keys {
        let host = lock(&e.world).hosts[&k.host].clone();
        let n = again.room_of_unsaved(&team_id_str(k), &host, t0()).0;
        assert!(rooms.contains(&n), "{n} 이 저장되지 않았다: {rooms:?}");
    }
    rooms.sort();
    rooms.dedup();
    assert_eq!(rooms.len(), 3);
}

/// 통신 지연이 바뀌면 그 호스트 팀의 사무실 상태도 다시 나간다 (S46 구름)
#[test]
fn suspect_host_bumps_office_state() {
    let e = env(Scale::Large);
    let k = key("wsl:Test", Vendor::Claude, "a");
    {
        let mut w = lock(&e.world);
        w.put_team(k.clone(), team("a"), t0(), true);
        let office = w.office_of(&k);
        assert!(w.select(Some(office)));
    }
    let mut sent = Sent::default();
    e.ipc.poll(&mut sent, t0());
    lock(&e.world).set_suspect("wsl:Test", true);
    let (list, state) = e.ipc.poll(&mut sent, t0());
    assert!(list.unwrap().offices[0].delayed);
    let state = state.expect("구름이 office-state 로 나간다");
    let team = &state.teams[0];
    assert_eq!(
        (team.delayed, team.host_badge.as_deref()),
        (true, Some("WSL"))
    );
}

/// 프론트 타입 전부. 순서는 파일에 쓰는 순서다
fn bindings() -> String {
    let cfg = Config::new().with_large_int("number");
    let mut out = String::from(
        "// ipc.rs 에서 ts-rs 로 만든 파일이다. 손으로 고치지 않는다. 다시 만드는 명령은 docs/BUILD.md\n",
    );
    macro_rules! decl {
        ($($t:ty),* $(,)?) => {$(
            out.push('\n');
            if let Some(d) = <$t as TS>::docs() {
                out.push_str(&d);
            }
            out.push_str("export ");
            // ts-rs 가 줄 끝에 남기는 공백을 뗀다 (git diff --check 가 잡는다)
            let decl = <$t as TS>::decl(&cfg);
            out.push_str(&decl.lines().map(str::trim_end).collect::<Vec<_>>().join("\n"));
            out.push('\n');
        )*};
    }
    decl!(
        WorldDto,
        OfficeListDto,
        OfficeSummaryDto,
        TeamSummaryDto,
        Dot,
        EmployeeState,
        Border,
        OfficeDto,
        RoomRecentDto,
        RoomEventDto,
        CueDto,
        SelectResponseDto,
        TeamDto,
        Daylight,
        EmployeeDto,
        Item,
        Meeting,
        Review,
        SharedPropsDto,
        BoardStatsDto,
        Activity,
        Bubble,
        BubbleSay,
        Counters,
        TeamProps,
        TestVerdict,
        TestCounts,
        Incident,
        Cue,
        Rank,
        Role,
        WorkType,
        Room,
        Pose,
        TeamPhase,
        Light,
        Scale,
        SettingsDto,
        NightMode,
        Animal,
        DiscoveryPathDto,
        DiscoveryStatus,
        ManualPath,
        crate::settings::ManualVendor,
        StepState,
        StartupSummary,
        StartupDetail,
        StartupProgressDto,
        Confidence,
        TodoItem,
        TodoStatus,
        CursorDto,
        ClipDto,
        EventDto,
        TestDto,
        ToolOutcome,
        Tokens,
        TimelineKind,
        EmployeeDetailDto,
        CountDto,
        HeadcountDto,
        PrDto,
        SessionDetailDto,
        OpenTarget,
        DetailError,
    );
    out
}

/// 생성 결과와 저장소 파일이 같아야 한다. `OWLFICE_WRITE_TYPES=1` 이면 파일을 다시 쓴다
#[test]
fn ts_bindings_are_fresh() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/types/ipc.ts");
    let want = bindings();
    if std::env::var_os("OWLFICE_WRITE_TYPES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "src/types/ipc.ts 가 낡았다. docs/BUILD.md 의 타입 생성 명령으로 다시 만든다"
    );
}

/// docs/CONTRACTS.md C13 "상태 직렬화 + emit < 2ms". 측정만 한다:
/// `cargo test --release --manifest-path src-tauri/Cargo.toml serialize_time -- --ignored --nocapture`
#[test]
#[ignore]
fn serialize_time() {
    let e = env(Scale::Small);
    let mut w = lock(&e.world);
    for rel in FIXTURES {
        let (k, t) = fixture_team(rel);
        let now = t.last_activity;
        w.put_team(k, t, now, true);
    }
    let id = OfficeId::Vendor(Vendor::Claude);
    let dto = office_dto(&w, &id, false, Vec::new(), t0()).unwrap();
    let employees: usize = dto.teams.iter().map(|t| t.employees.len()).sum();
    let n = 2000;
    let start = Instant::now();
    let mut bytes = 0;
    for _ in 0..n {
        let dto = office_dto(&w, &id, false, Vec::new(), t0()).unwrap();
        bytes = serde_json::to_string(&dto).unwrap().len();
    }
    let avg = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
    println!(
        "OfficeDto 만들기+직렬화 평균 {avg:.4}ms (팀 {}, 직원 {employees}, {bytes}바이트)",
        dto.teams.len()
    );
}

#[test]
fn extension_dtos_use_recorded_paths_counts_and_snapshot_medal_time() {
    let e = env(Scale::Large);
    let mut w = lock(&e.world);
    let (plan_key, plan) = fixture_team("claude/planning-state");
    let plan_time = plan.last_activity;
    w.put_team(plan_key.clone(), plan, plan_time, true);
    let id = w.office_of(&plan_key);
    let dto = office_dto(&w, &id, true, Vec::new(), plan_time).unwrap();
    let employee = &dto.teams[0].employees[0];
    assert!(employee.planning);
    assert_eq!((employee.unique_reads, employee.unique_edits), (1, 1));

    let (test_key, tests) = fixture_team("claude/test-streak");
    let end = tests.last_activity;
    w.put_team(test_key.clone(), tests, end, true);
    let id = w.office_of(&test_key);
    let dto = office_dto(&w, &id, true, Vec::new(), end).unwrap();
    assert_eq!(
        dto.teams[0].tests_today,
        TestCounts {
            passed: 5,
            failed: 1
        }
    );
    assert_eq!(dto.teams[0].commits_today, 1);
    assert!(dto.teams[0].employees[0].medal);
    let expiry = t0() + Duration::seconds(312);
    assert!(
        office_dto(
            &w,
            &id,
            true,
            Vec::new(),
            expiry - Duration::milliseconds(1)
        )
        .unwrap()
        .teams[0]
            .employees[0]
            .medal
    );
    assert!(!office_dto(&w, &id, true, Vec::new(), expiry).unwrap().teams[0].employees[0].medal);
    assert!(
        !office_dto(&w, &id, true, Vec::new(), expiry + Duration::hours(1))
            .unwrap()
            .teams[0]
            .employees[0]
            .medal
    );
}

#[test]
fn employee_check_border_uses_own_calls_and_rebuilds() {
    let other = |secs, kind| Event {
        agent_id: "b".into(),
        parent_agent: Some("a".into()),
        ..ev("a", secs, kind)
    };
    let events = vec![
        ev("a", 0, tool("test", ToolKind::Test, "cargo test")),
        other(1, tool("build", ToolKind::Build, "cargo build")),
        ev("a", 2, done("test", ToolOutcome::Err, None)),
        other(3, done("build", ToolOutcome::Ok, None)),
        ev("a", 4, tool("build", ToolKind::Build, "cargo build")),
        ev("a", 5, tool("test", ToolKind::Test, "cargo test")),
        ev("a", 6, done("build", ToolOutcome::Started, None)),
        ev("a", 7, done("test", ToolOutcome::Ok, None)),
        ev("a", 8, done("build", ToolOutcome::Unknown, None)),
    ];
    let mut team = Team::new("a", Product::ClaudeCli, t0());
    for (i, event) in events.iter().enumerate() {
        let now = event.ts.unwrap();
        team.tick(now, utc(), &mut Vec::new());
        team.apply(event, &mut Vec::new());
        let current = employee_dto(&team, &team.employees[0], now, None);
        let expected = match i {
            0..=1 | 4..=7 => Border::Running,
            2..=3 => Border::Failed,
            _ => Border::Plain,
        };
        assert_eq!(current.build_test_border, expected);
        let json = serde_json::to_value(&current).unwrap();
        assert_eq!(
            json["buildTestBorder"],
            serde_json::to_value(expected).unwrap()
        );
        if i >= 1 {
            assert_eq!(
                employee_dto(&team, &team.employees[1], now, None).build_test_border,
                if i < 3 {
                    Border::Running
                } else {
                    Border::Plain
                }
            );
        }
        let rebuilt = rebuild(
            "a",
            Product::ClaudeCli,
            &[],
            vec![events[..=i].to_vec()],
            utc(),
        );
        for (live, restored) in team.employees.iter().zip(&rebuilt.employees) {
            assert_eq!(
                employee_dto(&team, live, now, None).build_test_border,
                employee_dto(&rebuilt, restored, now, None).build_test_border
            );
        }
    }
}

#[test]
fn rejection_dto_uses_hold_time_even_after_a_compaction_sentence() {
    let mut team = Team::new("a", Product::ClaudeCli, t0());
    team.employees[0].rejected_at = Some(t0());
    team.employees[0].activity.sentence = "분갈이 중".into();
    assert!(employee_dto(&team, &team.employees[0], t0() + Duration::seconds(1), None).rejected);
    assert!(!employee_dto(&team, &team.employees[0], t0() + Duration::seconds(4), None).rejected);
    team.employees[0].activity.sentence = "반려: 옛 호출".into();
    assert!(!employee_dto(&team, &team.employees[0], t0() + Duration::seconds(5), None).rejected);
}

#[test]
fn blanket_boundary_publishes_once_and_off_removes_it() {
    let e = env(Scale::Large);
    let k = key("windows", Vendor::Claude, "blanket");
    let at = |secs| t0() + Duration::seconds(secs);
    {
        let mut w = lock(&e.world);
        w.put_team(k.clone(), team("blanket"), at(779), true);
        let id = w.office_of(&k);
        w.select(Some(id));
    }
    let mut sent = Sent::default();
    let before = e.ipc.poll(&mut sent, at(779)).1.unwrap();
    assert!(!before.teams[0].employees[0].items.contains(&Item::Blanket));
    lock(&e.world).tick(at(780), "windows");
    let after = e.ipc.poll(&mut sent, at(780)).1.expect("담요 경계 발행");
    assert!(after.state_seq > before.state_seq);
    assert!(after.teams[0].employees[0].items.contains(&Item::Blanket));
    lock(&e.world).tick(at(781), "windows");
    assert!(e.ipc.poll(&mut sent, at(781)).1.is_none());
    lock(&e.world).tick(at(1801), "windows");
    let off = e.ipc.poll(&mut sent, at(1801)).1.unwrap();
    assert!(!off.teams[0].employees[0].items.contains(&Item::Blanket));
}

#[test]
fn item_candidates_use_employee_state_and_medal_expiry() {
    let mut t = team("items");
    t.employees[0].medal_until = Some(t0() + Duration::seconds(10));
    t.employees[0].activity.room = Room::Incident;
    t.employees[0].blanket = true;
    t.employees[0].background.push(call(t0()));
    t.employees[0].planning = true;
    assert_eq!(
        employee_dto(&t, &t.employees[0], t0(), None).items,
        [
            Item::Medal,
            Item::Helmet,
            Item::Blanket,
            Item::Headphones,
            Item::Scroll
        ]
    );
    assert!(
        !employee_dto(&t, &t.employees[0], t0() + Duration::seconds(10), None)
            .items
            .contains(&Item::Medal)
    );
    t.employees[0].activity.room = Room::Door;
    t.employees[0].blanket = false;
    t.employees[0].background.clear();
    assert_eq!(
        employee_dto(&t, &t.employees[0], t0() + Duration::seconds(10), None).items,
        [Item::Scroll]
    );
}
