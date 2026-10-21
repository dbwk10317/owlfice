use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::runtime::Runtime;
use crate::tail::scheduler::Heartbeat;

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

fn remote(roots: Roots) -> ScanResult {
    scan_roots(
        vec![wsl::WslDistro {
            host: Host {
                id: "wsl:Synthetic".into(),
                kind: HostKind::Wsl {
                    distro: "Synthetic".into(),
                },
                label: "Synthetic".into(),
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
fn blocked_remote_does_not_delay_windows_rescans_or_accumulate_workers() {
    let (mut scanner, wake) = scanner();
    let (release, wait) = mpsc::channel();
    let (entered, started) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let first = calls.clone();
    let now = Instant::now();
    let roots = vec![
        (Vendor::Claude, PathBuf::from("C:/synthetic/projects")),
        (
            Vendor::Codex,
            PathBuf::from(r"\\?\UNC\WSL$\Synthetic\sessions"),
        ),
        (
            Vendor::Claude,
            PathBuf::from(r"\\wsl.localhost\Synthetic\manual"),
        ),
    ];
    let local_only = |root: &Path| {
        assert!(wsl_path(root).is_none());
        true
    };
    let found = scanner.scan_with(roots.clone(), &local_only, now, move |roots| {
        first.fetch_add(1, Ordering::SeqCst);
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|(_, root)| wsl_path(root).is_some()));
        entered.send(()).unwrap();
        wait.recv().unwrap();
        remote(roots)
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(found.sites.len(), 1);
    assert!(
        found
            .candidates
            .iter()
            .filter(|c| matches!(c.host.kind, HostKind::Wsl { .. }))
            .all(|c| c.result == CandidateResult::Unknown)
    );
    let mut changed = roots;
    changed[0].1 = PathBuf::from("D:/synthetic/new-projects");
    for _ in 0..100 {
        let extra = calls.clone();
        let found = scanner.scan_with(
            changed.clone(),
            &local_only,
            now + REMOTE_LIMIT * 2,
            move |roots| {
                extra.fetch_add(1, Ordering::SeqCst);
                remote(roots)
            },
        );
        assert_eq!(found.sites[0].root, changed[0].1);
        assert!(found.distros.is_empty());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    finished(&wake);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn late_and_superseded_remote_results_are_discarded_before_recovery() {
    for expired in [false, true] {
        let (mut scanner, wake) = scanner();
        let (release, wait) = mpsc::channel();
        let now = Instant::now();
        let original = vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Synthetic\old"))];
        scanner.scan_with(original.clone(), &|_| panic!(), now, move |roots| {
            wait.recv().unwrap();
            remote(roots)
        });
        let current = if expired {
            original
        } else {
            vec![(Vendor::Claude, PathBuf::from(r"\\wsl$\Synthetic\new"))]
        };
        let later = if expired {
            now + REMOTE_LIMIT + Duration::from_secs(1)
        } else {
            now
        };
        release.send(()).unwrap();
        finished(&wake);
        let rejected = scanner.scan_with(current.clone(), &|_| panic!(), later, remote);
        assert!(rejected.distros.is_empty());
        assert!(
            rejected
                .candidates
                .iter()
                .all(|c| c.result == CandidateResult::Unknown)
        );
        finished(&wake);
        let recovered = scanner.scan_with(current.clone(), &|_| panic!(), later, |_| {
            panic!("완료된 결과를 먼저 쓴다")
        });
        assert_eq!(recovered.distros[0].running, Some(true));
        assert_eq!(recovered.distros[0].sites[0].root, current[0].1);
    }
}

#[test]
fn incomplete_remote_scan_keeps_removal_pending_and_recovers_after_panic() {
    let (mut scanner, wake) = scanner();
    let now = Instant::now();
    let path = ManualPath {
        vendor: ManualVendor::Claude,
        root: r"\\wsl$\Synthetic\manual".into(),
    };
    let roots = input_roots(&|_| None, std::slice::from_ref(&path));
    scanner.scan_with(roots.clone(), &|_| panic!(), now, remote);
    finished(&wake);
    let found = scanner.scan_with(roots, &|_| panic!(), now, |_| panic!());
    let host = found.distros[0].host.clone();
    let runtime = Runtime::default();
    let hb = Arc::new(Heartbeat::default());
    runtime.register(&host, hb.clone());
    runtime.manual_changed(std::slice::from_ref(&path), &[]);
    let incomplete =
        scanner.scan_with(Vec::new(), &|_| panic!(), now, |_| panic!("합성 탐색 실패"));
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
    let after_panic = scanner.scan_with(Vec::new(), &|_| panic!(), now, remote);
    assert_eq!(after_panic.distros[0].running, None);
    finished(&wake);
    let recovered = scanner.scan_with(Vec::new(), &|_| panic!(), now, |_| panic!());
    assert_eq!(recovered.distros[0].running, Some(true));
    runtime.discovered(&recovered, &[]);
    runtime.publish_probe(&host, &hb, Vec::new());
    assert_eq!(hb.probed.lock().unwrap().as_ref().unwrap().removed.len(), 1);
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
                move |roots| {
                    wait.recv().unwrap();
                    remote(roots)
                },
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
