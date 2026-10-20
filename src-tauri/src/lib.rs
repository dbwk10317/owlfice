pub mod diag;
pub mod discover;
pub mod ipc;
pub mod office;
pub mod parse;
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
use discover::{Site, discover_all, discover_wsl, windows_host};
use ipc::Ipc;
use office::names::{NameBook, RoomBook};
use office::world::{OfficeWorld, Scale};
use settings::SettingsBook;
use startup::{Boot, Limits, ScanWatch, Startup, StartupSummary, StepState};
use tail::scheduler::{self, HostLoop, Prober, WallClock};

/// `%APPDATA%\Owlfice`. 이름·방 번호·미대응 로그를 둔다 (4.7.7, 4.7.8, 4.11)
fn data_dir() -> Result<PathBuf, String> {
    let appdata =
        std::env::var_os("APPDATA").ok_or("APPDATA 가 없어 저장 폴더를 정할 수 없습니다")?;
    let dir = PathBuf::from(appdata).join("Owlfice");
    std::fs::create_dir_all(dir.join("logs"))
        .map_err(|e| format!("{} 를 만들지 못했습니다: {e}", dir.display()))?;
    Ok(dir)
}

/// 기동이 바깥 세계에 하는 일의 실제 구현 (startup::Boot)
struct AppBoot {
    app: tauri::AppHandle,
    ipc: Arc<Ipc>,
    world: Arc<Mutex<OfficeWorld>>,
    diag: Arc<Mutex<Diag>>,
}

impl Boot for AppBoot {
    fn discover(&self) -> (Vec<Site>, Vec<WslDistro>) {
        (discover_all(), discover_wsl())
    }

    /// 호스트 스레드를 띄운다. WSL 스레드는 Probe 가 켜진 배포판을 보고 띄운다 (4.4.2)
    fn spawn_hosts(&self, sites: Vec<Site>, distros: Vec<WslDistro>, watch: Arc<ScanWatch>) {
        let mut host = HostLoop::new(
            windows_host(),
            sites.clone(),
            self.world.clone(),
            Arc::new(WallClock),
            self.diag.clone(),
        );
        host.watch = Some(watch);
        // 첫 Probe 는 방금 찾은 결과를 쓴다. wsl.exe 를 또 부르면 느린 PC 에서 로딩이 두 배가 된다
        let mut first = Some((sites, distros));
        let prober = Prober::new(
            &host,
            Box::new(move || {
                first
                    .take()
                    .unwrap_or_else(|| (discover_all(), discover_wsl()))
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
            eprintln!("startup-progress 발행 실패: {e}");
        }
    }));
    startup.report(1, StepState::Running, StartupSummary::None, None);
    let dir = data_dir()?;
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
    // ponytail: 원문 진단은 설정(T30) 전까지 끈다
    let diag = Arc::new(Mutex::new(Diag::open(&dir.join("logs"), false)));
    let ipc = Arc::new(Ipc::new(world.clone(), RoomBook::load(&dir)));
    startup.report(1, StepState::Done, StartupSummary::None, None);

    let boot = AppBoot {
        app: app.clone(),
        ipc: ipc.clone(),
        world: world.clone(),
        diag,
    };
    let progress = startup.clone();
    thread::spawn(move || startup::run(&progress, &boot, &world, &Limits::default()));
    app.manage(ipc);
    app.manage(settings);
    app.manage(startup);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            start(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::get_world_snapshot,
            ipc::select_office,
            ipc::get_settings,
            ipc::set_settings,
            startup::get_startup_progress
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
