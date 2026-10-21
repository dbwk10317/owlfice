pub mod detail;
pub mod diag;
pub mod discover;
pub mod ipc;
pub mod notification;
pub mod office;
pub mod oplog;
pub mod parse;
pub mod runtime;
pub mod settings;
pub mod startup;
pub mod tail;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::{Local, Offset, Utc};
use tauri::{Emitter, Manager};

use diag::Diag;
use discover::wsl::WslDistro;
use discover::{ScanResult, Scanner, Site, windows_host};
use ipc::Ipc;
use office::names::{NameBook, RoomBook};
use office::world::{OfficeWorld, Scale};
use runtime::Runtime;
use settings::SettingsBook;
use startup::{Boot, Limits, ScanWatch, Startup, StartupSummary, StepState};
use tail::scheduler::{self, HostLoop, Prober, WallClock};

/// `%APPDATA%\Owlfice`. 이름·방 번호·미대응 로그를 둔다 (docs/CONTRACTS.md C8·C11)
fn data_dir() -> Result<PathBuf, String> {
    let appdata =
        std::env::var_os("APPDATA").ok_or("APPDATA 가 없어 저장 폴더를 정할 수 없습니다")?;
    let dir = PathBuf::from(appdata).join("Owlfice");
    std::fs::create_dir_all(dir.join("logs"))
        .map_err(|e| format!("{} 를 만들지 못했습니다: {e}", dir.display()))?;
    Ok(dir)
}

/// 탐색 결과. 호스트·제품·루트 경로만 남긴다
fn log_sites(sites: &[Site], distros: &[WslDistro]) {
    if sites.is_empty() && distros.is_empty() {
        crate::oplog!("탐색: 찾은 세션 폴더 없음");
    }
    for d in distros {
        let running = match d.running {
            Some(true) => "켜짐",
            Some(false) => "꺼짐",
            None => "켜짐 여부 모름",
        };
        crate::oplog!("탐색: WSL {} {running}", d.host.id);
    }
    for s in sites.iter().chain(distros.iter().flat_map(|d| &d.sites)) {
        crate::oplog!(
            "탐색: {} {} {}",
            s.host.id,
            s.vendor.label(),
            s.root.display()
        );
    }
}

/// 기동이 바깥 세계에 하는 일의 실제 구현 (startup::Boot)
struct AppBoot {
    app: tauri::AppHandle,
    ipc: Arc<Ipc>,
    world: Arc<Mutex<OfficeWorld>>,
    diag: Arc<Mutex<Diag>>,
    settings: Arc<SettingsBook>,
    runtime: Arc<Runtime>,
    scanner: Arc<Mutex<Scanner>>,
}

fn discover_current(
    scanner: &Mutex<Scanner>,
    settings: &SettingsBook,
    runtime: &Runtime,
) -> ScanResult {
    loop {
        let manual = settings.snapshot().manual_paths.unwrap_or_default();
        let found = scanner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .scan(&manual);
        if settings.snapshot().manual_paths.unwrap_or_default() != manual {
            continue;
        }
        runtime.discovered(&found, &manual);
        return found;
    }
}

impl Boot for AppBoot {
    fn wait_for_scale(&self) {
        self.settings.wait_scale();
    }

    fn discover(&self) -> (Vec<Site>, Vec<WslDistro>) {
        let found = discover_current(&self.scanner, &self.settings, &self.runtime);
        log_sites(&found.sites, &found.distros);
        (found.sites, found.distros)
    }

    fn discovery_pending(&self) -> bool {
        self.scanner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_pending()
    }

    /// 호스트 스레드를 띄운다. WSL 스레드는 Probe 가 켜진 배포판을 보고 띄운다 (docs/CONTRACTS.md C5)
    fn spawn_hosts(&self, sites: Vec<Site>, distros: Vec<WslDistro>, watch: Arc<ScanWatch>) {
        let mut host = HostLoop::new(
            windows_host(),
            sites.clone(),
            self.world.clone(),
            Arc::new(WallClock),
            self.diag.clone(),
        );
        host.watch = Some(watch);
        host.runtime = self.runtime.clone();
        // Windows로 시작하고 WSL 탐색 완료는 Probe를 깨워 별도로 반영한다.
        let mut first = Some((sites, distros));
        let settings = self.settings.clone();
        let runtime = self.runtime.clone();
        let scanner = self.scanner.clone();
        let prober = Prober::new(
            &host,
            Box::new(move || {
                first.take().unwrap_or_else(|| {
                    let found = discover_current(&scanner, &settings, &runtime);
                    (found.sites, found.distros)
                })
            }),
        );
        scheduler::spawn(host);
        scheduler::spawn_probe(prober);
    }

    fn sidebar(&self) {
        self.ipc.snapshot(Utc::now());
    }

    fn publish(&self) {
        ipc::spawn_publisher(self.app.clone(), self.ipc.clone());
    }
}

/// 1단계(설정·이름·방 번호 저장소 읽기)를 하고 나머지 단계를 스레드로 띄운다.
/// 프론트가 처음 부르는 get_settings·get_world_snapshot 이 바로 답하도록 상태는 여기서 만든다
fn start(app: &tauri::AppHandle) -> Result<(), String> {
    let emitter = app.clone();
    let startup = Arc::new(Startup::new(move |p| {
        if let Err(e) = emitter.emit(startup::EVENT, p) {
            crate::oplog!("startup-progress 발행 실패: {e}");
        }
    }));
    startup.report(1, StepState::Running, StartupSummary::None, None);
    let dir = data_dir()?;
    oplog::init(&dir.join("logs"));
    let local = Local::now().offset().fix();
    let settings = Arc::new(SettingsBook::load(&dir));
    if settings.corrupt {
        startup.report(1, StepState::Warn, StartupSummary::SettingsBroken, None);
    }
    // 규모를 아직 안 골랐으면 고를 때까지 대기업. 고르면 set_settings 가 재배치한다
    let world = Arc::new(Mutex::new(OfficeWorld::new(
        settings.scale().unwrap_or(Scale::Large),
        NameBook::load(&dir),
        local,
    )));
    let diag = Arc::new(Mutex::new(Diag::open(
        &dir.join("logs"),
        settings.snapshot().raw_diagnostics.unwrap_or(false),
    )));
    let runtime = Arc::new(Runtime::new(
        dir.join("logs"),
        std::env::var_os("USERPROFILE").map(PathBuf::from),
    ));
    let wake = runtime.clone();
    let scanner = Arc::new(Mutex::new(Scanner::new(move || wake.request_rescan())));
    let ipc = Arc::new(Ipc::new(world.clone(), RoomBook::load(&dir)));
    startup.report(1, StepState::Done, StartupSummary::None, None);

    let boot = AppBoot {
        app: app.clone(),
        ipc: ipc.clone(),
        world: world.clone(),
        diag: diag.clone(),
        settings: settings.clone(),
        runtime: runtime.clone(),
        scanner,
    };
    let progress = startup.clone();
    app.manage(ipc);
    app.manage(settings);
    app.manage(diag);
    app.manage(runtime);
    app.manage(notification::WindowVisibility::default());
    app.manage(startup);
    thread::spawn(move || startup::run_guarded(&progress, &boot, &world, &Limits::default()));
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            start(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::get_world_snapshot,
            ipc::select_office,
            ipc::get_settings,
            ipc::set_settings,
            ipc::rescan,
            ipc::get_discovery,
            ipc::get_diagnostics,
            ipc::open_log_folder,
            notification::set_window_hidden,
            ipc::get_employee_timeline,
            ipc::get_employee_detail,
            ipc::get_session_detail,
            ipc::open_path,
            startup::get_startup_progress
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
