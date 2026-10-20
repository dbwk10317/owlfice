pub mod diag;
pub mod discover;
pub mod ipc;
pub mod office;
pub mod parse;
pub mod settings;
pub mod tail;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{Local, Offset};
use tauri::Manager;

use diag::Diag;
use discover::{discover_all, discover_wsl, windows_host};
use ipc::Ipc;
use office::names::{NameBook, RoomBook};
use office::world::{OfficeWorld, Scale};
use settings::SettingsBook;
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

/// 호스트 스레드를 띄우고 world 를 돌려준다. WSL 스레드는 Probe 가 켜진 배포판을 보고 띄운다 (4.4.2)
fn start() -> Result<(Arc<Ipc>, Arc<SettingsBook>), String> {
    let dir = data_dir()?;
    let local = Local::now().offset().fix();
    let settings = Arc::new(SettingsBook::load(&dir));
    if settings.corrupt {
        // ponytail: 로딩 화면(T16)이 3.9 의 "창립 서류가 찢어졌습니다" 줄을 낼 때까지 stderr 한 줄
        eprintln!("settings.json 이 손상되어 기본값으로 시작합니다");
    }
    // 규모를 아직 안 골랐으면 고를 때까지 대기업. 고르면 set_settings 가 재배치한다
    let world = Arc::new(Mutex::new(OfficeWorld::new(
        settings.scale().unwrap_or(Scale::Large),
        NameBook::load(&dir),
        local,
    )));
    // ponytail: 원문 진단은 설정(T30) 전까지 끈다
    let diag = Arc::new(Mutex::new(Diag::open(&dir.join("logs"), false)));
    let host = HostLoop::new(
        windows_host(),
        discover_all(),
        world.clone(),
        Arc::new(WallClock),
        diag,
    );
    let prober = Prober::new(&host, Box::new(|| (discover_all(), discover_wsl())));
    scheduler::spawn(host);
    scheduler::spawn_probe(prober);
    Ok((Arc::new(Ipc::new(world, RoomBook::load(&dir))), settings))
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let (ipc, settings) = start()?;
            app.manage(ipc.clone());
            app.manage(settings);
            ipc::spawn_publisher(app.handle().clone(), ipc);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::get_world_snapshot,
            ipc::select_office,
            ipc::get_settings,
            ipc::set_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
