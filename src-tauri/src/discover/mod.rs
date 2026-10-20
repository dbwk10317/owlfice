//! 세션 위치 탐색 (PLAN 4.4)

use std::path::PathBuf;

mod registry;
mod windows;

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
