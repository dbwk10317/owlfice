pub mod discover;
pub mod office;
pub mod parse;
pub mod tail;

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
