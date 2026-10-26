//! Windows 홈과 환경변수에서 Site 찾기 (docs/CONTRACTS.md C5)

use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;

use super::{Host, HostKind, ReadMode, Site, Vendor};

pub fn windows_host() -> Host {
    Host {
        id: "windows".into(),
        kind: HostKind::Windows,
        label: "Windows".into(),
    }
}

pub(super) fn is_local_path(root: &Path) -> bool {
    local_path_with(root, &|drive| {
        let name: Vec<_> = drive.encode_utf16().chain(Some(0)).collect();
        let mut target = vec![0u16; 32768];
        let len =
            unsafe { QueryDosDeviceW(name.as_ptr(), target.as_mut_ptr(), target.len() as u32) };
        if len == 0 {
            return None;
        }
        let end = target.iter().position(|c| *c == 0)?;
        String::from_utf16(&target[..end]).ok()
    })
}

fn local_path_with(root: &Path, mapping: &dyn Fn(&str) -> Option<String>) -> bool {
    let path = root.to_string_lossy().replace('/', "\\");
    let path = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\\.\"))
        .unwrap_or(&path);
    let bytes = path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
        return false;
    }
    // 매핑 드라이브를 열어 WSL을 깨우지 않도록 로컬 볼륨임을 먼저 확인한다.
    // ponytail: 로컬 SUBST도 미확인으로 둔다, 원래 경로 입력으로 해결되지 않는 사용례가 생기면 재귀 해석을 추가한다.
    mapping(&path[..2]).is_some_and(|target| {
        target
            .strip_prefix(r"\Device\HarddiskVolume")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// 빈 문자열은 없는 것으로 본다
fn non_empty(v: Option<String>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

fn under_profile(
    env: &dyn Fn(&str) -> Option<String>,
    override_var: &str,
    dot_dir: &str,
) -> Option<PathBuf> {
    non_empty(env(override_var)).or_else(|| Some(non_empty(env("USERPROFILE"))?.join(dot_dir)))
}

/// `CLAUDE_CONFIG_DIR ?? <USERPROFILE>\.claude`
pub fn claude_home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    under_profile(env, "CLAUDE_CONFIG_DIR", ".claude")
}

pub(super) fn candidate_roots(env: &dyn Fn(&str) -> Option<String>) -> Vec<(Vendor, PathBuf)> {
    [
        (Vendor::Claude, claude_home(env).map(|h| h.join("projects"))),
        (
            Vendor::Codex,
            under_profile(env, "CODEX_HOME", ".codex").map(|c| c.join("sessions")),
        ),
    ]
    .into_iter()
    .filter_map(|(vendor, root)| Some((vendor, root?)))
    .collect()
}

/// 루트 폴더가 실제로 있는 Claude·Codex Site만 낸다. Antigravity는 파서가 생기는 T32에서 더한다
pub fn find_sites(env: &dyn Fn(&str) -> Option<String>) -> Vec<Site> {
    candidate_roots(env)
        .into_iter()
        // WSL 환경변수 경로는 켜짐을 확인하는 통합 scan에서만 다룬다.
        .filter(|(_, root)| super::wsl_path(root).is_none() && is_local_path(root) && root.is_dir())
        .map(|(vendor, root)| Site {
            host: windows_host(),
            vendor,
            root,
            read_mode: ReadMode::Append,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn mapped_and_unknown_drives_are_not_treated_as_local() {
        for root in [r"Z:\sessions", r"\\?\Z:\sessions", r"\\.\Z:\sessions"] {
            for target in [
                None,
                Some(r"\Device\LanmanRedirector\;Z:\wsl$\Stopped"),
                Some(r"\??\C:\other"),
                Some(r"\Device\HarddiskVolume1\other"),
            ] {
                assert!(!local_path_with(Path::new(root), &|drive| {
                    assert_eq!(drive, "Z:");
                    target.map(str::to_owned)
                }));
            }
            assert!(local_path_with(Path::new(root), &|_| Some(
                r"\Device\HarddiskVolume12".into()
            )));
        }
        assert!(!local_path_with(
            Path::new(r"\\.\UNC\wsl$\Stopped\sessions"),
            &|_| panic!("공유에는 장치 조회도 필요 없다")
        ));
    }

    fn env_of<'a>(pairs: &'a [(&'a str, String)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
    }

    fn roots(sites: &[Site]) -> Vec<(Vendor, PathBuf)> {
        sites.iter().map(|s| (s.vendor, s.root.clone())).collect()
    }

    #[test]
    fn falls_back_under_userprofile_without_env() {
        let h = tempfile::tempdir().unwrap();
        fs::create_dir_all(h.path().join(".claude/projects")).unwrap();
        fs::create_dir_all(h.path().join(".codex/sessions")).unwrap();
        let pairs = [("USERPROFILE", h.path().display().to_string())];
        let sites = find_sites(&env_of(&pairs));
        assert_eq!(
            roots(&sites),
            vec![
                (Vendor::Claude, h.path().join(".claude/projects")),
                (Vendor::Codex, h.path().join(".codex/sessions")),
            ]
        );
        assert!(sites.iter().all(|s| s.read_mode == ReadMode::Append));
    }

    #[test]
    fn env_vars_take_priority_and_empty_means_unset() {
        let h = tempfile::tempdir().unwrap();
        let (c, x) = (h.path().join("c"), h.path().join("x"));
        fs::create_dir_all(c.join("projects")).unwrap();
        fs::create_dir_all(x.join("sessions")).unwrap();
        // USERPROFILE 쪽에도 폴더를 만들어 두어 우선순위가 틀리면 드러나게 한다
        fs::create_dir_all(h.path().join(".claude/projects")).unwrap();
        let pairs = [
            ("USERPROFILE", h.path().display().to_string()),
            ("CLAUDE_CONFIG_DIR", c.display().to_string()),
            ("CODEX_HOME", x.display().to_string()),
        ];
        assert_eq!(
            roots(&find_sites(&env_of(&pairs))),
            vec![
                (Vendor::Claude, c.join("projects")),
                (Vendor::Codex, x.join("sessions"))
            ]
        );
        let empty = [
            ("USERPROFILE", h.path().display().to_string()),
            ("CLAUDE_CONFIG_DIR", String::new()),
        ];
        assert_eq!(
            roots(&find_sites(&env_of(&empty))),
            vec![(Vendor::Claude, h.path().join(".claude/projects"))]
        );
    }

    #[test]
    fn missing_root_dir_gives_no_site() {
        let h = tempfile::tempdir().unwrap();
        fs::create_dir_all(h.path().join(".codex/sessions")).unwrap();
        let pairs = [("USERPROFILE", h.path().display().to_string())];
        let sites = find_sites(&env_of(&pairs));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].vendor, Vendor::Codex);
    }
}
