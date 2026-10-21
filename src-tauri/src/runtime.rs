//! 설정 화면의 요청을 기존 탐색·호스트 스레드에 넘긴다. 진단 때문에 파일을 열지 않는다.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::discover::{
    Candidate, CandidateResult, Host, HostKind, ScanResult, Site, Vendor, candidate_key,
};
use crate::office::world::TeamKey;
use crate::settings::{ManualPath, ManualVendor};
use crate::tail::scheduler::{Heartbeat, ProbeUpdate};

type RootKey = (String, Vendor, Option<String>);

#[derive(Clone, Default)]
pub struct TickStats {
    pub count: u64,
    pub total_us: u128,
    pub max_us: u128,
    pub last_us: u128,
}

impl TickStats {
    pub fn record(&mut self, spent: Duration) {
        self.count += 1;
        self.last_us = spent.as_micros();
        self.total_us += self.last_us;
        self.max_us = self.max_us.max(self.last_us);
    }
}

#[derive(Clone)]
pub struct FileDiagnostic {
    pub team: TeamKey,
    pub path: PathBuf,
    pub offset: u64,
    pub generation: u32,
}

pub struct HostDiagnostic {
    pub host: Host,
    pub sites: Vec<Site>,
    pub files: Vec<FileDiagnostic>,
    pub ticks: TickStats,
    pub path_ops: u64,
    pub suspect: bool,
}

pub struct Diagnostics {
    pub candidates: Vec<Candidate>,
    pub hosts: Vec<HostDiagnostic>,
    pub unavailable: Vec<String>,
}

#[derive(Default)]
pub struct Runtime {
    pub rescan: AtomicBool,
    pub candidates: Mutex<Vec<Candidate>>,
    hosts: Mutex<BTreeMap<String, Arc<Heartbeat>>>,
    removed: Mutex<BTreeMap<RootKey, Candidate>>,
    ready: Mutex<Vec<Candidate>>,
    pub log_dir: PathBuf,
    pub home: Option<PathBuf>,
}

impl Runtime {
    pub fn new(log_dir: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            log_dir,
            home,
            ..Self::default()
        }
    }

    pub fn register(&self, host: &Host, heartbeat: Arc<Heartbeat>) {
        self.hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(host.id.clone(), heartbeat);
    }

    pub fn request_rescan(&self) {
        self.rescan.store(true, Ordering::Relaxed);
    }

    pub fn manual_changed(&self, old: &[ManualPath], new: &[ManualPath]) {
        let new: Vec<_> = new
            .iter()
            .map(manual_candidate)
            .map(|c| candidate_key(&c))
            .collect();
        let mut removed = self.removed.lock().unwrap_or_else(|e| e.into_inner());
        removed.retain(|key, _| !new.contains(key));
        for candidate in old.iter().map(manual_candidate) {
            let key = candidate_key(&candidate);
            if !new.contains(&key) {
                removed.insert(key, candidate);
            }
        }
        let mut ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
        ready.retain(|candidate| !new.contains(&candidate_key(candidate)));
        for heartbeat in self
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            if let Some(update) = heartbeat
                .probed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
            {
                update.removed.retain(|(vendor, root)| {
                    let vendor = match vendor {
                        Vendor::Claude => ManualVendor::Claude,
                        Vendor::Codex => ManualVendor::Codex,
                        Vendor::Gemini => return true,
                    };
                    let candidate = manual_candidate(&ManualPath {
                        vendor,
                        root: root.to_string_lossy().into_owned(),
                    });
                    !new.contains(&candidate_key(&candidate))
                });
            }
        }
        self.request_rescan();
    }

    pub fn discovered(&self, scan: &ScanResult, manual: &[ManualPath]) {
        *self.candidates.lock().unwrap_or_else(|e| e.into_inner()) = scan.candidates.clone();
        let keys: Vec<_> = scan.candidates.iter().map(candidate_key).collect();
        let mut removed = self.removed.lock().unwrap_or_else(|e| e.into_inner());
        let mut ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
        removed.retain(|key, candidate| {
            // 탐색 중 설정이 바뀌었으면 그 탐색에 수동으로 넣었던 경로를 자동 경로로 오인하지 않는다.
            if manual
                .iter()
                .map(manual_candidate)
                .any(|c| candidate_key(&c) == *key)
            {
                return true;
            }
            // 이번 후보에도 있으면 자동 경로거나 다시 추가한 경로이므로 지우지 않는다.
            if keys.contains(key)
                || scan
                    .sites
                    .iter()
                    .chain(scan.distros.iter().flat_map(|d| &d.sites))
                    .any(|site| {
                        candidate_key(&Candidate {
                            host: site.host.clone(),
                            vendor: site.vendor,
                            root: Some(site.root.clone()),
                            result: CandidateResult::Found,
                        }) == *key
                    })
            {
                return false;
            }
            if matches!(candidate.host.kind, HostKind::Wsl { .. }) {
                let running = scan
                    .distros
                    .iter()
                    .find(|d| d.host.id.eq_ignore_ascii_case(&candidate.host.id))
                    .and_then(|d| d.running);
                if running != Some(true)
                    || scan.candidates.iter().any(|c| {
                        c.host.id.eq_ignore_ascii_case(&candidate.host.id)
                            && c.vendor == candidate.vendor
                            && c.result == CandidateResult::Unknown
                    })
                {
                    return true;
                }
            }
            ready.push(candidate.clone());
            false
        });
    }

    pub fn publish_probe(&self, host: &Host, heartbeat: &Heartbeat, sites: Vec<Site>) {
        let mut ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
        let mut pending = heartbeat.probed.lock().unwrap_or_else(|e| e.into_inner());
        let mut removed = pending
            .take()
            .map_or_else(Vec::new, |update| update.removed);
        ready.retain(|candidate| {
            if candidate.host.id.eq_ignore_ascii_case(&host.id) {
                removed.push((candidate.vendor, candidate.root.clone().unwrap()));
                false
            } else {
                true
            }
        });
        // 제거와 그 판단에 쓴 Site를 한 번에 넘겨 하위 자동 경로를 먼저 보호한다.
        *pending = Some(ProbeUpdate { sites, removed });
    }

    /// 모든 호스트에 같은 기한을 쓴다. 멈춘 호스트 하나 때문에 나머지 응답을 버리지 않는다.
    pub fn diagnostics(&self, timeout: Duration) -> Diagnostics {
        let deadline = Instant::now() + timeout;
        let candidates = self
            .candidates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (tx, rx) = mpsc::channel();
        let mut waiting = BTreeMap::new();
        let mut unavailable = Vec::new();
        for (id, heartbeat) in hosts {
            let mut pending = heartbeat
                .diagnostic
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if pending.is_none() {
                *pending = Some(tx.clone());
                waiting.insert(id, ());
            } else {
                // 이전 요청에도 답하지 못한 호스트에 요청을 쌓지 않는다.
                unavailable.push(id);
            }
        }
        drop(tx);
        let mut hosts = Vec::new();
        while !waiting.is_empty() {
            let Ok(snapshot) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                break;
            };
            waiting.remove(&snapshot.host.id);
            hosts.push(snapshot);
        }
        unavailable.extend(waiting.into_keys());
        hosts.sort_by(|a, b| a.host.id.cmp(&b.host.id));
        Diagnostics {
            candidates,
            hosts,
            unavailable,
        }
    }
}

fn manual_candidate(path: &ManualPath) -> Candidate {
    let normalized = path.root.replace('/', "\\");
    let host = if let Some(unc) = normalized.strip_prefix("\\\\") {
        let distro = unc.split('\\').nth(1).unwrap_or_default().to_owned();
        Host {
            id: format!("wsl:{distro}"),
            label: distro.clone(),
            kind: HostKind::Wsl { distro },
        }
    } else {
        crate::discover::windows_host()
    };
    Candidate {
        host,
        vendor: match path.vendor {
            ManualVendor::Claude => Vendor::Claude,
            ManualVendor::Codex => Vendor::Codex,
        },
        root: Some(path.root.clone().into()),
        result: CandidateResult::Unknown,
    }
}

pub(crate) fn contains_root(
    host: &Host,
    vendor: Vendor,
    root: &std::path::Path,
    path: &std::path::Path,
) -> bool {
    let key = |path: &std::path::Path| {
        candidate_key(&Candidate {
            host: host.clone(),
            vendor,
            root: Some(path.to_path_buf()),
            result: CandidateResult::Found,
        })
        .2
        .unwrap()
    };
    let root = key(root);
    let path = key(path);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{windows_host, wsl::WslDistro};

    fn path(root: &str) -> ManualPath {
        ManualPath {
            vendor: ManualVendor::Claude,
            root: root.into(),
        }
    }
    fn scan() -> ScanResult {
        ScanResult {
            sites: Vec::new(),
            distros: Vec::new(),
            candidates: Vec::new(),
        }
    }

    #[test]
    fn only_explicit_removed_paths_are_queued_after_a_conclusive_scan() {
        let runtime = Runtime::default();
        let manual = path(r"\\wsl$\Test\custom\records");
        let candidate = manual_candidate(&manual);
        let heartbeat = Arc::new(Heartbeat::default());
        runtime.register(&candidate.host, heartbeat.clone());
        runtime.manual_changed(std::slice::from_ref(&manual), &[]);
        let mut found = scan();
        found.distros.push(WslDistro {
            host: candidate.host.clone(),
            running: Some(false),
            sites: Vec::new(),
        });
        runtime.discovered(&found, &[]);
        assert!(runtime.ready.lock().unwrap().is_empty());
        found.distros[0].running = Some(true);
        found.candidates.push(Candidate {
            root: None,
            ..candidate.clone()
        });
        runtime.discovered(&found, &[]);
        assert!(runtime.ready.lock().unwrap().is_empty());
        found.candidates.clear();
        runtime.discovered(&found, &[]);
        assert_eq!(runtime.ready.lock().unwrap().len(), 1);
        runtime.publish_probe(&candidate.host, &heartbeat, Vec::new());
        assert_eq!(
            heartbeat
                .probed
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .removed
                .len(),
            1
        );
        runtime.manual_changed(&[], &[path(r"\\WSL.LOCALHOST\test\custom\records")]);
        assert!(
            heartbeat
                .probed
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .removed
                .is_empty(),
            "다시 추가한 루트는 지우지 않는다"
        );
    }

    #[test]
    fn automatic_duplicate_and_stale_scan_do_not_delete_the_root() {
        let runtime = Runtime::default();
        let manual = path("D:/fixture/records");
        let heartbeat = Arc::new(Heartbeat::default());
        runtime.register(&windows_host(), heartbeat.clone());
        runtime.manual_changed(std::slice::from_ref(&manual), &[]);
        let mut found = scan();
        found.candidates.push(Candidate {
            result: CandidateResult::Found,
            ..manual_candidate(&manual)
        });
        runtime.discovered(&found, std::slice::from_ref(&manual));
        assert_eq!(
            runtime.removed.lock().unwrap().len(),
            1,
            "이전 설정으로 시작한 탐색이면 보류한다"
        );
        runtime.discovered(&found, &[]);
        assert!(
            runtime.removed.lock().unwrap().is_empty(),
            "수동 없이도 발견한 자동 루트는 유지한다"
        );
        assert!(runtime.ready.lock().unwrap().is_empty());
        found.candidates.clear();
        runtime.discovered(&found, &[]);
        assert!(
            runtime.ready.lock().unwrap().is_empty(),
            "탐색에서 빠진 것만으로 삭제하지 않는다"
        );
    }

    #[test]
    fn diagnostic_deadline_returns_the_responsive_host_without_accumulating_requests() {
        let runtime = Runtime::default();
        let responsive = Arc::new(Heartbeat::default());
        runtime.register(&windows_host(), responsive.clone());
        let remote = manual_candidate(&path(r"\\wsl$\Off\records")).host;
        let blocked = Arc::new(Heartbeat::default());
        runtime.register(&remote, blocked.clone());
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(tx) = responsive.diagnostic.lock().unwrap().take() {
                    tx.send(HostDiagnostic {
                        host: windows_host(),
                        sites: Vec::new(),
                        files: Vec::new(),
                        ticks: TickStats::default(),
                        path_ops: 0,
                        suspect: false,
                    })
                    .unwrap();
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        });
        let data = runtime.diagnostics(Duration::from_millis(200));
        worker.join().unwrap();
        assert_eq!(data.hosts.len(), 1);
        assert_eq!(data.unavailable, [remote.id]);
        assert!(blocked.diagnostic.lock().unwrap().is_some());
        let next = runtime.diagnostics(Duration::ZERO);
        assert_eq!(next.unavailable.len(), 2);
    }
}
