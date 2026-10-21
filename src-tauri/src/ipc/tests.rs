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
    }
}

/// 부장 + 서브 하나. 아무 대기도 없는 근무 중 팀
fn team(session: &str) -> Team {
    let mut t = Team::new(session, Product::ClaudeCli, t0());
    t.preregister(&format!("{session}-s"), None, None, None, 1, t0());
    t
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

/// 3.3 범례의 각 경우
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

/// 중소기업 층 점: 장애 > 근무 > 결재·응답 대기 > 졸음 > 퇴근. 결재판은 어느 팀이든
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
            (Some(1), "1층 Claude", None)
        );
    }
    let mut w = lock(&e.world);
    let k = key("windows", Vendor::Claude, "off");
    w.teams.get_mut(&k).unwrap().employees[0].asking = Some("q".into());
    drop(w);
    assert!(e.ipc.snapshot(now).list.offices[0].asking);
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
        let dto = office_dto(&w, &id, false, Vec::new()).unwrap();
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

    // 미대응 기록(4.11)은 office-state 에 실리지 않는다
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
/// 규모를 바꾸면 그 전 연출은 버리고 rebuilt 로 내며 목록이 바뀐 선택을 알린다 (4.7.9 2·4·5번)
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

/// 설정으로 규모를 바꾸면 world 가 재배치하고 목록·상태가 rebuilt 로 나간다 (4.7.9).
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
    apply_settings(&e.ipc, &book, SettingsDto::default()).unwrap();
    assert!(e.ipc.poll(&mut sent, now).0.is_none());

    apply_settings(&e.ipc, &book, small.clone()).unwrap();
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
    assert!(apply_settings(&e.ipc, &broken, large).is_err());
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

    apply_settings(&e.ipc, &book, name("야근상사")).unwrap();
    let want = SettingsDto {
        scale: Some(Scale::Small),
        company_name: Some("야근상사".into()),
    };
    assert_eq!(settings_dto(&SettingsBook::load(dir.path())), want);
    // 규모를 건드리지 않았으니 재배치도 없다
    assert!(e.ipc.poll(&mut sent, now).0.is_none());

    // 빈 칸(null)은 이름을 건드리지 않는다
    apply_settings(&e.ipc, &book, SettingsDto::default()).unwrap();
    assert_eq!(settings_dto(&book), want);

    apply_settings(&e.ipc, &book, name("")).unwrap();
    let dto = settings_dto(&SettingsBook::load(dir.path()));
    assert_eq!(dto.company_name, None);
    assert_eq!(dto.scale, Some(Scale::Small));
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
    let light = office_dto(&w, &office, false, Vec::new())
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
        office_dto(w, &id, false, Vec::new()).unwrap().teams[0].employees[0].clone()
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
    let again = employee_dto(&rebuilt.employees[0]);
    assert_eq!(
        (again.jobs, again.meeting, again.review),
        (live.jobs, live.meeting, live.review)
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
        office_dto(w, &office, false, Vec::new())
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
            out.push_str(&<$t as TS>::decl(&cfg));
            out.push('\n');
        )*};
    }
    decl!(
        WorldDto,
        OfficeListDto,
        OfficeSummaryDto,
        TeamSummaryDto,
        Dot,
        Border,
        OfficeDto,
        CueDto,
        SelectResponseDto,
        TeamDto,
        EmployeeDto,
        Meeting,
        Review,
        SharedPropsDto,
        Activity,
        Bubble,
        BubbleSay,
        Counters,
        TeamProps,
        TestVerdict,
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

/// 4.10 "상태 직렬화 + emit < 2ms". 측정만 한다:
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
    let dto = office_dto(&w, &id, false, Vec::new()).unwrap();
    let employees: usize = dto.teams.iter().map(|t| t.employees.len()).sum();
    let n = 2000;
    let start = Instant::now();
    let mut bytes = 0;
    for _ in 0..n {
        let dto = office_dto(&w, &id, false, Vec::new()).unwrap();
        bytes = serde_json::to_string(&dto).unwrap().len();
    }
    let avg = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
    println!(
        "OfficeDto 만들기+직렬화 평균 {avg:.4}ms (팀 {}, 직원 {employees}, {bytes}바이트)",
        dto.teams.len()
    );
}
