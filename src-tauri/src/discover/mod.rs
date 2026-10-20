//! 세션 위치 탐색 (PLAN 4.4)

use std::path::PathBuf;

pub mod registry;
mod windows;
pub mod wsl;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: String,
    pub kind: HostKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    Windows,
    Wsl { distro: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Claude,
    Codex,
    Gemini,
}

/// 세션 루트 하나. 동(Product)은 세션마다 파서가 정하므로 여기에 없다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub host: Host,
    pub vendor: Vendor,
    /// UNC 또는 로컬
    pub root: PathBuf,
    pub read_mode: ReadMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    Append,
    /// 파일 전체를 다시 읽는다. Antigravity가 append 전용이면 지운다 (PLAN 4.6.3)
    WholeFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    ClaudeDesktop,
    ClaudeCli,
    CodexApp,
    CodexCli,
    AntigravityApp,
    AntigravityCli,
}

/// 실제 환경의 Windows 호스트 Site 목록
pub fn discover_all() -> Vec<Site> {
    windows::find_sites(&|k| std::env::var(k).ok())
}

/// 실제 환경의 Claude 홈에서 살아 있는 세션 등록부 항목
pub fn live_claude_sessions() -> Vec<registry::RegistryEntry> {
    windows::claude_home(&|k| std::env::var(k).ok())
        .map(|h| registry::live_sessions(&h))
        .unwrap_or_default()
}

/// 실제 환경의 WSL 배포판. 꺼진 배포판은 sites 없이 남는다
pub fn discover_wsl() -> Vec<wsl::WslDistro> {
    wsl::discover()
}
