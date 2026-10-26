use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

fn scanner() -> (Scanner, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    (
        Scanner::new(move || {
            let _ = tx.send(());
        }),
        rx,
    )
}

fn finished(wake: &mpsc::Receiver<()>) {
    wake.recv_timeout(Duration::from_secs(5)).unwrap();
}

fn states() -> DistroStates {
    vec!["Blocked", "Healthy"]
        .into_iter()
        .map(|name| {
            (
                wsl::LxssEntry {
                    name: name.into(),
                    default_uid: None,
                },
                Some(true),
            )
        })
        .collect()
}

fn remote(entry: wsl::LxssEntry, roots: Roots) -> ScanResult {
    scan_roots(
        vec![wsl::WslDistro {
            host: Host {
                id: format!("wsl:{}", entry.name),
                kind: HostKind::Wsl {
                    distro: entry.name.clone(),
                },
                label: entry.name,
            },
            running: Some(true),
            sites: Vec::new(),
        }],
        Vec::new(),
        roots,
        &|_| true,
    )
}

#[test]
fn blocked_distro_does_not_delay_healthy_distro_or_accumulate_workers() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let roots = vec![
        (Vendor::Claude, PathBuf::from(r"\\wsl$\Blocked\projects")),
        (Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\projects")),
    ];
    scanner.scan_with(roots.clone(), &|_| panic!(), now, states, remote);
    finished(&wake);
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let (entered, started) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    scanner.scan_with(
        roots.clone(),
        &|_| panic!(),
        now,
        states,
        move |entry, roots| {
            if entry.name == "Blocked" {
                count.fetch_add(1, Ordering::SeqCst);
                entered.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
            }
            remote(entry, roots)
        },
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    finished(&wake);
    let found = scanner.scan_with(roots.clone(), &|_| panic!(), now, states, remote);
    assert_eq!(
        found
            .distros
            .iter()
            .find(|d| d.host.label == "Healthy")
            .unwrap()
            .sites
            .len(),
        1
    );
    assert_eq!(
        found
            .distros
            .iter()
            .find(|d| d.host.label == "Blocked")
            .unwrap()
            .running,
        None
    );
    let later = now + REMOTE_LIMIT * 2;
    scanner.scan_with(roots.clone(), &|_| panic!(), later, states, remote);
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), later, states, remote);
    finished(&wake);
    let found = scanner.scan_with(roots.clone(), &|_| panic!(), later, states, remote);
    assert!(
        found
            .distros
            .iter()
            .any(|d| d.host.label == "Healthy" && !d.sites.is_empty())
    );
    for _ in 0..100 {
        scanner.scan_with(roots.clone(), &|_| panic!(), later, states, remote);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    finished(&wake);
    let stale = scanner.scan_with(roots.clone(), &|_| panic!(), later, states, remote);
    assert!(
        stale
            .distros
            .iter()
            .any(|d| d.host.label == "Blocked" && d.sites.is_empty())
    );
    assert!(
        stale
            .distros
            .iter()
            .any(|d| d.host.label == "Healthy" && d.sites.len() == 1)
    );
    let recovered_at = later + REMOTE_LIMIT;
    scanner.scan_with(roots.clone(), &|_| panic!(), recovered_at, states, remote);
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), recovered_at, states, remote);
    finished(&wake);
    finished(&wake);
    let found = scanner.scan_with(roots, &|_| panic!(), recovered_at, states, remote);
    assert!(found.distros.iter().all(|d| d.sites.len() == 1));
    assert!(!scanner.is_pending());
}

#[test]
fn late_and_superseded_distro_results_are_discarded_before_recovery() {
    for expired in [false, true] {
        let (mut scanner, wake) = scanner();
        let now = Instant::now();
        let state = || {
            vec![(
                wsl::LxssEntry {
                    name: "Healthy".into(),
                    default_uid: None,
                },
                Some(true),
            )]
        };
        let original = vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\old"))];
        scanner.scan_with(original.clone(), &|_| panic!(), now, state, remote);
        finished(&wake);
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        scanner.scan_with(
            original.clone(),
            &|_| panic!(),
            now,
            state,
            move |entry, roots| {
                wait.lock().unwrap().recv().unwrap();
                remote(entry, roots)
            },
        );
        let current = if expired {
            original.clone()
        } else {
            vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\new"))]
        };
        let later = if expired {
            now + REMOTE_LIMIT + Duration::from_secs(1)
        } else {
            now
        };
        release.send(()).unwrap();
        finished(&wake);
        let found = scanner.scan_with(current.clone(), &|_| panic!(), later, state, remote);
        assert!(found.distros.iter().all(|d| d.sites.is_empty()));
        let mut recovered = false;
        for _ in 0..10 {
            finished(&wake);
            let found = scanner.scan_with(current.clone(), &|_| panic!(), later, state, remote);
            if found
                .distros
                .iter()
                .any(|d| d.sites.iter().any(|s| s.root == current[0].1))
            {
                recovered = true;
                break;
            }
        }
        assert!(recovered);
    }
}

#[test]
fn roots_changed_back_still_discard_the_original_pending_result() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let state = || {
        vec![(
            wsl::LxssEntry {
                name: "Healthy".into(),
                default_uid: None,
            },
            Some(true),
        )]
    };
    let roots = vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\original"))];
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    finished(&wake);
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    scanner.scan_with(
        roots.clone(),
        &|_| panic!(),
        now,
        state,
        move |entry, roots| {
            wait.lock().unwrap().recv().unwrap();
            remote(entry, roots)
        },
    );
    scanner.scan_with(
        vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\changed"))],
        &|_| panic!(),
        now,
        state,
        remote,
    );
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    release.send(()).unwrap();
    finished(&wake);
    let discarded = scanner.scan_with(roots, &|_| panic!(), now, state, remote);
    assert!(discarded.distros.iter().all(|d| d.sites.is_empty()));
    assert!(!scanner.is_pending());
}

#[test]
fn blocked_status_query_does_not_accumulate_workers_or_probe_unc() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let (release, wait) = mpsc::channel();
    let roots = vec![(
        Vendor::Claude,
        PathBuf::from(r"\\.\UNC\wsl$\Blocked\projects"),
    )];
    scanner.scan_with(
        roots.clone(),
        &|_| panic!(),
        now,
        move || {
            wait.recv().unwrap();
            states()
        },
        |_, _| panic!(),
    );
    for _ in 0..100 {
        let found = scanner.scan_with(
            roots.clone(),
            &|_| panic!(),
            now + REMOTE_LIMIT * 2,
            || panic!(),
            |_, _| panic!(),
        );
        assert!(
            found
                .candidates
                .iter()
                .all(|c| c.result == CandidateResult::Unknown)
        );
    }
    release.send(()).unwrap();
    finished(&wake);
}

#[test]
fn offline_or_unknown_state_does_not_start_unc_work() {
    for running in [Some(false), None] {
        let (mut scanner, wake) = scanner();
        let now = Instant::now();
        let state = move || {
            vec![(
                wsl::LxssEntry {
                    name: "Blocked".into(),
                    default_uid: None,
                },
                running,
            )]
        };
        let roots = vec![(
            Vendor::Claude,
            PathBuf::from(r"\\.\UNC\wsl$\Blocked\projects"),
        )];
        scanner.scan_with(roots.clone(), &|_| panic!(), now, state, |_, _| panic!());
        finished(&wake);
        let found = scanner.scan_with(roots, &|_| panic!(), now, state, |_, _| panic!());
        assert_eq!(found.distros[0].running, running);
        assert!(found.distros[0].sites.is_empty());
    }
}

#[test]
fn incomplete_distro_scan_keeps_removal_pending_and_recovers_after_panic() {
    use crate::runtime::Runtime;
    use crate::tail::scheduler::Heartbeat;

    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let state = || {
        vec![(
            wsl::LxssEntry {
                name: "Healthy".into(),
                default_uid: None,
            },
            Some(true),
        )]
    };
    let path = ManualPath {
        vendor: ManualVendor::Claude,
        root: r"\\wsl$\Healthy\manual".into(),
    };
    let roots = input_roots(&|_| None, std::slice::from_ref(&path));
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    finished(&wake);
    let found = scanner.scan_with(roots, &|_| panic!(), now, state, remote);
    let host = found.distros[0].host.clone();
    let runtime = Runtime::default();
    let hb = Arc::new(Heartbeat::default());
    runtime.register(&host, hb.clone());
    runtime.manual_changed(std::slice::from_ref(&path), &[]);
    let incomplete = scanner.scan_with(Vec::new(), &|_| panic!(), now, state, |_, _| {
        panic!("합성 탐색 실패")
    });
    assert_eq!(incomplete.distros[0].running, None);
    runtime.discovered(&incomplete, &[]);
    runtime.publish_probe(&host, &hb, Vec::new());
    assert!(
        hb.probed
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .removed
            .is_empty()
    );
    finished(&wake);
    scanner.scan_with(Vec::new(), &|_| panic!(), now, state, |_, _| {
        panic!("합성 탐색 실패")
    });
    finished(&wake);
    let failed = scanner.scan_with(Vec::new(), &|_| panic!(), now, state, remote);
    assert_eq!(failed.distros[0].running, None);
    assert!(!scanner.is_pending());
    let later = now + REMOTE_LIMIT;
    scanner.scan_with(Vec::new(), &|_| panic!(), later, state, remote);
    finished(&wake);
    scanner.scan_with(Vec::new(), &|_| panic!(), later, state, remote);
    finished(&wake);
    let found = scanner.scan_with(Vec::new(), &|_| panic!(), later, state, remote);
    assert_eq!(found.distros[0].running, Some(true));
    runtime.discovered(&found, &[]);
    runtime.publish_probe(&host, &hb, Vec::new());
    assert_eq!(hb.probed.lock().unwrap().as_ref().unwrap().removed.len(), 1);
}

#[test]
fn completed_round_is_idle_until_next_probe_period() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let state = || {
        vec![(
            wsl::LxssEntry {
                name: "Healthy".into(),
                default_uid: None,
            },
            Some(true),
        )]
    };
    let roots = vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\projects"))];
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    scanner.request_refresh();
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    finished(&wake);
    let found = scanner.scan_with(roots.clone(), &|_| panic!(), now, state, remote);
    assert_eq!(found.distros[0].sites.len(), 1);
    assert!(!scanner.is_pending());
    for _ in 0..100 {
        let cached = scanner.scan_with(
            roots.clone(),
            &|_| panic!(),
            now + Duration::from_secs(1),
            || panic!("아직 다음 Probe 전이다"),
            |_, _| panic!("완료 알림은 새 UNC 작업을 만들지 않는다"),
        );
        assert_eq!(cached.distros[0].sites.len(), 1);
        assert!(!scanner.is_pending());
    }
    let expired = scanner.scan_with(
        roots,
        &|_| panic!(),
        now + REMOTE_LIMIT + Duration::from_secs(1),
        state,
        remote,
    );
    assert!(expired.distros[0].sites.is_empty());
    assert_eq!(expired.distros[0].running, None);
    assert!(
        expired
            .candidates
            .iter()
            .any(|c| c.result == CandidateResult::Found && c.root.is_some())
    );
    assert!(scanner.is_pending());
    finished(&wake);
}

#[test]
fn explicit_refresh_starts_before_ttl_and_invalidates_changed_registry_entry() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let state = |uid| {
        move || {
            vec![(
                wsl::LxssEntry {
                    name: "Healthy".into(),
                    default_uid: Some(uid),
                },
                Some(true),
            )]
        }
    };
    let roots = vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Healthy\projects"))];
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state(1), remote);
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state(1), remote);
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state(1), remote);
    assert!(!scanner.is_pending());
    scanner.request_refresh();
    let later = now + Duration::from_secs(1);
    scanner.scan_with(roots.clone(), &|_| panic!(), later, state(2), remote);
    assert!(scanner.is_pending());
    finished(&wake);
    let invalidated = scanner.scan_with(
        roots.clone(),
        &|_| panic!(),
        later,
        state(2),
        |entry, roots| {
            assert_eq!(entry.default_uid, Some(2));
            remote(entry, roots)
        },
    );
    assert!(invalidated.distros[0].sites.is_empty());
    finished(&wake);
    let found = scanner.scan_with(roots, &|_| panic!(), later, state(2), remote);
    assert_eq!(found.distros[0].sites.len(), 1);
    assert!(!scanner.is_pending());
}

#[test]
fn explicit_unknown_state_disables_access_even_with_a_fresh_found_cache() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let state = |running| {
        move || {
            vec![(
                wsl::LxssEntry {
                    name: "Healthy".into(),
                    default_uid: Some(1000),
                },
                running,
            )]
        }
    };
    let root = PathBuf::from(r"\\wsl$\Healthy\projects");
    let roots = vec![(Vendor::Claude, root.clone())];
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state(Some(true)), remote);
    finished(&wake);
    scanner.scan_with(roots.clone(), &|_| panic!(), now, state(Some(true)), remote);
    finished(&wake);
    let found = scanner.scan_with(roots.clone(), &|_| panic!(), now, state(Some(true)), remote);
    assert_eq!(found.distros[0].running, Some(true));
    assert_eq!(found.distros[0].sites.len(), 1);
    assert!(!scanner.is_pending());

    let remote_calls = Arc::new(AtomicUsize::new(0));
    let count = remote_calls.clone();
    let unexpected_remote = move |entry, roots| {
        count.fetch_add(1, Ordering::SeqCst);
        remote(entry, roots)
    };
    scanner.request_refresh();
    let later = now + Duration::from_secs(1);
    scanner.scan_with(
        roots.clone(),
        &|_| panic!(),
        later,
        state(None),
        unexpected_remote.clone(),
    );
    finished(&wake);
    let unknown = scanner.scan_with(roots, &|_| panic!(), later, state(None), unexpected_remote);
    assert_eq!(unknown.distros.len(), 1);
    assert_eq!(unknown.distros[0].running, None);
    assert!(unknown.distros[0].sites.is_empty());
    assert!(
        unknown
            .candidates
            .iter()
            .any(|candidate| candidate.result == CandidateResult::Found
                && candidate.root.as_ref() == Some(&root))
    );
    assert_eq!(remote_calls.load(Ordering::SeqCst), 0);
    assert!(!scanner.is_pending());
}

#[test]
fn startup_collects_windows_and_publishes_while_remote_discovery_is_blocked() {
    use crate::diag::Diag;
    use crate::office::{
        names::NameBook,
        world::{OfficeWorld, Scale},
    };
    use crate::startup::{self, Boot, Limits, ScanWatch, Startup, StartupSummary, StepState};
    use crate::tail::scheduler::{HostLoop, LogicalClock};
    use chrono::{FixedOffset, Utc};

    struct LocalBoot {
        scanner: Mutex<Scanner>,
        roots: Roots,
        blocked: Mutex<Option<mpsc::Receiver<()>>>,
        world: Arc<Mutex<OfficeWorld>>,
        diag: Arc<Mutex<Diag>>,
        published: AtomicBool,
    }
    impl Boot for LocalBoot {
        fn wait_for_scale(&self) {}
        fn discover(&self) -> (Vec<Site>, Vec<wsl::WslDistro>) {
            let wait = self.blocked.lock().unwrap().take().unwrap();
            let found = self.scanner.lock().unwrap().scan_with(
                self.roots.clone(),
                &Path::is_dir,
                Instant::now(),
                move || {
                    wait.recv().unwrap();
                    states()
                },
                remote,
            );
            (found.sites, found.distros)
        }
        fn discovery_pending(&self) -> bool {
            self.scanner.lock().unwrap().is_pending()
        }
        fn spawn_hosts(&self, sites: Vec<Site>, _: Vec<wsl::WslDistro>, watch: Arc<ScanWatch>) {
            let mut host = HostLoop::new(
                windows_host(),
                sites,
                self.world.clone(),
                Arc::new(LogicalClock::new(Utc::now(), 0.0)),
                self.diag.clone(),
            );
            host.watch = Some(watch);
            host.tick();
        }
        fn sidebar(&self) {}
        fn publish(&self) {
            self.published.store(true, Ordering::SeqCst);
        }
    }
    for local in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(projects.join("synthetic")).unwrap();
        let line = serde_json::json!({"type":"user","message":{"role":"user","content":"synthetic"},"uuid":"u-one","timestamp":Utc::now().to_rfc3339(),"entrypoint":"claude-desktop","cwd":"C:\\synthetic","sessionId":"one"});
        std::fs::write(projects.join("synthetic/one.jsonl"), format!("{line}\n")).unwrap();
        let world = Arc::new(Mutex::new(OfficeWorld::new(
            Scale::Large,
            NameBook::load(tmp.path()),
            FixedOffset::east_opt(0).unwrap(),
        )));
        let (scanner, wake) = scanner();
        let (release, wait) = mpsc::channel();
        let boot = LocalBoot {
            scanner: Mutex::new(scanner),
            roots: if local {
                vec![(Vendor::Claude, projects)]
            } else {
                Vec::new()
            },
            blocked: Mutex::new(Some(wait)),
            world: world.clone(),
            diag: Arc::new(Mutex::new(Diag::open(tmp.path(), false))),
            published: AtomicBool::new(false),
        };
        let startup = Startup::new(|_| {});
        startup::run_guarded(
            &startup,
            &boot,
            &world,
            &Limits {
                step: Duration::from_secs(1),
                tick: Duration::from_millis(10),
            },
        );
        assert!(boot.published.load(Ordering::SeqCst));
        assert_eq!(world.lock().unwrap().teams.len(), usize::from(local));
        assert!(
            !startup
                .progress()
                .iter()
                .any(|p| p.summary == StartupSummary::NoFolders)
        );
        if !local {
            assert!(startup.progress().iter().any(|p| p.step == 3
                && p.state == StepState::Done
                && p.summary == StartupSummary::WslPending));
        }
        assert!(
            startup
                .progress()
                .iter()
                .any(|p| p.step == 7 && p.state == StepState::Done)
        );
        assert!(wake.try_recv().is_err(), "WSL은 아직 끝나지 않았다");
        release.send(()).unwrap();
        finished(&wake);
    }
}
