use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use chrono::{DateTime, FixedOffset, Utc};

use super::*;
use crate::diag::Diag;
use crate::discover::{ReadMode, Vendor};
use crate::office::names::NameBook;
use crate::office::world::Scale;
use crate::settings::SettingsBook;
use crate::tail::scheduler::{Heartbeat, HostLoop, LogicalClock, Prober, spawn, spawn_probe};

fn t0() -> DateTime<Utc> {
    "2026-03-02T09:00:00Z".parse().unwrap()
}

fn wsl_host(name: &str) -> Host {
    Host {
        id: format!("wsl:{name}"),
        kind: HostKind::Wsl {
            distro: name.into(),
        },
        label: name.into(),
    }
}

/// `<root>/<프로젝트>/<세션>.jsonl` 에 사장님 지시 한 줄
fn session(root: &Path, project: &str, id: &str) {
    let line = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": "hi"},
        "uuid": format!("u-{id}"),
        "timestamp": "2026-03-02T09:00:00.000Z",
        "entrypoint": "claude-desktop",
        "cwd": "C:\\dir1",
        "sessionId": id,
    });
    let dir = root.join(project);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{id}.jsonl")), format!("{line}\n")).unwrap();
}

fn claude_site(host: Host, root: &Path) -> Site {
    Site {
        host,
        vendor: Vendor::Claude,
        root: root.to_path_buf(),
        read_mode: ReadMode::Append,
    }
}

fn distro(name: &str, running: Option<bool>, sites: Vec<Site>) -> WslDistro {
    WslDistro {
        host: wsl_host(name),
        running,
        sites,
    }
}

/// 가짜 Site·가짜 Probe 를 진짜 호스트 스레드에 물린다
struct FakeBoot {
    sites: Vec<Site>,
    distros: Vec<WslDistro>,
    /// false 면 Probe 를 띄우지 않아 WSL 호스트가 영영 안 뜬다 (멈춘 공유)
    probe: bool,
    world: Arc<Mutex<OfficeWorld>>,
    diag: Arc<Mutex<Diag>>,
    stop: Mutex<Option<Arc<Heartbeat>>>,
    settings: Arc<SettingsBook>,
    /// true 면 discover 에서 패닉한다
    panic: bool,
    sidebar: AtomicUsize,
    publish: AtomicUsize,
}

impl Boot for FakeBoot {
    fn wait_for_scale(&self) {
        self.settings.wait_scale();
    }

    fn discover(&self) -> (Vec<Site>, Vec<WslDistro>) {
        assert!(!self.panic, "discover 가 터졌다");
        (self.sites.clone(), self.distros.clone())
    }

    fn spawn_hosts(&self, sites: Vec<Site>, distros: Vec<WslDistro>, watch: Arc<ScanWatch>) {
        let mut host = HostLoop::new(
            windows_host(),
            sites.clone(),
            self.world.clone(),
            Arc::new(LogicalClock::new(t0(), 0.0)),
            self.diag.clone(),
        );
        host.watch = Some(watch);
        *self.stop.lock().unwrap() = Some(host.heartbeat.clone());
        if self.probe {
            let prober = Prober::new(&host, Box::new(move || (sites.clone(), distros.clone())));
            spawn_probe(prober);
        }
        spawn(host);
    }

    fn sidebar(&self) {
        self.sidebar.fetch_add(1, Ordering::SeqCst);
    }

    fn publish(&self) {
        self.publish.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for FakeBoot {
    fn drop(&mut self) {
        if let Some(hb) = self.stop.lock().unwrap().as_ref() {
            hb.stop.store(true, Ordering::Relaxed);
        }
    }
}

fn boot(
    tmp: &Path,
    sites: Vec<Site>,
    distros: Vec<WslDistro>,
    probe: bool,
) -> (FakeBoot, Arc<Mutex<OfficeWorld>>) {
    let world = Arc::new(Mutex::new(OfficeWorld::new(
        Scale::Large,
        NameBook::load(tmp),
        FixedOffset::east_opt(0).unwrap(),
    )));
    let b = FakeBoot {
        sites,
        distros,
        probe,
        world: world.clone(),
        diag: Arc::new(Mutex::new(Diag::open(tmp, false))),
        stop: Mutex::new(None),
        // 규모가 이미 정해진 평소 실행
        settings: {
            let book = Arc::new(SettingsBook::load(tmp));
            book.set_scale(Scale::Large).unwrap();
            book
        },
        panic: false,
        sidebar: AtomicUsize::new(0),
        publish: AtomicUsize::new(0),
    };
    (b, world)
}

/// 발행된 이벤트를 모으며 기동을 끝까지 돌린다
fn run_all(b: &FakeBoot, world: &Mutex<OfficeWorld>, lim: &Limits) -> Vec<StartupProgressDto> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let s = Startup::new(move |p| sink.lock().unwrap().push(p.clone()));
    run(&s, b, world, lim);
    seen.lock().unwrap().clone()
}

fn quick() -> Limits {
    Limits {
        step: Duration::from_secs(20),
        tick: Duration::from_millis(50),
    }
}

/// 본줄의 (단계, 상태) 중 진행 중 갱신은 접고 순서만 본다
fn order(events: &[StartupProgressDto]) -> Vec<(u8, StepState)> {
    let mut out: Vec<(u8, StepState)> = Vec::new();
    for e in events.iter().filter(|e| !e.summary.is_note()) {
        let at = (e.step, e.state);
        if out.last() != Some(&at) {
            out.push(at);
        }
    }
    out
}

fn last(events: &[StartupProgressDto]) -> Option<(u8, StepState)> {
    events.last().map(|e| (e.step, e.state))
}

#[test]
fn steps_come_in_order_with_summaries() {
    let tmp = tempfile::tempdir().unwrap();
    let (win, rocky) = (tmp.path().join("win"), tmp.path().join("rocky"));
    session(&win, "p1", "s1");
    session(&win, "p2", "s2");
    session(&rocky, "p1", "s3");
    let (b, world) = boot(
        tmp.path(),
        vec![claude_site(windows_host(), &win)],
        vec![distro(
            "Rocky",
            Some(true),
            vec![claude_site(wsl_host("Rocky"), &rocky)],
        )],
        true,
    );
    let events = run_all(&b, &world, &quick());

    let mut want = Vec::new();
    for step in 2..=7u8 {
        want.push((step, StepState::Running));
        want.push((step, StepState::Done));
    }
    assert_eq!(order(&events), want);

    let done = |step: u8| {
        events
            .iter()
            .rfind(|e| e.step == step && e.state == StepState::Done)
            .unwrap()
            .summary
            .clone()
    };
    assert_eq!(
        done(2),
        StartupSummary::Candidates {
            n: 2,
            hosts: vec!["Windows".into(), "WSL Rocky".into()]
        }
    );
    assert_eq!(
        done(3),
        StartupSummary::Rented {
            buildings: vec!["Windows Claude".into(), "WSL Rocky Claude".into()]
        }
    );
    // 방금 만든 파일 셋이 모두 문턱 안이다
    assert_eq!(done(4), StartupSummary::Teams { n: 3 });
    assert_eq!(
        done(5),
        StartupSummary::Hired {
            n: 3,
            bujang: 3,
            gwajang: 0,
            daeri: 0,
            sawon: 0
        }
    );
    assert_eq!(b.sidebar.load(Ordering::SeqCst), 1);
    assert_eq!(b.publish.load(Ordering::SeqCst), 1);
}

#[test]
fn powered_off_wsl_is_lights_out_not_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let (b, world) = boot(
        tmp.path(),
        Vec::new(),
        vec![distro("Rocky", Some(false), Vec::new())],
        true,
    );
    let events = run_all(&b, &world, &quick());
    let off = events
        .iter()
        .find(|e| matches!(e.summary, StartupSummary::WslOff { .. }))
        .unwrap();
    assert_eq!(off.step, 3);
    assert_eq!(off.state, StepState::Done);
    assert_eq!(
        off.summary,
        StartupSummary::WslOff {
            floor: 2,
            distro: "Rocky".into()
        }
    );
    assert!(events.iter().all(|e| e.state != StepState::Warn));
}

#[test]
fn unknown_wsl_state_is_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let (b, world) = boot(
        tmp.path(),
        Vec::new(),
        vec![distro("Alpine", None, Vec::new())],
        true,
    );
    let events = run_all(&b, &world, &quick());
    let fail = events
        .iter()
        .find(|e| matches!(e.summary, StartupSummary::WslFail { .. }))
        .unwrap();
    assert_eq!((fail.step, fail.state), (3, StepState::Warn));
}

#[test]
fn no_session_folder_fails_step_three_and_goes_on() {
    let tmp = tempfile::tempdir().unwrap();
    let (b, world) = boot(tmp.path(), Vec::new(), Vec::new(), true);
    let events = run_all(&b, &world, &quick());
    assert!(events.iter().any(|e| e.step == 3
        && e.state == StepState::Error
        && e.summary == StartupSummary::NoFolders));
    assert_eq!(last(&events), Some((7, StepState::Done)));
}

#[test]
fn unresponsive_wsl_gets_a_cause_then_a_warning_and_the_rest_goes_on() {
    let tmp = tempfile::tempdir().unwrap();
    let (win, rocky) = (tmp.path().join("win"), tmp.path().join("rocky"));
    session(&win, "p1", "s1");
    session(&rocky, "p1", "s2");
    // Probe 를 안 띄워 Rocky 호스트가 영영 나열하지 않는다
    let (b, world) = boot(
        tmp.path(),
        vec![claude_site(windows_host(), &win)],
        vec![distro(
            "Rocky",
            Some(true),
            vec![claude_site(wsl_host("Rocky"), &rocky)],
        )],
        false,
    );
    let lim = Limits {
        step: Duration::from_millis(400),
        tick: Duration::from_millis(50),
    };
    let events = run_all(&b, &world, &lim);
    assert!(events.iter().any(|e| e.step == 4
        && e.state == StepState::Running
        && e.detail
            == Some(StartupDetail::Wsl {
                distro: "Rocky".into()
            })));
    let fails: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.summary, StartupSummary::WslFail { .. }))
        .collect();
    // 4단계에서 포기했으니 5단계는 다시 기다리지 않는다
    assert_eq!(fails.len(), 1);
    assert_eq!((fails[0].step, fails[0].state), (4, StepState::Warn));
    assert_eq!(last(&events), Some((7, StepState::Done)));
}

#[test]
fn read_progress_is_at_most_once_per_tick() {
    let watch = Arc::new(ScanWatch::default());
    watch.listed("windows", 100, Vec::new());
    let w = watch.clone();
    let feeder = thread::spawn(move || {
        for _ in 0..100 {
            w.read(1);
            thread::sleep(Duration::from_millis(2));
        }
        w.finished("windows");
    });
    let count = AtomicUsize::new(0);
    let ok = watch.wait(
        Instant::now() + Duration::from_secs(10),
        Duration::from_millis(100),
        |st| st.finished.contains("windows"),
        |st| read_progress(st, &[]),
        |_| {
            count.fetch_add(1, Ordering::SeqCst);
        },
    );
    feeder.join().unwrap();
    assert!(ok);
    // 읽는 데 걸린 시간을 100ms 로 나눈 만큼이 상한이다. 읽기마다 내면 100번이다
    assert!(count.load(Ordering::SeqCst) < 20);
}

#[test]
fn late_subscriber_catches_up_from_the_log() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let s = Startup::new(move |p| sink.lock().unwrap().push(p.clone()));
    s.report(4, StepState::Running, StartupSummary::None, None);
    s.report(4, StepState::Done, StartupSummary::Teams { n: 4 }, None);
    s.report(5, StepState::Running, StartupSummary::None, None);
    let files = |done| Some(StartupDetail::Files { done, total: 312 });
    s.report(5, StepState::Running, StartupSummary::None, files(100));
    s.report(5, StepState::Running, StartupSummary::None, files(187));
    let off = StartupSummary::WslOff {
        floor: 2,
        distro: "Rocky".into(),
    };
    s.report(5, StepState::Done, off.clone(), None);
    // 같은 알림 줄을 또 내도 하나다
    s.report(5, StepState::Done, off.clone(), None);

    assert_eq!(seen.lock().unwrap().len(), 7);
    let log = s.progress();
    // 단계 4 끝, 단계 5 는 마지막 진행(187/312), 알림 줄 하나
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].summary, StartupSummary::Teams { n: 4 });
    assert_eq!(log[1].detail, files(187));
    assert_eq!(log[2].summary, off);
}

#[test]
fn candidates_count_only_running_distros_with_session_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let rocky = tmp.path().join("rocky");
    session(&rocky, "p1", "s1");
    let (b, world) = boot(
        tmp.path(),
        Vec::new(),
        vec![
            distro(
                "Rocky",
                Some(true),
                vec![claude_site(wsl_host("Rocky"), &rocky)],
            ),
            distro("Off", Some(false), Vec::new()),
            distro("docker-desktop", Some(true), Vec::new()),
        ],
        true,
    );
    let events = run_all(&b, &world, &quick());
    let c = events
        .iter()
        .find(|e| e.step == 2 && e.state == StepState::Done)
        .unwrap();
    assert_eq!(
        c.summary,
        StartupSummary::Candidates {
            n: 2,
            hosts: vec!["Windows".into(), "WSL Rocky".into()]
        }
    );
}

/// 불 켜진 팀은 호스트 스레드가 모아 준 수정 시각으로 센다. 기동 스레드는 경로를 모른다
#[test]
fn lit_teams_counts_by_collected_mtimes() {
    let watch = ScanWatch::default();
    let now = SystemTime::now();
    let old = now - LIT_WITHIN - Duration::from_secs(60);
    watch.listed("windows", 3, vec![now, old]);
    watch.listed("wsl:Rocky", 1, vec![now - Duration::from_secs(10)]);
    assert_eq!(lit_teams(&watch), 2);
}

/// 4단계에서 이미 쓴 시간은 5단계 몫에서 빠진다 (단계마다 한도를 받으면 멈춘 호스트 하나로 두 배가 걸린다)
#[test]
fn steps_four_and_five_share_one_deadline() {
    let watch = ScanWatch::default();
    // a 는 나열했지만 끝내지 못하고, b 는 나열도 못 한다
    watch.listed("wsl:a", 1, Vec::new());
    let mut waits: Vec<Expect> = ["a", "b"]
        .iter()
        .enumerate()
        .map(|(i, n)| Expect {
            id: format!("wsl:{n}"),
            distro: Some((*n).into()),
            floor: i as u32 + 2,
        })
        .collect();
    let s = Startup::new(|_| {});
    let tick = Duration::from_millis(20);
    let started = Instant::now();
    let until = Until {
        deadline: started + Duration::from_millis(300),
        tick,
    };
    await_hosts(
        &s,
        &watch,
        &mut waits,
        4,
        until,
        listed_or_finished,
        pending_wsl,
    );
    assert_eq!(waits.len(), 1);
    await_hosts(
        &s,
        &watch,
        &mut waits,
        5,
        until,
        |st, id| st.finished.contains(id),
        read_progress,
    );
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(300), "{took:?}");
    assert!(took < Duration::from_millis(500), "{took:?}");
    assert!(waits.is_empty());
}

#[test]
fn panic_in_startup_still_ends_every_step_with_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut b, world) = boot(tmp.path(), Vec::new(), Vec::new(), true);
    b.panic = true;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let s = Startup::new(move |p| sink.lock().unwrap().push(p.clone()));
    run_guarded(&s, &b, &world, &quick());
    let log = s.progress();
    // 2단계는 시작했다가 끝내지 못했고, 나머지는 시작도 못 했다. 모두 Error 로 닫힌다
    for step in 2..=7u8 {
        let line = log.iter().find(|e| e.step == step).unwrap();
        assert_eq!(line.state, StepState::Error, "{step}");
    }
    assert_eq!(last(&seen.lock().unwrap()), Some((7, StepState::Error)));
}

/// 첫 실행(규모 없음)은 규모를 고르기 전에 2단계가 나가지 않는다 (docs/CONTRACTS.md C4)
#[test]
fn nothing_runs_until_the_scale_is_chosen() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut b, world) = boot(tmp.path(), Vec::new(), Vec::new(), true);
    let empty = tempfile::tempdir().unwrap();
    b.settings = Arc::new(SettingsBook::load(empty.path()));
    let settings = b.settings.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let runner = thread::spawn(move || {
        let s = Startup::new(move |p| sink.lock().unwrap().push(p.clone()));
        run(&s, &b, &world, &quick());
    });
    thread::sleep(Duration::from_millis(300));
    assert!(seen.lock().unwrap().is_empty());
    settings.set_scale(Scale::Small).unwrap();
    runner.join().unwrap();
    let events = seen.lock().unwrap().clone();
    assert_eq!(events.first().map(|e| e.step), Some(2));
    assert_eq!(last(&events), Some((7, StepState::Done)));
}
