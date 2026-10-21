//! WSL 배포판에서 Site 찾기 (PLAN 4.4.2)
//!
//! 꺼진 배포판의 UNC 경로는 만들지도 열지도 않는다. 첫 UNC 접근이 VM을 부팅시키기 때문이다 (PLAN 1.3).
//! 그래서 UNC 루트는 `roots` 주입 함수로만 얻고, 켜졌다고 확인된 배포판에만 부른다.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegGetValueW, RegOpenKeyExW,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use super::{Candidate, CandidateResult, Host, HostKind, ReadMode, Site, Vendor};

/// Lxss 레지스트리의 배포판 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LxssEntry {
    pub name: String,
    /// 값이 없으면 passwd 단계를 건너뛰고 printenv 폴백으로 간다
    pub default_uid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WslDistro {
    pub host: Host,
    /// Some(false)는 소등. None은 `wsl.exe -l --running` 이 실패해 모르는 것
    pub running: Option<bool>,
    /// 켜진 배포판만 채운다
    pub sites: Vec<Site>,
}

/// `running` 은 레지스트리에 배포판이 있을 때만 한 번 부른다.
/// `roots` 는 배포판 이름의 UNC 루트 후보를 순서대로 준다. 켜진 배포판에만 부른다.
/// `printenv` 는 배포판 안 `printenv` 출력이다. 켜진 배포판에서 홈을 못 찾았을 때만 부른다
pub fn find_distros(
    lxss: &[LxssEntry],
    running: &dyn Fn() -> Option<Vec<String>>,
    roots: &dyn Fn(&str) -> Vec<PathBuf>,
    printenv: &dyn Fn(&str) -> Option<String>,
) -> Vec<WslDistro> {
    find_distros_reporting(lxss, running, roots, printenv, &mut Vec::new())
}

fn find_distros_reporting(
    lxss: &[LxssEntry],
    running: &dyn Fn() -> Option<Vec<String>>,
    roots: &dyn Fn(&str) -> Vec<PathBuf>,
    printenv: &dyn Fn(&str) -> Option<String>,
    candidates: &mut Vec<Candidate>,
) -> Vec<WslDistro> {
    if lxss.is_empty() {
        return Vec::new();
    }
    let on = running();
    lxss.iter()
        .map(|e| {
            let host = Host {
                id: format!("wsl:{}", e.name),
                kind: HostKind::Wsl {
                    distro: e.name.clone(),
                },
                label: e.name.clone(),
            };
            let running = on.as_ref().map(|names| names.contains(&e.name));
            let first = candidates.len();
            let sites = if running == Some(true) {
                sites_of(e, &host, roots, printenv, candidates)
            } else {
                Vec::new()
            };
            for vendor in [Vendor::Claude, Vendor::Codex] {
                if !candidates[first..].iter().any(|c| c.vendor == vendor) {
                    candidates.push(Candidate {
                        host: host.clone(),
                        vendor,
                        // 소등·홈 탐색 실패에서는 경로를 알아내려고 UNC를 더 열지 않는다.
                        root: None,
                        result: if running == Some(false) {
                            CandidateResult::Offline
                        } else {
                            CandidateResult::Unknown
                        },
                    });
                }
            }
            WslDistro {
                host,
                running,
                sites,
            }
        })
        .collect()
}

fn sites_of(
    e: &LxssEntry,
    host: &Host,
    roots: &dyn Fn(&str) -> Vec<PathBuf>,
    printenv: &dyn Fn(&str) -> Option<String>,
    candidates: &mut Vec<Candidate>,
) -> Vec<Site> {
    let Some(root) = roots(&e.name).into_iter().find(|r| r.is_dir()) else {
        return Vec::new();
    };
    let home = e.default_uid.and_then(|uid| {
        let passwd = fs::read_to_string(root.join("etc").join("passwd")).ok()?;
        under_root(&root, &home_of(&passwd, uid)?)
    });
    // 홈을 찾았으면 폴더가 없어도 끝낸다. printenv 는 배포판을 깨우므로 홈을 못 찾았을 때만 부른다
    // ponytail: 홈을 찾으면 CLAUDE_CONFIG_DIR·CODEX_HOME 은 보지 않는다, 환경변수로 폴더를 옮긴 WSL 사용자가 보이면 올린다
    // ponytail: 기본 사용자는 레지스트리 DefaultUid 로만 정한다(/etc/wsl.conf 의 [user] default 는 안 본다), 그 구성이 보이면 올린다
    if let Some(home) = home {
        return [
            site(
                host,
                Vendor::Claude,
                home.join(".claude").join("projects"),
                candidates,
            ),
            site(
                host,
                Vendor::Codex,
                home.join(".codex").join("sessions"),
                candidates,
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
    }
    let Some(out) = printenv(&e.name) else {
        return Vec::new();
    };
    let var = |k: &str| env_value(&out, k);
    let linux_home = var("HOME");
    let claude =
        var("CLAUDE_CONFIG_DIR").or_else(|| Some(format!("{}/.claude", linux_home.as_ref()?)));
    let codex = var("CODEX_HOME").or_else(|| Some(format!("{}/.codex", linux_home.as_ref()?)));
    let at = |linux: Option<String>, sub: &str| under_root(&root, &linux?).map(|p| p.join(sub));
    [
        at(claude, "projects").and_then(|r| site(host, Vendor::Claude, r, candidates)),
        at(codex, "sessions").and_then(|r| site(host, Vendor::Codex, r, candidates)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Antigravity CLI는 파서가 생기는 T32에서 더한다
fn site(
    host: &Host,
    vendor: Vendor,
    root: PathBuf,
    candidates: &mut Vec<Candidate>,
) -> Option<Site> {
    let found = root.is_dir();
    candidates.push(Candidate {
        host: host.clone(),
        vendor,
        root: Some(root.clone()),
        result: if found {
            CandidateResult::Found
        } else {
            CandidateResult::Missing
        },
    });
    found.then(|| Site {
        host: host.clone(),
        vendor,
        root,
        read_mode: ReadMode::Append,
    })
}

/// passwd 에서 세 번째 필드가 uid 인 줄의 여섯 번째 필드
fn home_of(passwd: &str, uid: u32) -> Option<String> {
    passwd
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.get(2).and_then(|u| u.parse::<u32>().ok()) == Some(uid))
        .and_then(|f| f.get(5).map(|h| h.to_string()))
        .filter(|h| !h.is_empty())
}

/// Linux 절대 경로를 배포판 루트 아래 Windows 경로로. 루트를 벗어나거나
/// Windows 경로로 해석될 수 있는 조각(`..`, `\`, `:`)이 있으면 None
fn under_root(root: &Path, linux: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for c in linux.strip_prefix('/')?.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c if c.contains(['\\', ':']) => return None,
            c => p.push(c),
        }
    }
    Some(p)
}

/// `KEY=VALUE` 줄에서 처음 나온 값. 빈 값은 없는 것으로 본다
fn env_value(printenv: &str, key: &str) -> Option<String> {
    printenv
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim_end_matches('\r').to_string())
        .filter(|v| !v.is_empty())
}

/// `wsl.exe -l --running -q` 출력. UTF-16LE이고 BOM·NUL·`\r\n` 이 섞일 수 있다
fn parse_running(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
        .split('\n')
        .map(|l| l.trim_matches(|c: char| c == '\u{feff}' || c == '\0' || c.is_whitespace()))
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// 실제 환경의 WSL 배포판 목록. 켜진 배포판만 UNC로 본다
pub(super) fn discover() -> Vec<WslDistro> {
    discover_with_candidates().0
}

pub(super) fn discover_with_candidates() -> (Vec<WslDistro>, Vec<Candidate>) {
    let mut candidates = Vec::new();
    let distros = find_distros_reporting(
        &read_lxss(),
        &|| run_wsl(&["-l", "--running", "-q"]).map(|b| parse_running(&b)),
        &unc_roots,
        &|name| {
            run_wsl(&["-d", name, "--", "printenv"])
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        },
        &mut candidates,
    );
    (distros, candidates)
}

fn unc_roots(name: &str) -> Vec<PathBuf> {
    // 레지스트리 값이 경로 조각으로 해석되면 다른 공유를 열게 된다
    if name.is_empty() || name == "." || name == ".." || name.contains(['\\', '/', ':']) {
        return Vec::new();
    }
    vec![
        PathBuf::from(format!(r"\\wsl.localhost\{name}\")),
        PathBuf::from(format!(r"\\wsl$\{name}\")),
    ]
}

// ponytail: wsl.exe 가 멈추면 호출 스레드도 멈춘다, T12 호스트별 스레드에서 감시자가 필요해지면 타임아웃을 단다
fn run_wsl(args: &[&str]) -> Option<Vec<u8>> {
    // 현재 폴더나 PATH 앞쪽의 다른 wsl.exe 를 집지 않도록 System32 를 직접 가리킨다
    let exe = PathBuf::from(std::env::var_os("SystemRoot")?).join(r"System32\wsl.exe");
    let out = Command::new(exe)
        .args(args)
        // WSL_UTF8=1 이면 -l 출력이 UTF-8 로 바뀐다
        .env_remove("WSL_UTF8")
        .stderr(Stdio::null())
        // 창 없는 앱에서 콘솔 창이 깜빡이지 않게
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 키가 없으면 WSL 미설치로 보고 빈 목록. 이름을 못 읽은 하위 키는 건너뛴다
fn read_lxss() -> Vec<LxssEntry> {
    let base = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: 키는 이 함수 안에서 열고 닫는다
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, base.as_ptr(), 0, KEY_READ, &mut key) }
        != ERROR_SUCCESS
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0.. {
        // 레지스트리 키 이름은 최대 255자
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: buf 와 len 이 버퍼 크기를 맞춰 넘긴다
        let r = unsafe {
            RegEnumKeyExW(
                key,
                i,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        // ERROR_NO_MORE_ITEMS 와 그 밖의 실패 모두 멈춘다. 계속 돌면 무한 반복할 수 있다
        if r != ERROR_SUCCESS {
            break;
        }
        let mut sub = buf[..len as usize].to_vec();
        sub.push(0);
        if let Some(name) = reg_sz(key, &sub, "DistributionName") {
            out.push(LxssEntry {
                name,
                default_uid: reg_dword(key, &sub, "DefaultUid"),
            });
        }
    }
    // SAFETY: 위에서 연 키
    unsafe { RegCloseKey(key) };
    out
}

fn reg_get(
    key: HKEY,
    sub: &[u16],
    value: &str,
    flags: u32,
    data: *mut u8,
    cb: &mut u32,
) -> WIN32_ERROR {
    let value = wide(value);
    // SAFETY: sub·value 는 NUL 로 끝나고 data 는 cb 바이트 이상이거나 크기만 물을 때 null
    unsafe {
        RegGetValueW(
            key,
            sub.as_ptr(),
            value.as_ptr(),
            flags,
            std::ptr::null_mut(),
            data.cast(),
            cb,
        )
    }
}

fn reg_sz(key: HKEY, sub: &[u16], value: &str) -> Option<String> {
    let mut cb = 0u32;
    if reg_get(
        key,
        sub,
        value,
        RRF_RT_REG_SZ,
        std::ptr::null_mut(),
        &mut cb,
    ) != ERROR_SUCCESS
    {
        return None;
    }
    let mut buf = vec![0u16; (cb as usize).div_ceil(2)];
    if reg_get(
        key,
        sub,
        value,
        RRF_RT_REG_SZ,
        buf.as_mut_ptr().cast(),
        &mut cb,
    ) != ERROR_SUCCESS
    {
        return None;
    }
    buf.truncate(cb as usize / 2);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf16(&buf).ok().filter(|s| !s.is_empty())
}

fn reg_dword(key: HKEY, sub: &[u16], value: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut cb = 4u32;
    (reg_get(
        key,
        sub,
        value,
        RRF_RT_REG_DWORD,
        (&mut v as *mut u32).cast(),
        &mut cb,
    ) == ERROR_SUCCESS)
        .then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn entry(name: &str, uid: Option<u32>) -> LxssEntry {
        LxssEntry {
            name: name.into(),
            default_uid: uid,
        }
    }

    /// 임시 폴더 아래 `<이름>` 을 가짜 배포판 루트로 만든다
    fn fake_root(base: &Path, name: &str, passwd: &str) -> PathBuf {
        let root = base.join(name);
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(root.join("etc").join("passwd"), passwd).unwrap();
        root
    }

    fn vendors(d: &WslDistro) -> Vec<(Vendor, PathBuf)> {
        d.sites.iter().map(|s| (s.vendor, s.root.clone())).collect()
    }

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\nme:x:1000:1000::/home/me:/bin/bash\n";

    #[test]
    fn candidate_report_keeps_missing_paths_and_never_looks_up_stopped_homes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fake_root(tmp.path(), "On", PASSWD);
        fs::create_dir_all(root.join("home/me/.claude/projects")).unwrap();
        let mut candidates = Vec::new();
        let distros = find_distros_reporting(
            &[entry("Off", Some(1000)), entry("On", Some(1000))],
            &|| Some(vec!["On".into()]),
            &|name| {
                assert_eq!(name, "On");
                vec![root.clone()]
            },
            &|_| panic!("홈이 있으면 printenv를 부르지 않는다"),
            &mut candidates,
        );
        assert!(distros[0].sites.is_empty());
        assert_eq!(distros[1].sites.len(), 1);
        assert_eq!(candidates.len(), 4);
        assert!(
            candidates[..2]
                .iter()
                .all(|c| c.root.is_none() && c.result == CandidateResult::Offline)
        );
        assert_eq!(candidates[2].result, CandidateResult::Found);
        assert_eq!(candidates[3].result, CandidateResult::Missing);
        assert_eq!(
            candidates[3].root,
            Some(root.join("home/me/.codex/sessions"))
        );
        candidates.clear();
        find_distros_reporting(
            &[entry("Unknown", Some(1000))],
            &|| None,
            &|_| panic!("켜짐을 모르면 UNC를 건드리지 않는다"),
            &|_| panic!(),
            &mut candidates,
        );
        assert!(
            candidates
                .iter()
                .all(|c| c.root.is_none() && c.result == CandidateResult::Unknown)
        );
    }

    #[test]
    fn no_registry_gives_empty_and_runs_nothing() {
        let got = find_distros(
            &[],
            &|| panic!("배포판이 없으면 wsl.exe 를 띄우지 않는다"),
            &|_| panic!(),
            &|_| panic!(),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn stopped_distro_never_reaches_path_or_process() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fake_root(tmp.path(), "On", PASSWD);
        fs::create_dir_all(root.join("home/me/.claude/projects")).unwrap();
        fake_root(tmp.path(), "Off", PASSWD);
        let path_calls = RefCell::new(Vec::new());
        let env_calls = RefCell::new(Vec::new());
        let roots = |n: &str| {
            path_calls.borrow_mut().push(n.to_string());
            vec![tmp.path().join(n)]
        };
        let printenv = |n: &str| {
            env_calls.borrow_mut().push(n.to_string());
            None
        };
        let lxss = [entry("Off", Some(1000)), entry("On", Some(1000))];

        let got = find_distros(&lxss, &|| Some(vec!["On".into()]), &roots, &printenv);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].host.label, "Off");
        assert_eq!(got[0].running, Some(false));
        assert!(got[0].sites.is_empty());
        assert_eq!(got[1].running, Some(true));
        assert_eq!(got[1].sites.len(), 1);
        assert_eq!(*path_calls.borrow(), vec!["On".to_string()]);
        assert!(env_calls.borrow().is_empty());

        // 켜진 목록을 모르면 어느 배포판도 건드리지 않는다
        path_calls.borrow_mut().clear();
        let got = find_distros(&lxss, &|| None, &roots, &printenv);
        assert!(
            got.iter()
                .all(|d| d.running.is_none() && d.sites.is_empty())
        );
        assert!(path_calls.borrow().is_empty());
        assert!(env_calls.borrow().is_empty());
    }

    #[test]
    fn home_is_sixth_field_of_default_uid_line() {
        let passwd = "# 주석:x:1000:1000::/wrong:/bin/sh\r\n\
                      \r\n\
                      root:x:0:0:root:/root:/bin/bash\r\n\
                      other:x:1001:1001::/home/other:/bin/bash\r\n\
                      broken:x:abc\r\n\
                      me:x:1000:1000:Me,,,:/home/me:/bin/zsh\r\n";
        assert_eq!(home_of(passwd, 1000).as_deref(), Some("/home/me"));
        assert_eq!(home_of(passwd, 0).as_deref(), Some("/root"));
        assert_eq!(home_of(passwd, 4242), None);
    }

    #[test]
    fn running_list_parses_utf16le_with_bom_nul_crlf() {
        let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        // 개발 PC 실측 모양: BOM·NUL 없이 이름 + \r\n
        assert_eq!(parse_running(&utf16("Rocky-9\r\n")), vec!["Rocky-9"]);
        let mut bytes = utf16("\u{feff}Ubuntu-24.04\r\n\r\nRocky-9\0\r\n\0");
        bytes.push(0x41); // 홀수 바이트 꼬리는 버린다
        assert_eq!(parse_running(&bytes), vec!["Ubuntu-24.04", "Rocky-9"]);
        assert!(parse_running(&[]).is_empty());
    }

    #[test]
    fn only_existing_vendor_dirs_become_sites() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fake_root(tmp.path(), "D", PASSWD);
        fs::create_dir_all(root.join("home/me/.codex/sessions")).unwrap();
        // 다른 사용자 홈의 Claude 폴더는 보지 않는다
        fs::create_dir_all(root.join("root/.claude/projects")).unwrap();
        // 첫 후보 루트(\\wsl.localhost 자리)가 없으면 다음 후보(\\wsl$ 자리)를 쓴다
        let got = find_distros(
            &[entry("D", Some(1000))],
            &|| Some(vec!["D".into()]),
            &|n| vec![tmp.path().join("missing"), tmp.path().join(n)],
            &|_| panic!("홈에서 찾았으면 폴백하지 않는다"),
        );
        assert_eq!(
            vendors(&got[0]),
            vec![(Vendor::Codex, root.join("home/me/.codex/sessions"))]
        );
        let s = &got[0].sites[0];
        assert_eq!(s.host.id, "wsl:D");
        assert_eq!(s.host.kind, HostKind::Wsl { distro: "D".into() });
        assert_eq!(s.read_mode, ReadMode::Append);
    }

    #[test]
    fn found_home_without_vendor_dirs_never_runs_printenv() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fake_root(tmp.path(), "D", PASSWD);
        fs::create_dir_all(root.join("home/me")).unwrap();
        let env_calls = RefCell::new(Vec::new());
        let got = find_distros(
            &[entry("D", Some(1000))],
            &|| Some(vec!["D".into()]),
            &|n| vec![tmp.path().join(n)],
            &|n| {
                env_calls.borrow_mut().push(n.to_string());
                None
            },
        );
        assert!(got[0].sites.is_empty());
        assert!(env_calls.borrow().is_empty());
    }

    #[test]
    fn printenv_fallback_when_home_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        // passwd 에 DefaultUid 줄이 없다
        let root = fake_root(tmp.path(), "D", "root:x:0:0:root:/root:/bin/bash\n");
        fs::create_dir_all(root.join("home/u/.claude/projects")).unwrap();
        fs::create_dir_all(root.join("opt/codex/sessions")).unwrap();
        let out = "PATH=/usr/bin\nHOME=/home/u\nCODEX_HOME=/opt/codex\nCLAUDE_CONFIG_DIR=\n";
        let got = find_distros(
            &[entry("D", Some(1000))],
            &|| Some(vec!["D".into()]),
            &|n| vec![tmp.path().join(n)],
            &|_| Some(out.into()),
        );
        assert_eq!(
            vendors(&got[0]),
            vec![
                (Vendor::Claude, root.join("home/u/.claude/projects")),
                (Vendor::Codex, root.join("opt/codex/sessions")),
            ]
        );

        // 루트를 벗어나는 경로는 따라가지 않는다
        fs::create_dir_all(tmp.path().join("escape/sessions")).unwrap();
        let got = find_distros(
            &[entry("D", None)],
            &|| Some(vec!["D".into()]),
            &|n| vec![tmp.path().join(n)],
            &|_| Some("CODEX_HOME=/../escape\n".into()),
        );
        assert!(got[0].sites.is_empty());
    }
}
