//! Windows 홈과 환경변수에서 Site 찾기 (PLAN 4.4.1)

use std::path::PathBuf;

use super::{Host, HostKind, ReadMode, Site, Vendor};

pub fn windows_host() -> Host {
    Host {
        id: "windows".into(),
        kind: HostKind::Windows,
        label: "Windows".into(),
    }
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

/// 루트 폴더가 실제로 있는 Claude·Codex Site만 낸다. Antigravity는 파서가 생기는 T32에서 더한다
pub fn find_sites(env: &dyn Fn(&str) -> Option<String>) -> Vec<Site> {
    let candidates = [
        (Vendor::Claude, claude_home(env).map(|h| h.join("projects"))),
        (
            Vendor::Codex,
            under_profile(env, "CODEX_HOME", ".codex").map(|c| c.join("sessions")),
        ),
    ];
    candidates
        .into_iter()
        .filter_map(|(vendor, root)| Some((vendor, root?)))
        .filter(|(_, root)| root.is_dir())
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
