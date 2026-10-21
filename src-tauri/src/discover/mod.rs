//! 세션 위치 탐색 (PLAN 4.4)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::settings::{ManualPath, ManualVendor};

pub mod registry;
mod windows;
pub mod wsl;

pub use windows::windows_host;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Host {
    pub id: String,
    pub kind: HostKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum HostKind {
    Windows,
    Wsl { distro: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub enum Vendor {
    Claude,
    Codex,
    Gemini,
}

impl Vendor {
    /// 화면에 나가는 이름
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
        }
    }
}

/// 세션 루트 하나. 동(Product)은 세션마다 파서가 정하므로 여기에 없다
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Site {
    pub host: Host,
    pub vendor: Vendor,
    /// UNC 또는 로컬
    pub root: PathBuf,
    pub read_mode: ReadMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ReadMode {
    Append,
    /// 파일 전체를 다시 읽는다. Antigravity가 append 전용이면 지운다 (PLAN 4.6.3)
    WholeFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Product {
    ClaudeDesktop,
    ClaudeCli,
    CodexApp,
    CodexCli,
    AntigravityApp,
    AntigravityCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateResult {
    Found,
    Missing,
    Offline,
    Unknown,
}

/// 진단용 메모리 자료. 경로를 외부로 내보낼 때의 비식별 처리는 호출자가 맡는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub host: Host,
    pub vendor: Vendor,
    pub root: Option<PathBuf>,
    pub result: CandidateResult,
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub sites: Vec<Site>,
    pub distros: Vec<wsl::WslDistro>,
    pub candidates: Vec<Candidate>,
}

/// 자동 탐색과 수동 세션 루트를 합친다. 수동 root는 projects/sessions 폴더 자체다.
pub fn scan(manual: &[ManualPath]) -> ScanResult {
    let (distros, candidates) = wsl::discover_with_candidates();
    scan_with(
        &|k| std::env::var(k).ok(),
        distros,
        candidates,
        manual,
        &Path::is_dir,
    )
}

fn scan_with(
    env: &dyn Fn(&str) -> Option<String>,
    distros: Vec<wsl::WslDistro>,
    candidates: Vec<Candidate>,
    manual: &[ManualPath],
    is_dir: &dyn Fn(&Path) -> bool,
) -> ScanResult {
    let mut out = ScanResult {
        sites: Vec::new(),
        distros,
        candidates,
    };
    let mut seen: HashSet<_> = out.candidates.iter().map(candidate_key).collect();
    for d in &out.distros {
        for s in &d.sites {
            let candidate = Candidate {
                host: s.host.clone(),
                vendor: s.vendor,
                root: Some(s.root.clone()),
                result: CandidateResult::Found,
            };
            if seen.insert(candidate_key(&candidate)) {
                out.candidates.push(candidate);
            }
        }
    }
    for (vendor, root) in windows::candidate_roots(env) {
        add_root(&mut out, &mut seen, vendor, root, is_dir);
    }
    for p in manual {
        // 설정 저장 때도 검사하지만 공개 scan에 직접 전달된 잘못된 입력 역시 열지 않는다.
        if p.validate().is_err() {
            continue;
        }
        let vendor = match p.vendor {
            ManualVendor::Claude => Vendor::Claude,
            ManualVendor::Codex => Vendor::Codex,
        };
        add_root(&mut out, &mut seen, vendor, PathBuf::from(&p.root), is_dir);
    }
    out
}

type CandidateKey = (String, Vendor, Option<String>);

pub(crate) fn candidate_key(candidate: &Candidate) -> CandidateKey {
    let wsl = matches!(candidate.host.kind, HostKind::Wsl { .. });
    let root = candidate.root.as_ref().map(|p| {
        let path = if wsl {
            wsl_path(p)
                .map(|(_, tail)| tail)
                .unwrap_or_else(|| p.to_string_lossy().into_owned())
        } else {
            p.to_string_lossy().to_lowercase()
        };
        path.replace('/', "\\").trim_end_matches('\\').to_owned()
    });
    (candidate.host.id.to_lowercase(), candidate.vendor, root)
}

/// 공유 이름과 구분자만 정규화하고 Linux 내부 경로의 대소문자는 그대로 둔다.
fn wsl_path(root: &Path) -> Option<(String, String)> {
    let root = root.to_string_lossy().replace('/', "\\");
    let unc = root.strip_prefix("\\\\")?;
    let unc = if unc
        .get(..6)
        .is_some_and(|s| s.eq_ignore_ascii_case("?\\UNC\\"))
    {
        &unc[6..]
    } else {
        unc
    };
    let (server, rest) = unc.split_once('\\')?;
    if !server.eq_ignore_ascii_case("wsl.localhost") && !server.eq_ignore_ascii_case("wsl$") {
        return None;
    }
    let (distro, tail) = rest.split_once('\\').unwrap_or((rest, ""));
    Some((distro.to_owned(), tail.to_owned()))
}

fn add_root(
    out: &mut ScanResult,
    seen: &mut HashSet<CandidateKey>,
    vendor: Vendor,
    root: PathBuf,
    is_dir: &dyn Fn(&Path) -> bool,
) {
    let (host, result, distro_index) = if let Some((name, _)) = wsl_path(&root) {
        let known = out.distros.iter().position(|d| match &d.host.kind {
            HostKind::Wsl { distro } => distro.eq_ignore_ascii_case(&name),
            HostKind::Windows => false,
        });
        if let Some(i) = known {
            let d = &out.distros[i];
            let result = match d.running {
                Some(true) => CandidateResult::Found,
                Some(false) => CandidateResult::Offline,
                None => CandidateResult::Unknown,
            };
            (d.host.clone(), result, Some(i))
        } else {
            (
                Host {
                    id: format!("wsl:{name}"),
                    kind: HostKind::Wsl {
                        distro: name.clone(),
                    },
                    label: name,
                },
                CandidateResult::Unknown,
                None,
            )
        }
    } else {
        (windows_host(), CandidateResult::Found, None)
    };
    let mut candidate = Candidate {
        host,
        vendor,
        root: Some(root.clone()),
        result,
    };
    if !seen.insert(candidate_key(&candidate)) {
        return;
    }
    if candidate.result == CandidateResult::Found {
        if is_dir(&root) {
            let site = Site {
                host: candidate.host.clone(),
                vendor,
                root,
                read_mode: ReadMode::Append,
            };
            if let Some(i) = distro_index {
                out.distros[i].sites.push(site);
            } else {
                out.sites.push(site);
            }
        } else {
            candidate.result = CandidateResult::Missing;
        }
    }
    out.candidates.push(candidate);
}

/// 실제 환경의 Windows 호스트 Site 목록
pub fn discover_all() -> Vec<Site> {
    windows::find_sites(&|k| std::env::var(k).ok())
}

/// 실제 환경의 Claude 홈에서 살아 있는 세션 등록부 항목
pub fn live_claude_sessions() -> Vec<registry::RegistryEntry> {
    windows::claude_home(&|k| std::env::var(k).ok())
        .and_then(|h| registry::live_sessions(&h, &Default::default()))
        .map(|m| m.into_values().collect())
        .unwrap_or_default()
}

/// 실제 환경의 WSL 배포판. 꺼진 배포판은 sites 없이 남는다
pub fn discover_wsl() -> Vec<wsl::WslDistro> {
    wsl::discover()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    fn manual(vendor: ManualVendor, root: &str) -> ManualPath {
        ManualPath {
            vendor,
            root: root.into(),
        }
    }

    fn distro(name: &str, running: Option<bool>) -> wsl::WslDistro {
        wsl::WslDistro {
            host: Host {
                id: format!("wsl:{name}"),
                kind: HostKind::Wsl {
                    distro: name.into(),
                },
                label: name.into(),
            },
            running,
            sites: Vec::new(),
        }
    }

    #[test]
    fn local_roots_report_missing_and_deduplicate_manual_case_and_slashes() {
        let tmp = tempfile::tempdir().unwrap();
        let auto = tmp.path().join(".claude/projects");
        let extra = tmp.path().join("manual-sessions");
        fs::create_dir_all(&auto).unwrap();
        fs::create_dir_all(&extra).unwrap();
        let home = tmp.path().display().to_string();
        let env = |k: &str| (k == "USERPROFILE").then(|| home.clone());
        let paths = [
            manual(
                ManualVendor::Claude,
                &format!("{}/", auto.display())
                    .replace('\\', "/")
                    .to_uppercase(),
            ),
            manual(ManualVendor::Codex, &extra.display().to_string()),
        ];
        let probes = RefCell::new(Vec::new());
        let out = scan_with(&env, Vec::new(), Vec::new(), &paths, &|p| {
            probes.borrow_mut().push(p.to_owned());
            p.is_dir()
        });
        assert_eq!(out.sites.len(), 2);
        assert_eq!(out.sites[0].root, auto);
        assert_eq!(out.sites[1].root, extra);
        assert_eq!(out.candidates.len(), 3);
        assert_eq!(out.candidates[1].result, CandidateResult::Missing);
        assert_eq!(
            out.candidates[1].root,
            Some(tmp.path().join(".codex/sessions"))
        );
        assert_eq!(probes.borrow().len(), 3);
    }

    #[test]
    fn wsl_aliases_merge_with_existing_sites_but_linux_case_and_vendor_stay_distinct() {
        let mut d = distro("D", Some(true));
        d.sites.push(Site {
            host: d.host.clone(),
            vendor: Vendor::Claude,
            root: PathBuf::from(r"\\wsl.localhost\D\home\me\projects"),
            read_mode: ReadMode::Append,
        });
        let paths = [
            manual(ManualVendor::Claude, "//WSL$/d/home/me/projects/"),
            manual(ManualVendor::Claude, r"\\wsl$\D\Home\me\projects"),
            manual(ManualVendor::Codex, r"\\wsl$\D\home\me\projects"),
        ];
        let probes = RefCell::new(Vec::new());
        let out = scan_with(&|_| None, vec![d], Vec::new(), &paths, &|p| {
            probes.borrow_mut().push(p.to_owned());
            true
        });
        assert!(out.sites.is_empty());
        assert_eq!(out.distros[0].sites.len(), 3);
        assert_eq!(out.candidates.len(), 3);
        assert_eq!(probes.borrow().len(), 2);
        assert!(out.distros[0].sites.iter().all(|s| s.host.id == "wsl:D"));
    }

    #[test]
    fn stopped_unknown_and_unregistered_manual_wsl_paths_never_probe() {
        let paths = [
            manual(
                ManualVendor::Claude,
                r"\\wsl.localhost\Off\home\me\projects",
            ),
            manual(ManualVendor::Codex, r"\\wsl$\Unknown\home\me\sessions"),
            manual(ManualVendor::Claude, r"\\wsl$\Unregistered\custom\projects"),
        ];
        let out = scan_with(
            &|_| None,
            vec![distro("Off", Some(false)), distro("Unknown", None)],
            Vec::new(),
            &paths,
            &|_| panic!("켜짐이 확인되지 않은 WSL 경로에는 접근하지 않는다"),
        );
        assert!(out.sites.is_empty());
        assert!(out.distros.iter().all(|d| d.sites.is_empty()));
        assert_eq!(out.distros.len(), 2);
        assert_eq!(
            out.candidates.iter().map(|c| c.result).collect::<Vec<_>>(),
            vec![
                CandidateResult::Offline,
                CandidateResult::Unknown,
                CandidateResult::Unknown,
            ]
        );
        assert_eq!(out.candidates[2].root, Some(PathBuf::from(&paths[2].root)));
    }

    #[test]
    fn automatic_env_wsl_roots_use_the_same_running_gate() {
        let env = |k: &str| (k == "CLAUDE_CONFIG_DIR").then(|| r"\\wsl$\D\custom".into());
        let out = scan_with(
            &env,
            vec![distro("D", Some(false))],
            Vec::new(),
            &[],
            &|_| panic!(),
        );
        assert!(out.sites.is_empty());
        assert_eq!(out.candidates[0].host.id, "wsl:D");
        assert_eq!(out.candidates[0].result, CandidateResult::Offline);
        let probes = RefCell::new(Vec::new());
        let out = scan_with(&env, vec![distro("D", Some(true))], Vec::new(), &[], &|p| {
            probes.borrow_mut().push(p.to_owned());
            true
        });
        assert!(out.sites.is_empty());
        assert_eq!(out.distros[0].sites.len(), 1);
        assert_eq!(
            out.distros[0].sites[0].root,
            PathBuf::from(r"\\wsl$\D\custom\projects")
        );
        assert_eq!(probes.borrow().len(), 1);
    }

    #[test]
    fn invalid_manual_paths_never_probe() {
        let paths = [
            manual(ManualVendor::Claude, "relative/projects"),
            manual(ManualVendor::Codex, r"\\server\share\sessions"),
            manual(ManualVendor::Claude, r"\\wsl$\D\..\projects"),
        ];
        let out = scan_with(&|_| None, Vec::new(), Vec::new(), &paths, &|_| panic!());
        assert!(out.sites.is_empty());
        assert!(out.candidates.is_empty());
    }

    #[test]
    fn verbatim_wsl_env_never_bypasses_running_gate() {
        for root in [
            r"\\?\UNC\wsl.localhost\Off\custom",
            r"\\?\UNC\WSL$\Off\custom",
        ] {
            let env = |k: &str| (k == "CLAUDE_CONFIG_DIR").then(|| root.into());
            let out = scan_with(
                &env,
                vec![distro("Off", Some(false))],
                Vec::new(),
                &[],
                &|_| panic!("확장 UNC도 꺼진 WSL에는 접근하지 않는다"),
            );
            assert_eq!(out.candidates[0].host.id, "wsl:Off");
            assert_eq!(out.candidates[0].result, CandidateResult::Offline);
            assert!(out.sites.is_empty());
        }
    }
}
