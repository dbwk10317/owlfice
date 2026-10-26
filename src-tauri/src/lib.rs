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

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
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

// 설정·이름 파일을 읽기 전에 잠가야 다른 프로세스의 옛 값이 저장을 덮지 않는다.
#[derive(Default)]
struct Initialization(Mutex<Option<File>>);

fn lock_data_dir(dir: &Path) -> Result<File, String> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("instance.lock"))
        .map_err(|e| format!("저장 폴더의 실행 잠금을 열지 못했습니다: {e}"))?;
    file.try_lock().map_err(|e| match e {
        TryLockError::WouldBlock => {
            "같은 저장 폴더를 사용하는 야근불이 이미 실행 중입니다. 기존 창을 사용하거나 기존 앱을 종료한 뒤 다시 시도해 주세요.".into()
        }
        TryLockError::Error(e) => format!("저장 폴더의 실행 잠금을 얻지 못했습니다: {e}"),
    })?;
    Ok(file)
}

#[tauri::command]
async fn initialize_app(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Initialization>();
        let mut guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }
        let startup = app.state::<Arc<Startup>>();
        match start(&app, &startup) {
            Ok(file) => {
                *guard = Some(file);
                Ok(())
            }
            Err(e) => {
                startup.report(1, StepState::Error, StartupSummary::None, None);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| format!("앱 준비 작업을 완료하지 못했습니다: {e}"))?
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
        let found = {
            let mut scanner = scanner.lock().unwrap_or_else(|e| e.into_inner());
            if runtime
                .refresh_requested
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                scanner.request_refresh();
            }
            scanner.scan(&manual)
        };
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
fn start(app: &tauri::AppHandle, startup: &Arc<Startup>) -> Result<File, String> {
    startup.report(1, StepState::Running, StartupSummary::None, None);
    let dir = data_dir()?;
    let guard = lock_data_dir(&dir)?;
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
    let scanner = Arc::new(Mutex::new(Scanner::new(move || wake.wake_discovery())));
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
    thread::spawn(move || startup::run_guarded(&progress, &boot, &world, &Limits::default()));
    Ok(guard)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let emitter = app.handle().clone();
            app.manage(Arc::new(Startup::new(move |p| {
                if let Err(e) = emitter.emit(startup::EVENT, p) {
                    crate::oplog!("startup-progress 발행 실패: {e}");
                }
            })));
            app.manage(Initialization::default());
            app.manage(notification::WindowVisibility::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            initialize_app,
            ipc::get_world_snapshot,
            ipc::select_office,
            ipc::get_settings,
            ipc::set_settings,
            ipc::rescan,
            ipc::get_discovery,
            ipc::get_diagnostics,
            ipc::get_log_folder,
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

#[cfg(test)]
mod instance_tests {
    use super::*;

    #[test]
    fn data_directory_has_one_writer_and_unlocks_after_exit() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let guard = lock_data_dir(first.path()).unwrap();
        assert!(
            lock_data_dir(first.path())
                .unwrap_err()
                .contains("이미 실행 중")
        );
        let separate = lock_data_dir(second.path()).unwrap();
        drop(guard);
        assert!(lock_data_dir(first.path()).is_ok());
        drop(separate);
    }
}
