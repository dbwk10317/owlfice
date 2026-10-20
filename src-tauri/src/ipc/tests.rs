use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset};
use ts_rs::{Config, TS};

use super::*;
use crate::office::classify::rebuild;
use crate::office::names::NameBook;
use crate::office::{PendingCall, Pose, Room};
use crate::parse::{Compound, Event, EventKind, RawRef, TodoStatus, ToolKind, ToolSummary};
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

fn env(scale: Scale) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let world = Arc::new(Mutex::new(OfficeWorld::new(
        scale,
        NameBook::load(dir.path()),
        utc(),
    )));
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
    let w = OfficeWorld::new(
        Scale::Large,
        NameBook::load(tempfile::tempdir().unwrap().path()),
        utc(),
    );
    let k = key("windows", Vendor::Claude, "s");
    let row = |t: &Team| team_summary(&w, &k, t, now);

    let base = team("s");
    let r = row(&base);
    assert_eq!(
        (r.dot, r.asking, r.border, r.delayed),
        (Dot::Working, false, Border::Plain, false)
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
            w.put_team(k.clone(), t, now);
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
        w.put_team(k.clone(), t, now);
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

fn usage(session: &str, secs: i64, input_total: u64) -> Event {
    Event {
        ts: Some(t0() + Duration::seconds(secs)),
        session_id: session.into(),
        agent_id: session.into(),
        parent_agent: None,
        kind: EventKind::TokenUsage {
            input_total,
            output: 0,
            context_window: Some(1000),
        },
        raw_ref: RawRef {
            path: PathBuf::new(),
            generation: 0,
            byte_offset: 0,
            line_no: 0,
        },
    }
}

/// 번호가 바뀔 때만 내고, 연출은 선택 사무실 것만 한 번 싣는다. 재구성 뒤 첫 상태는 rebuilt 이고 출근만 싣는다
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
        w.put_team(a.clone(), team("a"), now);
        w.put_team(b.clone(), team("b"), now);
        w.outbox.push((OfficeId::Team(a.clone()), Cue::Truck));
        w.outbox
            .push((OfficeId::Team(a.clone()), Cue::Enter { agent: "a".into() }));
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
    assert_eq!(state.cues, vec![Cue::Enter { agent: "a".into() }]);

    // 바뀐 것이 없으면 아무것도 안 낸다
    let (list, state) = e.ipc.poll(&mut sent, now);
    assert!(list.is_none() && state.is_none());

    // 선택 안 된 사무실이 바뀌면 상태는 안 낸다. 그 사무실 연출은 버린다
    {
        let mut w = lock(&e.world);
        w.apply(&b, &[usage("b", 1, 500)], now);
        w.outbox.push((OfficeId::Team(b.clone()), Cue::Fireworks));
    }
    let (_, state) = e.ipc.poll(&mut sent, now);
    assert!(state.is_none());

    // 선택 사무실이 바뀌면 낸다. rebuilt 가 아니고 연출을 싣고 비운다
    let seq = {
        let mut w = lock(&e.world);
        w.apply(&a, &[usage("a", 2, 500)], now);
        w.outbox.push((OfficeId::Team(a.clone()), Cue::Truck));
        w.state_seq[&w.office_of(&a)]
    };
    let (_, state) = e.ipc.poll(&mut sent, now);
    let state = state.unwrap();
    assert_eq!((state.state_seq, state.rebuilt), (seq, false));
    assert_eq!(state.cues, vec![Cue::Truck]);
    assert!(lock(&e.world).outbox.is_empty());
    assert!(e.ipc.poll(&mut sent, now).1.is_none());

    // 번호가 같아도 늦게 온 연출은 낸다 (출근 cue 는 재구성보다 한 틱 늦게 넘어온다)
    {
        let mut w = lock(&e.world);
        w.outbox
            .push((OfficeId::Team(a.clone()), Cue::Enter { agent: "x".into() }));
    }
    let state = e.ipc.poll(&mut sent, now).1.unwrap();
    assert_eq!((state.state_seq, state.cues.len()), (seq, 1));
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
        w.put_team(k.clone(), t, now);
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
    lock(&e.world).put_team(k.clone(), team("a"), t0());
    let id = office_id_str(&OfficeId::Team(k));
    let r = e.ipc.select(&id, "r7".into()).unwrap();
    assert_eq!(
        (r.request_id.as_str(), r.office.office_id.as_str()),
        ("r7", id.as_str())
    );
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["requestId"], "r7");
    assert!(json["stateSeq"].is_u64());
    assert!(
        e.ipc.select("vendor/claude", "r8".into()).is_err(),
        "대기업에 층 사무실은 없다"
    );
    assert!(e.ipc.select("../etc", "r9".into()).is_err());
    assert!(e.ipc.snapshot(t0()).selected.is_some());
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
        SelectResponseDto,
        TeamDto,
        EmployeeDto,
        SharedPropsDto,
        Activity,
        Counters,
        TeamProps,
        Incident,
        Cue,
        Rank,
        Room,
        Pose,
        TeamPhase,
        Light,
        Scale,
        Product,
        Confidence,
        TodoItem,
        TodoStatus,
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
        w.put_team(k, t, now);
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
