//! 호스트별 폴링 스레드 (docs/CONTRACTS.md C6·C5)

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration as StdDuration, Instant, SystemTime};

use chrono::{DateTime, Duration, Utc};

use super::{MAX_READ_BYTES, Pending, TAIL_HASH_BYTES, TailReader, hash_bytes, read_range};
use crate::diag::Diag;
use crate::discover::registry;
use crate::discover::wsl::WslDistro;
use crate::discover::{Host, HostKind, Product, Site, Vendor};
use crate::office::classify::{SubMeta, rebuild};
use crate::office::world::{OfficeWorld, TeamKey};
use crate::office::{Cue, TeamPhase};
use crate::parse::claude::ClaudeParser;
use crate::parse::codex::CodexParser;
use crate::parse::{Event, EventKind, Parser, RawRef, Unhandled};
use crate::runtime::{FileDiagnostic, HostDiagnostic, Runtime, TickStats};
use crate::startup::ScanWatch;

const HOT: Duration = Duration::milliseconds(200);
const WARM: Duration = Duration::seconds(2);
const COLD: Duration = Duration::seconds(60);
const SCAN: Duration = Duration::seconds(3);
const PROBE: StdDuration = StdDuration::from_secs(30);
/// 등록부에 있는데 jsonl 이 아직 없는 세션을 보는 간격 (docs/CONTRACTS.md C6)
const AWAIT: Duration = Duration::seconds(1);
/// 첫 레코드가 이보다 최근이면 방금 시작된 세션이라 출근 연출을 낸다 (docs/CONTRACTS.md C6)
const JUST_STARTED: Duration = Duration::seconds(60);
/// 다른 호스트의 심장박동이 이만큼 멈추면 의심 (docs/CONTRACTS.md C5·C13)
const STALL_MS: i64 = 2000;
const TICK: StdDuration = StdDuration::from_millis(200);
const SLOW_TICK: StdDuration = StdDuration::from_millis(400);
/// 이만큼 연달아 틱이 200ms 를 넘기면 그 호스트만 늦춘다 (docs/CONTRACTS.md C13)
const SLOW_AFTER: u32 = 5;

/// 실행 중에는 벽시계, replay·테스트에서는 논리 시계 (docs/CONTRACTS.md C4)
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    /// 읽은 이벤트의 ts 를 알린다. 논리 시계만 쓴다
    fn observe(&self, _ts: DateTime<Utc>) {}
}

pub struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// 마지막으로 읽은 이벤트 ts + 그 뒤 벽시계 경과 × 배속 (docs/CONTRACTS.md C4).
/// 배속만 올리고 벽시계를 쓰면 결재 대기·졸음·종료 추정이 벽시계로 계산되어 결과가 달라진다
pub struct LogicalClock {
    speed: f64,
    anchor: Mutex<(DateTime<Utc>, Instant)>,
}

impl LogicalClock {
    /// `speed` 0 이면 이벤트를 읽을 때만 시계가 간다
    pub fn new(start: DateTime<Utc>, speed: f64) -> Self {
        assert!(speed.is_finite() && speed >= 0.0, "배속은 0 이상: {speed}");
        Self {
            speed,
            anchor: Mutex::new((start, Instant::now())),
        }
    }

    fn at(&self, anchor: (DateTime<Utc>, Instant)) -> DateTime<Utc> {
        let secs = anchor.1.elapsed().as_secs_f64() * self.speed;
        anchor.0 + Duration::microseconds((secs * 1e6) as i64)
    }
}

impl Clock for LogicalClock {
    fn now(&self) -> DateTime<Utc> {
        self.at(*self.anchor.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn observe(&self, ts: DateTime<Utc>) {
        let mut a = self.anchor.lock().unwrap_or_else(|e| e.into_inner());
        // 여러 파일·팀을 섞어 읽으면 옛 ts 가 늦게 온다. 시계는 거꾸로 가지 않는다
        if ts > self.at(*a) {
            *a = (ts, Instant::now());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Hot,
    Warm,
    Cold,
}

/// 프로세스 안 단조 시계(ms). 심장박동은 논리 시계로 재지 않는다. 배속이면 멀쩡한 호스트가 멈춘 것으로 보인다
pub fn mono_ms() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as i64
}

pub struct ProbeUpdate {
    pub sites: Vec<Site>,
    pub removed: Vec<(Vendor, PathBuf)>,
}

/// 다른 스레드가 이 호스트를 지켜보고 지시하는 값 (docs/CONTRACTS.md C5·C13). 멈춘 I/O 안의 스레드는 자기 지연을 잴 수 없다
pub struct Heartbeat {
    pub diagnostic: Mutex<Option<mpsc::Sender<HostDiagnostic>>>,
    /// `mono_ms` 로 잰 마지막 틱
    pub last_tick_ms: AtomicI64,
    /// 의심 상태. Probe 가 켜짐을 다시 확인할 때까지 UNC 를 건드리지 않는다 (docs/CONTRACTS.md C5).
    /// 바꿀 때는 `set_suspect` 로 world 목록에도 알린다
    // ponytail: 소등과 통신 지연을 이 깃발 하나로 둔다, 사이드바가 둘을 나눠 그리는 티켓에서 나눈다
    pub suspect: AtomicBool,
    /// Probe 가 넘긴 Site. WSL 은 이것을 받은 틱만 경로 접근(나열, verify_path, 열기·닫기)을 한다 (docs/CONTRACTS.md C5)
    pub probed: Mutex<Option<ProbeUpdate>>,
    /// 켜면 스레드가 다음 틱 전에 끝난다
    pub stop: AtomicBool,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self {
            diagnostic: Mutex::new(None),
            last_tick_ms: AtomicI64::new(mono_ms()),
            suspect: AtomicBool::new(false),
            probed: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }
}

/// 의심 깃발과 world 목록의 의심 표시를 함께 바꾼다. 바뀌었으면 true
fn set_suspect(world: &Mutex<OfficeWorld>, host: &str, hb: &Heartbeat, on: bool) -> bool {
    if hb.suspect.swap(on, Ordering::Relaxed) == on {
        return false;
    }
    world
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_suspect(host, on);
    true
}

/// Probe 가 부른다: (Windows Site, WSL 배포판). 테스트가 바꿔 끼운다
pub type ProbeFn = Box<dyn FnMut() -> (Vec<Site>, Vec<WslDistro>) + Send>;

/// 감시할 다른 호스트 (id, 심장박동). Probe 스레드가 넣고 Windows 스레드가 본다
type Remotes = Arc<Mutex<Vec<(String, Arc<Heartbeat>)>>>;

/// 세션 파일 하나와 그 파서
struct Tracked {
    key: TeamKey,
    /// None 이면 WSL 졸음·퇴근 팀이라 핸들을 닫아 두었다 (docs/CONTRACTS.md C5). 다음 Probe 직후 Scan 에서 바뀌었으면 다시 연다
    reader: Option<TailReader>,
    /// 핸들을 닫을 때의 (길이, mtime, 끝 4KB 해시)
    closed: Option<(u64, SystemTime, u64)>,
    closed_offset: u64,
    parser: Box<dyn Parser + Send>,
    last_read: DateTime<Utc>,
    /// 연속으로 Scan 에서 못 본 횟수
    missing: u8,
    /// 서브 파일 옆 meta.json 을 마지막 Scan 에서 봤나
    has_meta: bool,
    /// 이 파일에서 지금까지 읽은 줄의 최대 ts. 실행 중 병합의 정렬 키 (docs/CONTRACTS.md C4 순서)
    max_ts: Option<DateTime<Utc>>,
    /// 핸들을 닫았을 때의 세대. 다시 열 때 잇는다
    generation: u32,
    /// Reset 을 봤지만 아직 다시 못 만들었다. 다시 만들 때까지 이 파일만 읽지 않는다
    stale: bool,
}

impl Tracked {
    fn generation(&self) -> u32 {
        self.reader
            .as_ref()
            .map_or(self.generation, TailReader::generation)
    }
}

/// 미뤄 둔 재구성의 이유. 성공할 때까지 합쳐 들고 가고, 어느 길의 재구성이든 이것을 합쳐 쓴다. 잃으면 성공했을 때 rebuilt 표시(docs/CONTRACTS.md C4)나
/// Reset 파일의 세대 올림이 빠져, 팝업의 옛 커서가 같은 바이트 자리의 새 줄에 붙는다
#[derive(Debug, Default)]
struct Due {
    /// 조용히 다시 만들 이유(새 팀·Reset)가 있었다
    quiet: bool,
    /// Reset 을 본 파일
    reset: BTreeSet<PathBuf>,
}

pub struct HostLoop {
    pub runtime: Arc<Runtime>,
    tick_stats: TickStats,
    pub host: Host,
    pub sites: Vec<Site>,
    files: BTreeMap<PathBuf, Tracked>,
    /// 아직 리더가 없는 새 서브도 실패를 보존해야 정상 파일 하나의 복구로 경고가 사라지지 않는다
    read_failures: BTreeMap<PathBuf, TeamKey>,
    world: Arc<Mutex<OfficeWorld>>,
    clock: Arc<dyn Clock>,
    pub heartbeat: Arc<Heartbeat>,
    diag: Arc<Mutex<Diag>>,
    last_scan: Option<DateTime<Utc>>,
    /// 다시 만들어야 하는데 아직 못 만든 팀. 경로 접근이 되는 다음 Scan 에서 만든다. 그동안 기존 리더는 계속 읽는다
    rebuild_due: BTreeMap<TeamKey, Due>,
    /// 파일이 없어져 퇴장시킨 직원과 그 시각. 재구성이 메인의 Spawn 으로 되살리면 다시 퇴장시킨다
    gone: BTreeMap<TeamKey, BTreeMap<String, DateTime<Utc>>>,
    /// 다 읽어 보니 팀이 되지 않는 파일 (Codex 자식·내부 rollout). Scan 마다 처음부터 다시 읽지 않는다.
    /// 나열에서 사라지면 잊는다
    // ponytail: 자식 rollout 의 활동은 버린다, T33 에서 부모 팀에 붙인다
    side: BTreeSet<PathBuf>,
    /// Windows 스레드만: 감시할 다른 호스트
    remotes: Remotes,
    /// Claude 등록부에 살아 있는 세션(id → 항목)과 Windows에서 jsonl 이 아직 없는 세션 (docs/CONTRACTS.md C6)
    alive: HashMap<String, registry::RegistryEntry>,
    registry_roots: BTreeMap<PathBuf, HashMap<String, registry::RegistryEntry>>,
    #[cfg(test)]
    proc_root_override: Option<PathBuf>,
    awaited: BTreeSet<String>,
    last_await: Option<DateTime<Utc>>,
    project_dirs: Vec<PathBuf>,
    /// 경로로 한 접근(나열, 열기, verify_path, stat) 수. WSL 에서는 Probe 직후 틱 밖에서 늘면 안 된다
    pub path_ops: u64,
    /// 로딩 화면(docs/CONTRACTS.md C4)에 첫 Scan 의 나열·읽기 진행을 알린다. 첫 Scan 이 끝나면 비운다
    pub watch: Option<Arc<ScanWatch>>,
}

impl HostLoop {
    pub fn new(
        host: Host,
        sites: Vec<Site>,
        world: Arc<Mutex<OfficeWorld>>,
        clock: Arc<dyn Clock>,
        diag: Arc<Mutex<Diag>>,
    ) -> Self {
        world
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hosts
            .insert(host.id.clone(), host.clone());
        Self {
            runtime: Arc::new(Runtime::default()),
            tick_stats: TickStats::default(),
            host,
            sites,
            files: BTreeMap::new(),
            read_failures: BTreeMap::new(),
            world,
            clock,
            heartbeat: Arc::new(Heartbeat::default()),
            diag,
            last_scan: None,
            rebuild_due: BTreeMap::new(),
            gone: BTreeMap::new(),
            side: BTreeSet::new(),
            remotes: Arc::new(Mutex::new(Vec::new())),
            alive: HashMap::new(),
            registry_roots: BTreeMap::new(),
            #[cfg(test)]
            proc_root_override: None,
            awaited: BTreeSet::new(),
            last_await: None,
            project_dirs: Vec::new(),
            path_ops: 0,
            watch: None,
        }
    }

    /// 다른 호스트의 심장박동을 매 틱 본다 (docs/CONTRACTS.md C5·C13)
    pub fn watch(&mut self, host_id: String, heartbeat: Arc<Heartbeat>) {
        self.remotes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((host_id, heartbeat));
    }

    fn is_wsl(&self) -> bool {
        matches!(self.host.kind, HostKind::Wsl { .. })
    }

    fn suspect(&self) -> bool {
        self.heartbeat.suspect.load(Ordering::Relaxed)
    }

    fn mark_suspect(&self) {
        if set_suspect(&self.world, &self.host.id, &self.heartbeat, true) {
            crate::oplog!(
                "{} 의심: Probe 가 켜짐을 확인할 때까지 UNC 를 멈춘다",
                self.host.id
            );
        }
    }

    fn lock_world(&self) -> std::sync::MutexGuard<'_, OfficeWorld> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 한 틱. 읽기·파싱은 잠금 밖에서, 적용만 잠금 안에서 한다 (docs/CONTRACTS.md C4)
    pub fn tick(&mut self) {
        self.reply_diagnostic();
        let now = self.clock.now();
        self.heartbeat
            .last_tick_ms
            .store(mono_ms(), Ordering::Relaxed);
        self.watch_remotes();
        if self.suspect() {
            return;
        }
        let probed = self
            .heartbeat
            .probed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // WSL 은 경로 접근이 Probe 직후에만 된다. 나열도 경로 기반이라 Scan 도 그때만 돈다 (docs/CONTRACTS.md C5)
        let fresh = probed.is_some();
        let path_ok = !self.is_wsl() || fresh;
        if let Some(update) = probed {
            self.set_sites(update.sites);
            self.remove_manual_roots(update.removed);
        }
        let scan = if self.is_wsl() {
            path_ok
        } else {
            fresh || self.awaited_appeared(now) || self.last_scan.is_none_or(|t| now - t >= SCAN)
        };
        if scan {
            self.scan(now);
            self.last_scan = Some(now);
            let due: Vec<TeamKey> = self.rebuild_due.keys().cloned().collect();
            for key in due {
                if self.suspect() {
                    break;
                }
                self.try_rebuild(&key, now, &BTreeSet::new(), &BTreeSet::new());
            }
            // 나열이 막혀 일찍 끝나도 알린다. 안 알리면 로딩 화면이 이 호스트를 끝없이 기다린다
            if let Some(w) = self.watch.take() {
                w.finished(&self.host.id);
            }
        }
        self.poll_teams(now, path_ok);
        if self.is_wsl() && path_ok && !self.suspect() {
            self.close_idle(now);
        }
        if self.suspect() {
            return;
        }
        {
            let mut w = self.lock_world();
            w.tick(now, &self.host.id);
            w.drain_unhandled(&self.diag, now);
        }
        let names = self.lock_world().unsaved_names();
        if let Some(e) = names.and_then(|n| n.write_report()) {
            crate::oplog!("이름 저장 실패: {e}");
        }
        if let Err(e) = self
            .diag
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush_if_due(now)
        {
            crate::oplog!("미대응 로그 쓰기 실패: {e}");
        }
    }

    fn reply_diagnostic(&self) {
        let reply = self
            .heartbeat
            .diagnostic
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(reply) = reply else {
            return;
        };
        let files = self
            .files
            .iter()
            .map(|(path, tracked)| FileDiagnostic {
                team: tracked.key.clone(),
                path: path.clone(),
                offset: tracked
                    .reader
                    .as_ref()
                    .map_or(tracked.closed_offset, TailReader::offset),
                generation: tracked.generation(),
            })
            .collect();
        let _ = reply.send(HostDiagnostic {
            host: self.host.clone(),
            sites: self.sites.clone(),
            files,
            ticks: self.tick_stats.clone(),
            path_ops: self.path_ops,
            suspect: self.suspect(),
        });
    }

    fn remove_manual_roots(&mut self, removed: Vec<(Vendor, PathBuf)>) {
        if removed.is_empty() {
            return;
        }
        let removed_path = |vendor: Vendor, path: &Path| {
            !self.sites.iter().any(|site| {
                site.vendor == vendor
                    && crate::runtime::contains_root(&self.host, vendor, &site.root, path)
            }) && removed.iter().any(|(gone_vendor, root)| {
                *gone_vendor == vendor
                    && crate::runtime::contains_root(&self.host, vendor, root, path)
            })
        };
        let mut world = self.lock_world();
        let gone: Vec<_> = world
            .teams
            .iter()
            .filter(|(key, team)| {
                key.host == self.host.id
                    && !team.sources.is_empty()
                    && team
                        .sources
                        .iter()
                        .all(|path| removed_path(key.vendor, path))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &gone {
            world.remove_team(key);
        }
        drop(world);
        self.files
            .retain(|path, tracked| !removed_path(tracked.key.vendor, path));
        let failed: BTreeSet<_> = self.read_failures.values().cloned().collect();
        self.read_failures
            .retain(|path, key| !removed_path(key.vendor, path));
        for key in failed {
            self.sync_read_failed(&key);
        }
        self.registry_roots.retain(|home, _| {
            self.sites.iter().any(|site| {
                site.vendor == Vendor::Claude
                    && site.root.parent().is_some_and(|parent| {
                        crate::runtime::contains_root(&self.host, site.vendor, parent, home)
                            && crate::runtime::contains_root(&self.host, site.vendor, home, parent)
                    })
            }) || !removed.iter().any(|(vendor, root)| {
                *vendor == Vendor::Claude
                    && root.parent().is_some_and(|parent| {
                        crate::runtime::contains_root(&self.host, *vendor, parent, home)
                            && crate::runtime::contains_root(&self.host, *vendor, home, parent)
                    })
            })
        });
        for key in gone {
            self.rebuild_due.remove(&key);
            self.gone.remove(&key);
        }
    }

    /// Probe 가 넘긴 Site 로 바꾼다. 같은 배포판의 루트가 `\\wsl.localhost` 와 `\\wsl$` 를 오가면
    /// 옛 루트의 리더가 남아 같은 줄이 두 번 적용되므로, 없어진 루트 아래 리더를 버리고 새 루트에서 다시 찾는다
    fn set_sites(&mut self, sites: Vec<Site>) {
        let roots: Vec<&PathBuf> = sites.iter().map(|s| &s.root).collect();
        self.files
            .retain(|p, _| roots.iter().any(|r| p.starts_with(r)));
        self.side.retain(|p| roots.iter().any(|r| p.starts_with(r)));
        self.sites = sites;
    }

    /// 선택 사무실의 팀은 Hot, 퇴근 팀은 Cold, 나머지 Warm (docs/CONTRACTS.md C6).
    /// 선택이 퇴근보다 앞선다. 보고 있는 퇴근 팀이 다시 일하면 60초가 아니라 바로 보여야 한다
    fn tiers(&self) -> HashMap<TeamKey, (Tier, TeamPhase)> {
        let w = self.lock_world();
        w.teams
            .iter()
            .filter(|(k, _)| k.host == self.host.id)
            .map(|(k, t)| {
                let tier = if w.selected.as_ref() == Some(&w.office_of(k)) {
                    Tier::Hot
                } else if t.phase == TeamPhase::Off {
                    Tier::Cold
                } else {
                    Tier::Warm
                };
                (k.clone(), (tier, t.phase))
            })
            .collect()
    }

    /// 때가 된 팀의 파일을 팀 단위로 읽는다. 한 팀의 여러 파일은 한 번에 병합해 적용한다 (docs/CONTRACTS.md C4 순서)
    fn poll_teams(&mut self, now: DateTime<Utc>, path_ok: bool) {
        let tiers = self.tiers();
        let mut teams: BTreeMap<TeamKey, Vec<PathBuf>> = BTreeMap::new();
        for (p, t) in &self.files {
            if t.reader.is_some() && !t.stale {
                teams.entry(t.key.clone()).or_default().push(p.clone());
            }
        }
        for (key, mut paths) in teams {
            if self.suspect() {
                break;
            }
            let (tier, _) = tiers
                .get(&key)
                .copied()
                .unwrap_or((Tier::Warm, TeamPhase::Working));
            let every = match tier {
                Tier::Hot => HOT,
                Tier::Warm => WARM,
                Tier::Cold => COLD,
            };
            if !paths.iter().any(|p| now - self.files[p].last_read >= every) {
                continue;
            }
            // 퇴근 팀은 매 Scan 대신 Cold 폴링 때 바꿔치기를 본다 (docs/CONTRACTS.md C5, 실제 기록 수백 개에 매 3초 열기를 피한다)
            if tier == Tier::Cold && !self.is_wsl() {
                for p in &paths {
                    self.path_ops += 1;
                    if let Some(r) = self.files.get_mut(p).and_then(|t| t.reader.as_mut()) {
                        let _ = r.verify_path();
                    }
                }
            }
            paths.sort_by_key(|p| (is_subagent(p), p.clone()));
            self.poll_team(&key, &paths, now, path_ok);
        }
    }

    /// 한 팀의 파일들을 읽어 docs/CONTRACTS.md C4 순서로 합쳐 한 번 적용하고, 적용이 끝난 뒤에만 모두 commit 한다 (docs/CONTRACTS.md C6)
    fn poll_team(&mut self, key: &TeamKey, paths: &[PathBuf], now: DateTime<Utc>, path_ok: bool) {
        let mut batch: Vec<(PathBuf, Pending)> = Vec::new();
        let mut reset = BTreeSet::new();
        let wsl = self.is_wsl();
        // 삭제 대기 파일은 옛 핸들만 읽힐 수 있다. 재구성 실패는 다시 열기에 성공해야 해제한다
        let rebuilding = self.rebuild_due.contains_key(key);
        for p in paths {
            let Some(t) = self.files.get_mut(p) else {
                continue;
            };
            t.last_read = now;
            let Some(r) = t.reader.as_mut() else {
                continue;
            };
            let read = match r.changed() {
                Ok(true) => r.read_new_lines(MAX_READ_BYTES).map(Some),
                Ok(false) => Ok(None),
                Err(e) => Err(e),
            };
            match read {
                Ok(Some(pending)) if pending.reset => {
                    reset.insert(p.clone());
                }
                Ok(Some(pending)) => {
                    if pending.dropped_lines > 0 {
                        crate::oplog!("64MB 넘는 줄 버림 {}", p.display());
                    }
                    if !rebuilding {
                        self.read_failures.remove(p);
                    }
                    batch.push((p.clone(), pending));
                }
                Ok(None) => {
                    if !rebuilding {
                        self.read_failures.remove(p);
                    }
                }
                Err(e) => {
                    if wsl {
                        self.on_wsl_error(key, p, &e, now, path_ok, &reset);
                        return;
                    }
                    // 로컬 파일 하나의 실패로 팀 전체를 멈추지 않는다. 다음 폴링에 다시 본다
                    if self.read_failures.insert(p.clone(), key.clone()).is_none() {
                        crate::oplog!("읽기 실패 {}: {e}", p.display());
                    }
                }
            }
        }
        self.sync_read_failed(key);
        // 멈췄던 I/O 의 결과는 버리고 commit 하지 않는다. 재개한 첫 틱이 같은 구간을 다시 읽는다 (docs/CONTRACTS.md C5·C13).
        // 파서에 넣기 전에 본다. 넣고 버리면 파서가 같은 줄을 두 번 본다. 이미 본 Reset 은 재구성에 싣는다
        if self.suspect() {
            if !reset.is_empty() {
                self.defer_rebuild(key, true, &reset);
            }
            return;
        }
        if !reset.is_empty() {
            // 어느 파일의 Reset 이든 팀 전체를 다시 만든다 (docs/CONTRACTS.md C4). 다른 파일의 이번 읽기는 재구성이 다시 읽는다
            for p in &reset {
                crate::oplog!("Reset {}", p.display());
            }
            if path_ok {
                self.try_rebuild(key, now, &BTreeSet::new(), &reset);
                return;
            }
            for p in &reset {
                if let Some(t) = self.files.get_mut(p) {
                    t.stale = true;
                }
            }
            self.defer_rebuild(key, true, &reset);
        }
        if batch.is_empty() {
            return;
        }
        let mut per_file = Vec::new();
        let mut unhandled = Vec::new();
        for (p, pending) in &batch {
            let Some(t) = self.files.get_mut(p) else {
                continue;
            };
            let generation = u64::from(t.generation());
            let mut events = Vec::new();
            for line in &pending.lines {
                let raw = RawRef {
                    path: p.clone(),
                    generation,
                    byte_offset: line.offset,
                    line_no: line.line_no,
                };
                events.extend(t.parser.parse_line(&line.bytes, &raw, &mut unhandled));
            }
            per_file.push((t.max_ts, events));
        }
        let maxes: Vec<Option<DateTime<Utc>>> = per_file
            .iter()
            .map(|(m, evs)| (*m).max(evs.iter().filter_map(|e| e.ts).max()))
            .collect();
        let events = merge_live(per_file);
        let product = {
            let mut w = self.lock_world();
            if !events.is_empty() {
                w.apply(key, &events, now);
            }
            w.drain_unhandled(&self.diag, now);
            w.teams.get(key).map(|t| t.product)
        };
        for ((p, pending), max) in batch.into_iter().zip(maxes) {
            if let Some(t) = self.files.get_mut(&p) {
                t.max_ts = max;
                if let Some(r) = t.reader.as_mut() {
                    r.commit(pending);
                }
            }
        }
        if let Some(ts) = events.iter().filter_map(|e| e.ts).max() {
            self.clock.observe(ts);
        }
        let product = product.unwrap_or_else(|| default_product(key.vendor));
        self.observe(product, &events, &unhandled, now);
        if path_ok {
            self.register_subs(key, &events, now);
        }
    }

    /// WSL 핸들 에러. Probe 직후 틱이면 배포판이 켜져 있으니 배포판 재시작으로 죽은 핸들로 보고 경로로 다시 연다.
    /// 아니면 꺼졌거나 공유가 끊긴 것이라 의심으로 두고, Probe 가 켜짐을 본 뒤 다시 만든다 (docs/CONTRACTS.md C5).
    /// `reset` 은 같은 폴링에서 앞 파일들이 본 Reset 이다
    fn on_wsl_error(
        &mut self,
        key: &TeamKey,
        p: &Path,
        e: &io::Error,
        now: DateTime<Utc>,
        path_ok: bool,
        reset: &BTreeSet<PathBuf>,
    ) {
        crate::oplog!("읽기 실패 {}: {e}", p.display());
        if path_ok {
            self.try_rebuild(key, now, &BTreeSet::new(), reset);
            return;
        }
        self.mark_suspect();
        self.defer_rebuild(key, false, reset);
    }

    /// Probe 직후 틱에 근무 중이 아닌 WSL 팀의 핸들을 닫는다 (docs/CONTRACTS.md C5). 열어 둔 핸들은 배포판이 스스로 꺼지지 못하게 하고,
    /// 꺼진 뒤 그 핸들을 만지거나 닫으면 다시 켠다. 켜짐을 확인한 지금 닫아야 안전하다
    fn close_idle(&mut self, now: DateTime<Utc>) {
        let tiers = self.tiers();
        let mut idle: BTreeMap<TeamKey, Vec<PathBuf>> = BTreeMap::new();
        for (p, t) in &self.files {
            let working = tiers
                .get(&t.key)
                .is_none_or(|(_, phase)| *phase == TeamPhase::Working);
            if t.reader.is_some() && !working {
                idle.entry(t.key.clone()).or_default().push(p.clone());
            }
        }
        for (key, mut paths) in idle {
            if self.suspect() {
                return;
            }
            paths.sort_by_key(|p| (is_subagent(p), p.clone()));
            // 닫기 전에 남은 줄을 다 읽는다. 닫을 때의 길이를 기준으로 다시 열지 정한다
            self.poll_team(&key, &paths, now, true);
            // 방금 읽은 줄로 다시 일하기 시작했으면 쥔다. 닫으면 다음 Probe 까지 못 읽는다
            let working = self
                .lock_world()
                .teams
                .get(&key)
                .is_none_or(|t| t.phase == TeamPhase::Working);
            if working {
                continue;
            }
            for p in &paths {
                // poll_team 이 의심을 켰으면 경로를 더 건드리지 않는다 (docs/CONTRACTS.md C5)
                if self.suspect() {
                    return;
                }
                self.path_ops += 1;
                let Ok(m) = fs::metadata(p) else {
                    continue;
                };
                let Some(t) = self.files.get_mut(p) else {
                    continue;
                };
                // 덜 읽은 채(8MB 상한, 읽은 뒤 붙은 줄) 닫으면 길이가 다시 바뀔 때까지 남은 줄을 못 읽는다. 다음 틱에 이어 읽게 둔다
                if t.stale || !t.reader.as_ref().is_some_and(|r| r.caught_up(m.len())) {
                    continue;
                }
                // 같은 길이·더 긴 다른 내용으로 교체된 것을 다시 볼 때 가리려고 끝 4KB 를 기억한다 (docs/CONTRACTS.md C9).
                // 아직 쥔 핸들로 읽어 경로를 새로 건드리지 않는다. 못 읽으면 닫지 않고 다음 Probe 에 다시 본다
                let w = m.len().min(TAIL_HASH_BYTES as u64);
                let Some(Ok(tail)) = t
                    .reader
                    .as_ref()
                    .map(|r| read_range(r.file(), m.len() - w, w))
                else {
                    continue;
                };
                t.generation = t.generation();
                t.closed_offset = t
                    .reader
                    .as_ref()
                    .map_or(t.closed_offset, TailReader::offset);
                t.closed = Some((
                    m.len(),
                    m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    hash_bytes(&tail),
                ));
                t.reader = None;
            }
        }
    }

    /// Spawn 결과로 안 서브 파일은 Scan 을 기다리지 않고 바로 읽는다. S19 가 끊기지 않게 (docs/CONTRACTS.md C6).
    /// 서브 파일에는 Spawn 보다 이른 줄이 있어 이어 읽지 않고 팀을 다시 만든다 (docs/CONTRACTS.md C4).
    /// 아직 없으면 Scan 이 찾는다
    fn register_subs(&mut self, key: &TeamKey, events: &[Event], now: DateTime<Utc>) {
        if key.vendor != Vendor::Claude {
            return;
        }
        let mut new = BTreeSet::new();
        for ev in events {
            let EventKind::SubagentSpawn { child_id, .. } = &ev.kind else {
                continue;
            };
            // 파일 내용에서 온 값이라 경로 조각으로 쓰기 전에 거른다
            if !safe_id(child_id) {
                continue;
            }
            let path = &ev.raw_ref.path;
            let dir = if is_subagent(path) {
                path.parent().and_then(Path::parent).map(Path::to_path_buf)
            } else {
                Some(path.with_extension(""))
            };
            let Some(dir) = dir else {
                continue;
            };
            let sub = dir
                .join("subagents")
                .join(format!("agent-{child_id}.jsonl"));
            self.path_ops += 1;
            if !self.files.contains_key(&sub) && sub.is_file() {
                self.revive(key, &sub);
                new.insert(sub);
            }
        }
        if !new.is_empty() {
            self.try_rebuild(key, now, &new, &BTreeSet::new());
        }
    }

    fn observe(
        &self,
        product: Product,
        events: &[Event],
        unhandled: &[Unhandled],
        now: DateTime<Utc>,
    ) {
        let mut d = self.diag.lock().unwrap_or_else(|e| e.into_inner());
        for u in unhandled {
            d.observe_unhandled(product, u, now);
        }
        for ev in events {
            d.observe_event(ev, now);
        }
    }

    fn defer_rebuild(&mut self, key: &TeamKey, quiet: bool, reset: &BTreeSet<PathBuf>) {
        let due = self.rebuild_due.entry(key.clone()).or_default();
        due.quiet |= quiet;
        due.reset.extend(reset.iter().cloned());
    }

    /// 실패하면 기존 상태를 그대로 두고 다음 Scan 에 다시 한다
    fn try_rebuild(
        &mut self,
        key: &TeamKey,
        now: DateTime<Utc>,
        new: &BTreeSet<PathBuf>,
        reset: &BTreeSet<PathBuf>,
    ) {
        let due = self.rebuild_due.remove(key).unwrap_or_default();
        let quiet = due.quiet;
        let reset: BTreeSet<PathBuf> = due.reset.into_iter().chain(reset.iter().cloned()).collect();
        if let Err(e) = self.rebuild_team(key, now, new, &reset, quiet) {
            crate::oplog_once!("팀 재구성 실패 {}: {e}", key.session);
            if self.is_wsl() {
                self.mark_suspect();
            }
            // 새 팀이면 파일이 아직 등록 전이라 다음 Scan 이 새 파일로 다시 찾는다
            if self.files.values().any(|t| &t.key == key) {
                self.defer_rebuild(key, quiet || !reset.is_empty(), &reset);
            }
        }
        self.sync_read_failed(key);
    }

    fn sync_read_failed(&self, key: &TeamKey) {
        let failed = self.read_failures.values().any(|k| k == key);
        self.lock_world().set_read_failed(key, failed);
    }

    /// 팀의 파일을 모두 처음부터 다시 읽어 만든다 (새 세션, Reset. docs/CONTRACTS.md C4).
    /// `new` 는 이번에 처음 찾은 파일, `reset` 은 Reset 을 본 파일이다 (세대를 하나 올린다).
    /// 기존 팀의 파일을 못 읽으면 기록을 잃지 않도록 교체를 보류한다. 새 팀은 읽힌 서브만으로 만들고 실패는 보존한다.
    /// 방금 시작된 새 파일이면 출근 cue 를 내고 미대응 재료를 센다. 늦게 발견한 옛 파일은 세지 않는다 (재시작마다 다시 세지 않게)
    fn rebuild_team(
        &mut self,
        key: &TeamKey,
        now: DateTime<Utc>,
        new: &BTreeSet<PathBuf>,
        reset: &BTreeSet<PathBuf>,
        carried_quiet: bool,
    ) -> io::Result<()> {
        let mut paths: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, t)| &t.key == key)
            .map(|(p, _)| p.clone())
            .chain(
                self.read_failures
                    .iter()
                    .filter(|(_, k)| *k == key)
                    .map(|(p, _)| p.clone()),
            )
            .chain(new.iter().cloned())
            .collect();
        // 메인 파일이 먼저다 (docs/CONTRACTS.md C4 순서)
        paths.sort_by_key(|p| (is_subagent(p), p.clone()));
        paths.dedup();
        if paths.is_empty() {
            return Ok(());
        }
        let exists = self.lock_world().teams.contains_key(key);
        // 하나라도 실패하면 기존 상태를 건드리지 않도록 새 리더는 끝에 한 번에 바꾼다
        let mut fresh = Vec::new();
        let mut unseen = Vec::new();
        let mut files = Vec::new();
        let mut subs = Vec::new();
        let mut arrived = Vec::new();
        let (mut heard, mut heard_unhandled) = (Vec::new(), Vec::new());
        let mut last_ts = None;
        for path in &paths {
            if self.suspect() {
                return Err(io::Error::other("의심 중이라 경로 접근을 멈췄다"));
            }
            let old = self.files.get(path);
            let generation = old.map_or(0, |o| o.generation() + u32::from(reset.contains(path)));
            self.path_ops += 1;
            let read = open(path, key.clone(), now, generation).and_then(|mut t| {
                let mut unhandled = Vec::new();
                let events = read_all(&mut t, path, &mut unhandled)?;
                Ok((t, events, unhandled))
            });
            let (mut t, events, unhandled) = match read {
                Ok(r) => {
                    self.read_failures.remove(path);
                    r
                }
                Err(e) => {
                    if !self.is_wsl() {
                        self.read_failures.insert(path.clone(), key.clone());
                        if !exists && is_subagent(path) {
                            crate::oplog_once!(
                                "서브 파일 열기 실패, 빼고 재구성 {}: {e}",
                                path.display()
                            );
                            unseen.push(path.clone());
                            continue;
                        }
                    }
                    return Err(e);
                }
            };
            // 이미 팀에 든 파일은 Reset 으로 내용이 바뀌어도 빼지 않는다. 새로 찾은 파일만 가린다
            if !self.files.contains_key(path) && t.parser.side() {
                self.side.insert(path.clone());
                continue;
            }
            if let Some(o) = self.files.get(path) {
                t.missing = o.missing;
            }
            self.path_ops += 1;
            t.has_meta = is_subagent(path) && path.with_extension("meta.json").is_file();
            t.max_ts = events.iter().filter_map(|e| e.ts).max();
            last_ts = last_ts.max(t.max_ts);
            if !self.files.contains_key(path) && self.just_started(path, &events, now) {
                arrived.push(path.clone());
                heard.extend(events.iter().cloned());
                heard_unhandled.extend(unhandled);
            }
            if is_subagent(path) {
                subs.extend(sub_meta(path));
            }
            files.push(events);
            fresh.push((path.clone(), t));
        }
        // 시각 있는 레코드가 하나도 없으면 아직 시작 전이다. 지금 만들면 시작 시각이 1970년이 되고
        // 무재해·피자가 그때부터 센다. 새 팀은 등록하지 않아 다음 Scan 이 다시 찾고,
        // 기존 팀은 리더만 바꿔(Reset 반복을 막는다) 상태는 그대로 두고 다음 append 부터 적용한다
        if last_ts.is_none() {
            if exists {
                self.install(fresh, &unseen);
            }
            return Ok(());
        }
        let local = self.lock_world().local;
        let mut team = rebuild(
            &key.session,
            default_product(key.vendor),
            &subs,
            files,
            local,
        );
        team.alive_in_registry = self.alive.contains_key(&key.session);
        if let Some(r) = self.alive.get(&key.session) {
            // 재구성은 조용하다 (docs/CONTRACTS.md C4). 제목이 바뀌어도 액자 연출은 내지 않는다
            team.set_registry(r.user_name(), r.product(), &mut Vec::new());
        }
        let product = team.product;
        let boss = team.employees[0].id.clone();
        let gone = self.gone.get(key).cloned().unwrap_or_default();
        let arrived: Vec<String> = arrived
            .iter()
            .map(|path| {
                if is_subagent(path) {
                    agent_of(path).unwrap_or_default()
                } else {
                    boss.clone()
                }
            })
            .collect();
        {
            let mut w = self.lock_world();
            // Spawn 을 먼저 적용해 이미 출근한 서브는 다시 출근하지 않는다
            let known: HashSet<String> = w
                .teams
                .get(key)
                .map(|t| t.employees.iter().map(|e| e.id.clone()).collect())
                .unwrap_or_default();
            // 조용한 재구성은 새 세션과 Reset 뿐이다 (docs/CONTRACTS.md C4). 서브 파일이 새로 보인 것은 진행 중 연출을 끊지 않는다.
            // 미뤄졌다 성공한 재구성은 미뤄질 때의 이유(carried_quiet)도 함께 본다
            w.put_team(
                key.clone(),
                team,
                now,
                !exists || !reset.is_empty() || carried_quiet,
            );
            // 파일이 없어 퇴장시킨 직원을 메인의 Spawn 이 되살렸으면 조용히 다시 퇴장시킨다
            let quiet_from = w.outbox.len();
            for (agent, at) in &gone {
                w.retire_employee(key, agent, *at);
            }
            w.outbox.truncate(quiet_from);
            // 출근 연출은 같은 잠금에서 넣는다. 번호만 먼저 나가지 않게
            for agent in arrived {
                if !known.contains(&agent) {
                    w.cue(key, Cue::Enter { agent });
                }
            }
        }
        self.install(fresh, &unseen);
        if let Some(ts) = last_ts {
            self.clock.observe(ts);
        }
        // ponytail: 서브 파일이 새로 보여 다시 만들 때 그 틱에 처음 읽은 줄의 state_conflict·scenario_gap 은
        // 재구성 cue 와 함께 버려져 덜 센다, 미대응 로그에서 서브 등장 직후 전이가 빠져 보이면 재구성 전 적용분과 나눠 센다
        self.observe(product, &heard, &heard_unhandled, now);
        Ok(())
    }

    /// 재구성으로 새로 연 리더를 넣고, 안 열린 서브 파일은 빼서 다음 Scan 이 새 파일로 다시 찾게 한다
    fn install(&mut self, fresh: Vec<(PathBuf, Tracked)>, unseen: &[PathBuf]) {
        for p in unseen {
            self.files.remove(p);
        }
        self.files.extend(fresh);
    }

    /// 첫 레코드 ts(없으면 파일 생성 시각)가 지금부터 60초 안인가 (docs/CONTRACTS.md C6)
    fn just_started(&mut self, path: &Path, events: &[Event], now: DateTime<Utc>) -> bool {
        let first = events.iter().find_map(|e| e.ts).or_else(|| {
            self.path_ops += 1;
            let created = fs::metadata(path).ok()?.created().ok()?;
            Some(DateTime::<Utc>::from(created))
        });
        first.is_some_and(|t| now - t < JUST_STARTED)
    }

    /// 새 세션 파일·서브 파일·meta.json 과 사라진 파일을 찾는다 (docs/CONTRACTS.md C6).
    /// 접근에 실패한 Site 의 파일은 "없음"이 아니라 "못 봄"이라 아무것도 지우지 않는다.
    /// 경로 접근마다 앞에서 의심을 보고 멈춘다
    fn scan(&mut self, now: DateTime<Utc>) {
        self.scan_listing(now, list_site);
    }

    fn scan_listing(
        &mut self,
        now: DateTime<Utc>,
        mut list: impl FnMut(&Site, &mut u64) -> io::Result<Listing>,
    ) {
        self.read_registry();
        let mut seen = HashSet::new();
        let mut metas = HashSet::new();
        let mut ok_roots = Vec::new();
        let mut failed_subtrees = Vec::new();
        let mut project_dirs = Vec::new();
        let mut found: BTreeMap<TeamKey, BTreeSet<PathBuf>> = BTreeMap::new();
        for site in self.sites.clone() {
            if self.suspect() {
                return;
            }
            let listing = match list(&site, &mut self.path_ops) {
                Ok(l) => l,
                Err(e) => {
                    crate::oplog_once!("세션 폴더 나열 실패 {}: {e}", site.root.display());
                    if self.is_wsl() {
                        // Probe 직후인데 나열이 안 된다. 배포판이 꺼졌거나 공유가 끊겼다
                        self.mark_suspect();
                        return;
                    }
                    continue;
                }
            };
            ok_roots.push(site.root.clone());
            failed_subtrees.extend(listing.failed_subtrees);
            metas.extend(listing.metas);
            project_dirs.extend(listing.projects);
            for (path, session) in listing.files {
                if !self.files.contains_key(&path) && !self.side.contains(&path) {
                    let key = TeamKey {
                        host: self.host.id.clone(),
                        vendor: site.vendor,
                        session,
                    };
                    found.entry(key).or_default().insert(path.clone());
                }
                seen.insert(path);
            }
        }
        self.project_dirs = project_dirs;
        let observed = |path: &Path| {
            ok_roots.iter().any(|r| path.starts_with(r))
                && !failed_subtrees.iter().any(|r| path.starts_with(r))
        };
        // 나열에 실패한 Site 의 파일은 "없음"이 아니라 "못 봄"이라 잊지 않는다
        self.side.retain(|p| seen.contains(p) || !observed(p));
        if let Some(w) = self.watch.clone() {
            // 팀마다 메인 파일 하나. 서브만 있는 팀은 아무 파일이나
            // 수정 시각도 여기서 함께 모은다: 경로는 이 스레드만 건드려야 의심·Probe 가드가 먹는다
            let mtimes = found
                .values()
                .filter_map(|ps| ps.iter().find(|p| !is_subagent(p)).or(ps.first()))
                .filter_map(|p| {
                    self.path_ops += 1;
                    fs::metadata(p).and_then(|m| m.modified()).ok()
                })
                .collect();
            w.listed(&self.host.id, seen.len(), mtimes);
        }

        let tiers = self.tiers();
        let mut gone_teams = BTreeSet::new();
        let mut gone_subs = Vec::new();
        let mut meta_gone = Vec::new();
        // 다시 만들 팀과 그 팀에서 Reset 으로 볼 파일
        let mut rebuild: BTreeMap<TeamKey, BTreeSet<PathBuf>> = BTreeMap::new();
        let mut verify = Vec::new();
        for (path, t) in &mut self.files {
            if !observed(path) {
                continue;
            }
            if !seen.contains(path) {
                t.missing = t.missing.saturating_add(1);
                if is_subagent(path) {
                    gone_subs.push(path.clone());
                } else if t.missing >= 2 {
                    // 한 번만 없는 것은 보관 중 이동일 수 있어 기다린다
                    gone_teams.insert(t.key.clone());
                }
                continue;
            }
            t.missing = 0;
            if is_subagent(path) {
                let has = metas.contains(&path.with_extension("meta.json"));
                if has && !t.has_meta {
                    rebuild.entry(t.key.clone()).or_default();
                } else if !has && t.has_meta {
                    meta_gone.push(path.clone());
                }
                t.has_meta = has;
            }
            let phase = tiers.get(&t.key).map_or(TeamPhase::Working, |x| x.1);
            // 퇴근 팀은 Cold 폴링 때 본다 (D3). 닫아 둔 파일은 다시 열지 정한다
            if (t.reader.is_some() && phase != TeamPhase::Off) || t.closed.is_some() {
                verify.push(path.clone());
            }
        }
        // 로컬은 매 Scan, WSL 은 Probe 직후 Scan 에 근무 중·졸음 팀 파일만 경로의 파일과 핸들의 파일을 대조한다 (docs/CONTRACTS.md C5).
        // 핸들을 닫아 둔 파일은 닫을 때의 길이·mtime 과 비교해 바뀌었으면 다시 연다
        for path in verify {
            if self.suspect() {
                return;
            }
            self.path_ops += 1;
            let wsl = self.is_wsl();
            let Some(t) = self.files.get_mut(&path) else {
                continue;
            };
            let r = match (&mut t.reader, t.closed) {
                (Some(r), _) => r.verify_path(),
                (None, Some((len, mtime, tail))) => fs::metadata(&path).and_then(|m| {
                    if (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)) == (len, mtime) {
                        return Ok(());
                    }
                    // 줄었거나, 닫을 때의 끝 4KB 가 달라졌으면 다른 내용으로 교체된 것이라 Reset 이다 (docs/CONTRACTS.md C6).
                    // 세대를 올려야 옛 커서가 새 내용의 같은 바이트 자리에 붙지 않는다 (C9)
                    let replaced = m.len() < len || {
                        self.path_ops += 1;
                        let w = len.min(TAIL_HASH_BYTES as u64);
                        hash_bytes(&read_range(&fs::File::open(&path)?, len - w, w)?) != tail
                    };
                    crate::oplog!("파일 교체 감지, 재구성 {}", path.display());
                    let reset = rebuild.entry(t.key.clone()).or_default();
                    if replaced {
                        reset.insert(path.clone());
                    }
                    Ok(())
                }),
                (None, None) => Ok(()),
            };
            if let Err(e) = r
                && wsl
            {
                // 나열에는 보이는데 열리지 않는다. 그 배포판의 경로를 더 건드리지 않는다
                crate::oplog!("경로 대조 실패 {}: {e}", path.display());
                self.mark_suspect();
                return;
            }
            // 로컬 실패(그새 지워짐 등)는 다음 Scan 의 없음 판정이 맡는다
        }

        for key in &gone_teams {
            self.files.retain(|_, t| &t.key != key);
            self.read_failures.retain(|_, k| k != key);
            self.rebuild_due.remove(key);
            self.gone.remove(key);
            self.lock_world().remove_team(key);
        }
        // 서브만 없으면 그 직원만 퇴장하고 리더를 놓는다. 메인도 없으면 팀 제거 규칙이 정한다
        for path in gone_subs {
            let Some(key) = self.files.get(&path).map(|t| t.key.clone()) else {
                continue;
            };
            if self.main_present(&key) {
                self.files.remove(&path);
                self.retire(&key, &path, now);
            }
        }
        let failed: BTreeSet<_> = self.read_failures.values().cloned().collect();
        self.read_failures.retain(|path, _| {
            self.files.contains_key(path) || seen.contains(path) || !observed(path)
        });
        for key in failed {
            self.sync_read_failed(&key);
        }
        // meta.json 만 없으면 퇴장만 한다. 리더를 놓으면 다음 Scan 이 남은 jsonl 을 새 파일로 다시 집는다
        for path in meta_gone {
            let Some(key) = self.files.get(&path).map(|t| t.key.clone()) else {
                continue;
            };
            if self.main_present(&key) {
                self.retire(&key, &path, now);
            }
        }
        for (key, new) in &found {
            if self.suspect() {
                return;
            }
            let reset = rebuild.remove(key).unwrap_or_default();
            for p in new {
                self.revive(key, p);
            }
            self.try_rebuild(key, now, new, &reset);
            if let Some(w) = &self.watch {
                w.read(new.len());
            }
        }
        for (key, reset) in rebuild {
            if self.suspect() {
                return;
            }
            // meta.json 이 다시 생긴 직원은 되살린다
            let back: Vec<PathBuf> = self
                .files
                .iter()
                .filter(|(p, t)| t.key == key && t.has_meta && is_subagent(p))
                .map(|(p, _)| p.clone())
                .collect();
            for p in &back {
                self.revive(&key, p);
            }
            self.try_rebuild(&key, now, &BTreeSet::new(), &reset);
        }
        if !self.is_wsl() {
            let tracked: HashSet<&str> = self
                .files
                .values()
                .filter(|t| t.key.vendor == Vendor::Claude)
                .map(|t| t.key.session.as_str())
                .collect();
            self.awaited = self
                .alive
                .keys()
                .filter(|id| !tracked.contains(id.as_str()))
                .cloned()
                .collect();
        }
    }

    /// 서브 파일이 다시 보이면 퇴장 기억을 지운다
    fn revive(&mut self, key: &TeamKey, path: &Path) {
        if let (Some(agent), Some(g)) = (agent_of(path), self.gone.get_mut(key)) {
            g.remove(&agent);
        }
    }

    fn main_present(&self, key: &TeamKey) -> bool {
        self.files
            .iter()
            .any(|(p, t)| &t.key == key && !is_subagent(p) && t.missing == 0)
    }

    fn retire(&mut self, key: &TeamKey, path: &Path, now: DateTime<Utc>) {
        let Some(agent) = agent_of(path) else {
            return;
        };
        self.gone
            .entry(key.clone())
            .or_default()
            .entry(agent.clone())
            .or_insert(now);
        self.lock_world().retire_employee(key, &agent, now);
    }

    /// Claude 세션 등록부 (docs/CONTRACTS.md C5): 살아 있는 세션을 팀에 표시하고 이름·동을 넣는다
    fn read_registry(&mut self) {
        if self.suspect() {
            return;
        }
        let homes: BTreeSet<_> = self
            .sites
            .iter()
            .filter(|s| s.vendor == Vendor::Claude)
            .filter_map(|s| s.root.parent())
            .map(Path::to_path_buf)
            .chain(self.registry_roots.keys().cloned())
            .collect();
        for h in homes {
            if self.suspect() {
                return;
            }
            let previous = self.registry_roots.entry(h.clone()).or_default();
            let live = match &self.host.kind {
                HostKind::Windows => {
                    self.path_ops += 1;
                    registry::live_sessions(&h, previous)
                }
                HostKind::Wsl { distro } => {
                    let proc_root = wsl_proc_root(&h, distro);
                    #[cfg(test)]
                    let proc_root = self.proc_root_override.clone().or(proc_root);
                    let Some(proc_root) = proc_root else {
                        continue;
                    };
                    let heartbeat = self.heartbeat.clone();
                    let mut ops = 0;
                    let live = registry::live_wsl_sessions(&h, &proc_root, previous, || {
                        if heartbeat.suspect.load(Ordering::Relaxed) {
                            return false;
                        }
                        ops += 1;
                        true
                    });
                    self.path_ops += ops;
                    live
                }
            };
            // 한 루트의 실패가 다른 루트의 갱신을 막거나 직전 생존 행을 지우지 않는다.
            if let Some(live) = live {
                *previous = live;
            }
        }
        if self.suspect() {
            return;
        }
        self.registry_roots.retain(|_, rows| !rows.is_empty());
        self.alive.clear();
        for (id, row) in self
            .registry_roots
            .values()
            .flat_map(|rows| rows.iter())
            .filter(|(id, _)| safe_id(id))
        {
            match self.alive.get(id) {
                Some(old) if registry::rank(old) >= registry::rank(row) => {}
                _ => {
                    self.alive.insert(id.clone(), row.clone());
                }
            }
        }
        let mut w = self.world.lock().unwrap_or_else(|e| e.into_inner());
        let mut ours = Vec::new();
        for (k, t) in w.teams.iter_mut() {
            if k.host == self.host.id && k.vendor == Vendor::Claude {
                t.alive_in_registry = self.alive.contains_key(&k.session);
                ours.push(k.clone());
            }
        }
        for k in ours {
            let r = self.alive.get(&k.session);
            w.set_registry(
                &k,
                r.and_then(|r| r.user_name()),
                r.and_then(|r| r.product()),
            );
        }
    }

    /// 기다리는 세션의 jsonl 이 생겼나. 그 경로만 1초 간격으로 본다 (docs/CONTRACTS.md C6).
    /// 새 프로젝트 폴더에 생긴 파일은 다음 Scan 이 찾는다
    fn awaited_appeared(&mut self, now: DateTime<Utc>) -> bool {
        if self.awaited.is_empty() || self.last_await.is_some_and(|t| now - t < AWAIT) {
            return false;
        }
        self.last_await = Some(now);
        self.path_ops += (self.awaited.len() * self.project_dirs.len()) as u64;
        self.awaited.iter().any(|id| {
            self.project_dirs
                .iter()
                .any(|d| d.join(format!("{id}.jsonl")).is_file())
        })
    }

    /// 2초 넘게 멈춘 호스트를 의심으로 바꾼다. 멈춘 I/O 는 취소하지 않는다 (docs/CONTRACTS.md C5·C13)
    fn watch_remotes(&self) {
        let now = mono_ms();
        let remotes = self
            .remotes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (id, hb) in remotes {
            if now - hb.last_tick_ms.load(Ordering::Relaxed) > STALL_MS
                && set_suspect(&self.world, &id, &hb, true)
            {
                crate::oplog!("{id} 스레드가 멈췄다. Probe 가 켜짐을 확인할 때까지 의심");
            }
        }
    }
}

fn wsl_proc_root(home: &Path, distro: &str) -> Option<PathBuf> {
    let Component::Prefix(prefix) = home.components().next()? else {
        return None;
    };
    let (Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share)) = prefix.kind() else {
        return None;
    };
    let server = server.to_str()?;
    if !["wsl.localhost", "wsl$"]
        .iter()
        .any(|s| server.eq_ignore_ascii_case(s))
        || !share.to_str()?.eq_ignore_ascii_case(distro)
    {
        return None;
    }
    Some(PathBuf::from(format!(
        r"\\{server}\{}\proc",
        share.to_str()?
    )))
}

/// 30초마다 Windows Site 와 WSL 배포판의 켜짐·꺼짐을 각 호스트에 넘긴다 (docs/CONTRACTS.md C5).
/// `wsl.exe` 와 켜진 배포판의 UNC 를 쓰므로 자기 스레드에서 돈다. 멈춰도 Windows 갱신과 감시자는 계속된다 (docs/CONTRACTS.md C6)
pub struct Prober {
    runtime: Arc<Runtime>,
    probe: ProbeFn,
    windows: Arc<Heartbeat>,
    remotes: Remotes,
    world: Arc<Mutex<OfficeWorld>>,
    clock: Arc<dyn Clock>,
    diag: Arc<Mutex<Diag>>,
    watch: Option<Arc<ScanWatch>>,
    threads: Vec<(Arc<Heartbeat>, thread::JoinHandle<()>)>,
}

impl Prober {
    /// Windows 스레드와 감시 목록·world 를 함께 쓴다
    pub fn new(windows: &HostLoop, probe: ProbeFn) -> Self {
        windows
            .runtime
            .register(&windows.host, windows.heartbeat.clone());
        Self {
            runtime: windows.runtime.clone(),
            probe,
            windows: windows.heartbeat.clone(),
            remotes: windows.remotes.clone(),
            world: windows.world.clone(),
            clock: windows.clock.clone(),
            diag: windows.diag.clone(),
            watch: windows.watch.clone(),
            threads: Vec::new(),
        }
    }

    /// 한 번 본다. 꺼진 배포판은 UNC 를 건드리지 않고, 의심 상태를 푸는 것도 여기서만 한다
    pub fn run(&mut self) {
        let (local, distros) = (self.probe)();
        self.runtime
            .publish_probe(&crate::discover::windows_host(), &self.windows, local);
        for d in distros {
            let known = self
                .remotes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|(id, _)| *id == d.host.id)
                .map(|(_, hb)| hb.clone());
            if let Some(hb) = known {
                match d.running {
                    Some(true) => {
                        // Site 를 먼저 넘긴 뒤 의심을 푼다. 순서가 바뀌면 경로 접근 없이 한 틱이 돈다
                        self.runtime.publish_probe(&d.host, &hb, d.sites);
                        set_suspect(&self.world, &d.host.id, &hb, false);
                        // 팝업 읽기가 상한을 넘겨 world 에만 켠 의심(detail)도 켜짐을 본 여기서 푼다
                        self.world
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .set_suspect(&d.host.id, false);
                    }
                    Some(false) => {
                        set_suspect(&self.world, &d.host.id, &hb, true);
                    }
                    // wsl.exe 실패는 모름이다. 핸들 읽기는 이어 가고 경로 접근만 안 한다
                    None => {}
                }
            } else if d.running == Some(true) && !d.sites.is_empty() {
                let mut h = HostLoop::new(
                    d.host.clone(),
                    Vec::new(),
                    self.world.clone(),
                    self.clock.clone(),
                    self.diag.clone(),
                );
                h.watch = self.watch.clone();
                h.runtime = self.runtime.clone();
                self.runtime.register(&h.host, h.heartbeat.clone());
                self.runtime.publish_probe(&d.host, &h.heartbeat, d.sites);
                let hb = h.heartbeat.clone();
                self.remotes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((d.host.id.clone(), hb.clone()));
                // ponytail: 레지스트리에서 사라진 배포판의 스레드는 멈추지 않는다, 배포판 삭제가 보이면 올린다
                self.threads.push((hb, spawn(h)));
            }
        }
    }
}

/// Probe 스레드. Windows 스레드의 `stop` 을 보면 띄운 WSL 스레드도 멈추고 끝난다
pub fn spawn_probe(mut p: Prober) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        'run: loop {
            p.run();
            let until = Instant::now() + PROBE;
            while Instant::now() < until {
                if p.windows.stop.load(Ordering::Relaxed) {
                    break 'run;
                }
                if p.runtime.rescan.swap(false, Ordering::Relaxed) {
                    break;
                }
                thread::sleep(TICK);
            }
        }
        for (hb, t) in p.threads.drain(..) {
            hb.stop.store(true, Ordering::Relaxed);
            let _ = t.join();
        }
    })
}

/// 틱 간격. 연달아 5번 200ms 를 넘기면 그 호스트만 400ms 로 늦추고, 5번 연달아 안 넘기면 되돌린다 (docs/CONTRACTS.md C13)
struct Pace {
    slow: bool,
    run: u32,
}

impl Pace {
    /// 이번 틱 소요를 받아 다음 틱까지 쉴 시간을 준다
    fn next(&mut self, host: &str, spent: StdDuration) -> StdDuration {
        let over = spent > TICK;
        // 지금 상태와 반대인 틱이 이어진 수
        self.run = if over != self.slow { self.run + 1 } else { 0 };
        if self.run >= SLOW_AFTER {
            self.slow = over;
            self.run = 0;
            crate::oplog!(
                "{host} 틱 {}ms 로 {}",
                if over { 400 } else { 200 },
                if over { "늦춤" } else { "되돌림" }
            );
        }
        let every = if self.slow { SLOW_TICK } else { TICK };
        every.saturating_sub(spent)
    }
}

/// 호스트 하나의 루프를 스레드로 돌린다. 한 호스트가 UNC 에서 멈춰도 다른 호스트는 돈다 (docs/CONTRACTS.md C6).
/// `heartbeat.stop` 을 켜면 끝난다
pub fn spawn(mut host: HostLoop) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut pace = Pace {
            slow: false,
            run: 0,
        };
        while !host.heartbeat.stop.load(Ordering::Relaxed) {
            let start = Instant::now();
            host.tick();
            let spent = start.elapsed();
            host.tick_stats.record(spent);
            thread::sleep(pace.next(&host.host.id, spent));
        }
        // 꺼진 배포판의 핸들은 닫기만 해도 배포판을 다시 켠다 (docs/CONTRACTS.md C5). 의심 중이면 닫지 않고 둔다
        if host.suspect() {
            std::mem::forget(host);
        }
    })
}

/// 한 틱에 같은 팀의 여러 파일에서 읽은 이벤트를 적용 순서로 늘어놓는다 (docs/CONTRACTS.md C4 순서, `classify::merge` 와 같은 규칙).
/// 키는 파일마다 이전 틱까지 포함한 누적 최대 ts. 키가 같으면 파일 순서(메인 먼저), 그다음 줄 순서
fn merge_live(files: Vec<(Option<DateTime<Utc>>, Vec<Event>)>) -> Vec<Event> {
    let mut keyed = Vec::new();
    for (f, (start, events)) in files.into_iter().enumerate() {
        let mut key = start;
        for (n, ev) in events.into_iter().enumerate() {
            key = key.max(ev.ts);
            keyed.push((key, f, n, ev));
        }
    }
    keyed.sort_by_key(|k| (k.0, k.1, k.2));
    keyed.into_iter().map(|(.., ev)| ev).collect()
}

/// 파일 내용·등록부에서 온 id 를 경로 조각으로 쓸 수 있나
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `agent-<id>.jsonl` 의 id
fn agent_of(path: &Path) -> Option<String> {
    path.file_stem()?
        .to_str()?
        .strip_prefix("agent-")
        .map(str::to_string)
}

fn open(
    path: &Path,
    key: TeamKey,
    last_read: DateTime<Utc>,
    generation: u32,
) -> io::Result<Tracked> {
    Ok(Tracked {
        parser: make_parser(path, key.vendor),
        reader: Some(TailReader::open_at(path, generation)?),
        closed: None,
        closed_offset: 0,
        key,
        last_read,
        missing: 0,
        has_meta: false,
        max_ts: None,
        generation,
        stale: false,
    })
}

/// 재구성용으로 끝까지 읽는다. 8MB 상한이 있어 backlog 가 남으면 이어 읽는다 (docs/CONTRACTS.md C6).
/// 줄이 안 나온 읽기에서 멈추면 첫 줄이 8MB 넘는 세션을 영영 못 만든다. 파일 끝에 닿을 때까지 읽는다.
/// 읽기가 나아가지 않으면(stat 이 길이를 부풀려 말하는 경우) 끝낸다.
/// 읽는 사이에 파일이 다시 쓰여 Reset 이 나면 그 세대 처음부터 다시 모은다. 안 그러면 옛 줄과 새 줄이 둘 다 붙는다
fn read_all(
    t: &mut Tracked,
    path: &Path,
    unhandled: &mut Vec<Unhandled>,
) -> io::Result<Vec<Event>> {
    let mut events = Vec::new();
    let Some(reader) = t.reader.as_mut() else {
        return Ok(events);
    };
    let mut prev_end = None;
    loop {
        let pending = reader.read_new_lines(MAX_READ_BYTES)?;
        let end = pending.finished(&mut prev_end);
        if pending.dropped_lines > 0 {
            crate::oplog!("64MB 넘는 줄 버림 {}", path.display());
        }
        if pending.reset {
            crate::oplog!("Reset {}", path.display());
            events.clear();
            unhandled.clear();
            t.parser = make_parser(path, t.key.vendor);
        }
        let generation = u64::from(pending.generation);
        for line in &pending.lines {
            let raw = RawRef {
                path: path.to_path_buf(),
                generation,
                byte_offset: line.offset,
                line_no: line.line_no,
            };
            events.extend(t.parser.parse_line(&line.bytes, &raw, unhandled));
        }
        reader.commit(pending);
        if end {
            return Ok(events);
        }
    }
}

pub fn is_subagent(p: &Path) -> bool {
    p.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "subagents")
}

/// Codex 세션 id 는 파일명 끝 UUID 36자 (docs/CONTRACTS.md C7)
pub fn codex_id(stem: &str) -> &str {
    stem.get(stem.len().saturating_sub(36)..).unwrap_or(stem)
}

/// 파일 하나의 파서. 서브 파일은 옆 meta.json 의 깊이와 세션 id 를 쓴다
pub fn make_parser(path: &Path, vendor: Vendor) -> Box<dyn Parser + Send> {
    match vendor {
        Vendor::Codex => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            Box::new(CodexParser::new(codex_id(stem)))
        }
        _ if is_subagent(path) => {
            let depth = sub_meta(path).map_or(1, |m| m.depth);
            Box::new(ClaudeParser::new(depth, claude_session_of(path)))
        }
        _ => Box::new(ClaudeParser::new(0, None)),
    }
}

/// `<세션>/subagents/agent-<id>.jsonl` 의 세션 id
fn claude_session_of(path: &Path) -> Option<String> {
    path.parent()?
        .parent()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// 옆의 `agent-<id>.meta.json` (docs/CONTRACTS.md C7)
pub fn sub_meta(path: &Path) -> Option<SubMeta> {
    let stem = path.file_stem()?.to_str()?;
    let agent_id = stem.strip_prefix("agent-")?.to_string();
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path.with_extension("meta.json")).ok()?).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Some(SubMeta {
        agent_id,
        model: s("model"),
        effort: s("effort"),
        description: s("description"),
        depth: v
            .get("spawnDepth")
            .and_then(|d| d.as_u64())
            .and_then(|d| u8::try_from(d).ok())
            .unwrap_or(1),
    })
}

/// SessionMeta 가 오기 전의 자리값. 첫 레코드가 바로 고친다
fn default_product(vendor: Vendor) -> Product {
    match vendor {
        Vendor::Claude => Product::ClaudeDesktop,
        Vendor::Codex => Product::CodexApp,
        Vendor::Gemini => Product::AntigravityApp,
    }
}

/// Site 한 번 나열한 결과
#[derive(Default)]
pub struct Listing {
    /// 세션 파일과 그 세션 id
    pub files: Vec<(PathBuf, String)>,
    /// 서브 파일 옆 `*.meta.json`
    metas: HashSet<PathBuf>,
    /// Claude 프로젝트 폴더 (등록부 세션 기다리기용)
    projects: Vec<PathBuf>,
    failed_subtrees: Vec<PathBuf>,
}

/// Site 루트 실패는 Err, 하위 폴더 실패는 기존 파일을 보존할 범위와 정상 발견을 함께 돌려준다.
pub fn list_site(site: &Site, ops: &mut u64) -> io::Result<Listing> {
    list_site_with(site, ops, &mut entries)
}

type EntryReader<'a> = dyn FnMut(&Path, bool, &mut u64) -> io::Result<Vec<(PathBuf, bool)>> + 'a;

fn list_site_with(site: &Site, ops: &mut u64, read: &mut EntryReader<'_>) -> io::Result<Listing> {
    let mut out = Listing::default();
    let roots = read(&site.root, false, ops)?
        .into_iter()
        .filter(|(_, dir)| *dir)
        .map(|(p, _)| p);
    match site.vendor {
        Vendor::Claude => {
            for project in roots {
                for f in partial_entries(&project, false, ops, read, &mut out) {
                    if f.extension().is_none_or(|e| e != "jsonl") {
                        continue;
                    }
                    let Some(id) = f.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                        continue;
                    };
                    for sub in partial_entries(
                        &project.join(&id).join("subagents"),
                        false,
                        ops,
                        read,
                        &mut out,
                    ) {
                        let name = sub.file_name().map(|n| n.to_string_lossy().into_owned());
                        if name.as_deref().is_some_and(|n| n.ends_with(".meta.json")) {
                            out.metas.insert(sub);
                        } else if sub.extension().is_some_and(|e| e == "jsonl") {
                            out.files.push((sub, id.clone()));
                        }
                    }
                    out.files.push((f, id));
                }
                out.projects.push(project);
            }
        }
        Vendor::Codex => {
            for y in roots {
                for m in partial_entries(&y, true, ops, read, &mut out) {
                    for d in partial_entries(&m, true, ops, read, &mut out) {
                        for f in partial_entries(&d, false, ops, read, &mut out) {
                            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned());
                            let jsonl = f.extension().is_some_and(|e| e == "jsonl");
                            if let Some(stem) = stem.filter(|s| jsonl && s.starts_with("rollout-"))
                            {
                                let id = codex_id(&stem).to_string();
                                out.files.push((f, id));
                            }
                        }
                    }
                }
            }
        }
        Vendor::Gemini => {}
    }
    Ok(out)
}

fn partial_entries(
    p: &Path,
    dirs: bool,
    ops: &mut u64,
    read: &mut EntryReader<'_>,
    out: &mut Listing,
) -> Vec<PathBuf> {
    match read(p, true, ops) {
        Ok(entries) => entries
            .into_iter()
            .filter(|(_, dir)| *dir == dirs)
            .map(|(p, _)| p)
            .collect(),
        Err(e) => {
            crate::oplog_once!("세션 하위 폴더 나열 실패 {}: {e}", p.display());
            out.failed_subtrees.push(p.to_owned());
            Vec::new()
        }
    }
}

/// 폴더 안 (경로, 폴더인가). 종류는 나열 결과에서 읽어 항목마다 stat 하지 않는다 (docs/CONTRACTS.md C13).
/// `missing_ok` 면 그새 없어진 폴더는 빈 목록이다. Site 루트가 없으면 "못 봄"이라 에러로 둔다
fn entries(p: &Path, missing_ok: bool, ops: &mut u64) -> io::Result<Vec<(PathBuf, bool)>> {
    *ops += 1;
    let rd = match fs::read_dir(p) {
        Ok(rd) => rd,
        Err(e) if missing_ok && e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e?;
        let ft = e.file_type()?;
        let p = e.path();
        // 링크는 드물어서 그때만 따라가 본다
        let dir = if ft.is_symlink() {
            p.is_dir()
        } else {
            ft.is_dir()
        };
        out.push((p, dir));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::ReadMode;
    use crate::office::Team;
    use crate::office::names::NameBook;
    use crate::office::world::Scale;
    use chrono::FixedOffset;

    const REJECT: &str = "13f4d27c-e1d7-fa23-0b0e-05ad838d1b68";
    const SYNC: &str = "ed46b23c-eb2a-4459-1e37-cb69d8ecad93";
    const SYNC_SUB: &str = "ac0c27afe76bccd54";
    const ASYNC: &str = "302222d1-a790-f2ee-6a9b-2212d4de135d";
    const ASYNC_SUB: &str = "a4fa9a3e07b34f715";

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(rel)
    }

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                copy_dir(&p, &to.join(e.file_name()));
            } else {
                fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }

    fn t0() -> DateTime<Utc> {
        "2026-03-02T09:00:00Z".parse().unwrap()
    }

    fn secs(n: i64) -> Duration {
        Duration::seconds(n)
    }

    fn ms(n: i64) -> Duration {
        Duration::milliseconds(n)
    }

    fn win() -> Host {
        Host {
            id: "windows".into(),
            kind: HostKind::Windows,
            label: "Windows".into(),
        }
    }

    fn wsl() -> Host {
        Host {
            id: "wsl:Test".into(),
            kind: HostKind::Wsl {
                distro: "Test".into(),
            },
            label: "Test".into(),
        }
    }

    fn site_of(host: &Host, vendor: Vendor, root: &Path) -> Site {
        Site {
            host: host.clone(),
            vendor,
            root: root.to_path_buf(),
            read_mode: ReadMode::Append,
        }
    }

    fn lines_of(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn append(path: &Path, lines: &[String]) {
        use std::io::Write;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    fn ts_of(line: &str) -> Option<DateTime<Utc>> {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        v["timestamp"].as_str().and_then(|s| s.parse().ok())
    }

    /// Claude 레코드 한 줄. `agent` 가 있으면 서브 파일 줄이다
    fn rec(
        session: &str,
        agent: Option<&str>,
        ts: DateTime<Utc>,
        kind: &str,
        content: serde_json::Value,
    ) -> String {
        let mut v = serde_json::json!({
            "type": kind,
            "message": {"role": kind, "content": content},
            "uuid": format!("u-{}-{}", agent.unwrap_or("m"), ts.timestamp_millis()),
            "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "entrypoint": "claude-desktop",
            "cwd": "C:\\dir1",
            "sessionId": session,
        });
        if let Some(a) = agent {
            v["agentId"] = serde_json::Value::String(a.into());
            v["isSidechain"] = serde_json::Value::Bool(true);
        }
        v.to_string()
    }

    /// 사장님 지시 한 줄 (Claude 메인)
    fn user_line(session: &str, ts: DateTime<Utc>, text: &str) -> String {
        rec(session, None, ts, "user", serde_json::json!(text))
    }

    /// 논리 시계(배속 0)와 world·diag 를 나눠 쓰는 테스트 환경
    struct Env {
        tmp: tempfile::TempDir,
        world: Arc<Mutex<OfficeWorld>>,
        clock: Arc<LogicalClock>,
        diag: Arc<Mutex<Diag>>,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let world = Arc::new(Mutex::new(OfficeWorld::new(
            Scale::Large,
            NameBook::load(tmp.path()),
            FixedOffset::east_opt(0).unwrap(),
        )));
        Env {
            clock: Arc::new(LogicalClock::new(t0(), 0.0)),
            diag: Arc::new(Mutex::new(Diag::open(tmp.path(), false))),
            world,
            tmp,
        }
    }

    impl Env {
        fn host(&self, host: Host, sites: Vec<Site>) -> HostLoop {
            HostLoop::new(
                host,
                sites,
                self.world.clone(),
                self.clock.clone(),
                self.diag.clone(),
            )
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let p = self.tmp.path().join(rel);
            fs::create_dir_all(&p).unwrap();
            p
        }

        /// 논리 시각을 옮긴다. 읽은 이벤트의 ts 처럼 알린다
        fn at(&self, t: DateTime<Utc>) {
            self.clock.observe(t);
        }

        fn w(&self) -> std::sync::MutexGuard<'_, OfficeWorld> {
            self.world.lock().unwrap()
        }

        fn key(&self, session: &str) -> TeamKey {
            self.w()
                .teams
                .keys()
                .find(|k| k.session == session)
                .cloned()
                .unwrap()
        }

        fn team(&self, session: &str) -> Team {
            let key = self.key(session);
            self.w().teams[&key].clone()
        }

        fn seq(&self, key: &TeamKey) -> u64 {
            let w = self.w();
            w.state_seq.get(&w.office_of(key)).copied().unwrap_or(0)
        }

        fn select(&self, session: &str) {
            let key = self.key(session);
            let mut w = self.w();
            let office = w.office_of(&key);
            assert!(w.select(Some(office)));
        }
    }

    /// 같은 경로에서 차례로 돌린다. 팀 상태에 파일 경로가 들어 있어 경로가 같아야 통째로 비교할 수 있다
    fn in_same_dir(root: &Path, run: impl FnOnce(&Env, &Path) -> Team) -> Team {
        let _ = fs::remove_dir_all(root);
        fs::create_dir_all(root).unwrap();
        let e = env();
        let team = run(&e, root);
        fs::remove_dir_all(root).unwrap();
        team
    }

    fn probe_now(h: &HostLoop, sites: &[Site]) {
        *h.heartbeat.probed.lock().unwrap() = Some(ProbeUpdate {
            sites: sites.to_vec(),
            removed: Vec::new(),
        });
        set_suspect(&h.world, &h.host.id, &h.heartbeat, false);
    }

    #[test]
    fn logical_clock_follows_read_events_and_never_goes_back() {
        let c = LogicalClock::new(t0(), 0.0);
        assert_eq!(c.now(), t0());
        c.observe(t0() + secs(10));
        c.observe(t0() + secs(5));
        assert_eq!(c.now(), t0() + secs(10));
        let fast = LogicalClock::new(t0(), 1000.0);
        thread::sleep(StdDuration::from_millis(20));
        assert!(fast.now() >= t0() + secs(20));
    }

    #[test]
    fn first_tick_finds_sessions_and_builds_named_teams() {
        let e = env();
        let claude = e.dir("projects");
        copy_dir(&fixture("claude/sync-subagent"), &claude.join("p1"));
        let codex = e.dir("sessions");
        copy_dir(&fixture("codex/codex-app"), &codex.join("2026/03/02"));
        let mut h = e.host(
            win(),
            vec![
                site_of(&win(), Vendor::Claude, &claude),
                site_of(&win(), Vendor::Codex, &codex),
            ],
        );
        e.at(t0() + secs(300));
        h.tick();
        let w = e.w();
        assert_eq!(w.teams.len(), 2);
        // Claude 팀은 부장 + 동기 서브 하나
        let claude_team = w.teams.values().find(|t| t.employees.len() == 2).unwrap();
        assert!(claude_team.employees.iter().all(|e| !e.name.is_empty()));
        assert!(w.list_seq > 0);
        drop(w);
        // 이름은 잠금 밖에서 한 번 저장된다
        let saved = fs::read_to_string(e.tmp.path().join("names.json")).unwrap();
        assert!(saved.contains(SYNC_SUB));
    }

    #[test]
    fn append_reaches_world_within_500ms_on_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        append(&file, &[user_line(REJECT, Utc::now(), "첫 지시")]);
        let world = Arc::new(Mutex::new(OfficeWorld::new(
            Scale::Large,
            NameBook::load(tmp.path()),
            FixedOffset::east_opt(0).unwrap(),
        )));
        let h = HostLoop::new(
            win(),
            vec![site_of(&win(), Vendor::Claude, &root)],
            world.clone(),
            Arc::new(WallClock),
            Arc::new(Mutex::new(Diag::open(tmp.path(), false))),
        );
        let hb = h.heartbeat.clone();
        let thread = spawn(h);
        let wait = |cond: &dyn Fn(&OfficeWorld) -> bool, limit: StdDuration| {
            let start = Instant::now();
            while start.elapsed() < limit {
                if cond(&world.lock().unwrap()) {
                    return true;
                }
                thread::sleep(StdDuration::from_millis(2));
            }
            false
        };
        assert!(
            wait(&|w| !w.teams.is_empty(), StdDuration::from_secs(5)),
            "첫 Scan"
        );
        {
            let mut w = world.lock().unwrap();
            let key = w.teams.keys().next().unwrap().clone();
            let office = w.office_of(&key);
            assert!(w.select(Some(office)));
        }
        let boss_visit = |w: &OfficeWorld| {
            w.teams
                .values()
                .any(|t| t.props.boss_visit.as_deref() == Some("둘째 지시"))
        };
        // 붙인 직후부터 잰다
        append(&file, &[user_line(REJECT, Utc::now(), "둘째 지시")]);
        let ok = wait(&boss_visit, StdDuration::from_millis(500));
        hb.stop.store(true, Ordering::Relaxed);
        thread.join().unwrap();
        assert!(ok, "500ms 안에 반영되지 않았다");
    }

    #[test]
    fn new_session_appears_at_next_scan_with_arrival_cue_only_when_just_started() {
        for (late, cued) in [(secs(10), true), (Duration::hours(1), false)] {
            let e = env();
            let root = e.dir("projects");
            let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
            let start = t0() + late;
            e.at(start);
            h.tick();
            copy_dir(&fixture("claude/reject"), &root.join("p1"));
            e.at(start + secs(1));
            h.tick();
            assert!(e.w().teams.is_empty(), "Scan 전에는 모른다");
            e.at(start + secs(3));
            h.tick();
            assert_eq!(e.w().teams.len(), 1);
            let entered = e
                .w()
                .outbox
                .iter()
                .any(|(.., c)| matches!(c, Cue::Enter { agent } if agent == REJECT));
            assert_eq!(entered, cued, "첫 레코드 {late} 뒤 발견");
        }
    }

    #[test]
    fn late_discovery_is_quiet_even_when_rebuild_ends_long_ago() {
        let e = env();
        let root = e.dir("projects");
        copy_dir(&fixture("claude/async-subagent"), &root.join("p1"));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        e.at(t0() + Duration::minutes(45));
        h.tick();
        assert_eq!(e.w().teams.len(), 1);
        let w = e.w();
        assert!(w.outbox.is_empty(), "조용한 재구성에서 cue: {:?}", w.outbox);
    }

    #[test]
    fn missing_sub_retires_only_it_and_missing_main_removes_team_on_second_scan() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/sync-subagent"), &p1);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let mut now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        let key = e.key(SYNC);
        let subs = p1.join(SYNC).join("subagents");
        fs::remove_file(subs.join(format!("agent-{SYNC_SUB}.jsonl"))).unwrap();
        fs::remove_file(subs.join(format!("agent-{SYNC_SUB}.meta.json"))).unwrap();
        for _ in 0..2 {
            // 두 번째 Scan 에서도 다시 집거나 팀을 지우지 않는다
            now += secs(3);
            e.at(now);
            h.tick();
            let team = e.team(SYNC);
            let sub = team.employees.iter().find(|x| x.id == SYNC_SUB).unwrap();
            assert!(sub.retired_at.is_some());
            assert!(team.employees[0].retired_at.is_none());
            assert!(!h.files.keys().any(|p| is_subagent(p)), "리더를 놓는다");
        }
        let before = e.w().list_seq;
        fs::remove_file(p1.join(format!("{SYNC}.jsonl"))).unwrap();
        now += secs(3);
        e.at(now);
        h.tick();
        assert!(e.w().teams.contains_key(&key), "한 번 없는 것은 기다린다");
        now += secs(3);
        e.at(now);
        h.tick();
        assert!(e.w().teams.is_empty());
        assert!(h.files.is_empty());
        assert!(e.w().list_seq > before);
    }

    #[test]
    fn retired_sub_stays_retired_when_main_rebuild_spawns_it_again() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/async-subagent"), &p1);
        let main = p1.join(format!("{ASYNC}.jsonl"));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + secs(300);
        e.at(now);
        h.tick();
        let alive = |e: &Env| {
            let team = e.team(ASYNC);
            team.employees
                .iter()
                .find(|x| x.id == ASYNC_SUB)
                .unwrap()
                .retired_at
        };
        assert_eq!(alive(&e), None);
        let subs = p1.join(ASYNC).join("subagents");
        fs::remove_file(subs.join(format!("agent-{ASYNC_SUB}.jsonl"))).unwrap();
        fs::remove_file(subs.join(format!("agent-{ASYNC_SUB}.meta.json"))).unwrap();
        e.at(now + secs(3));
        h.tick();
        assert_eq!(alive(&e), Some(now + secs(3)));
        // 메인을 다른 파일로 바꿔 팀 전체를 다시 만든다. 메인의 Spawn 이 서브를 다시 만든다
        let text = fs::read_to_string(&main).unwrap();
        fs::rename(&main, main.with_extension("old")).unwrap();
        fs::write(&main, text).unwrap();
        e.at(now + secs(6));
        h.tick();
        assert_eq!(alive(&e), Some(now + secs(3)));
    }

    #[test]
    fn watchdog_marks_stalled_host_suspect_while_windows_keeps_updating() {
        let e = env();
        let root = e.dir("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        let lines = lines_of(&fixture(&format!("claude/reject/{REJECT}.jsonl")));
        append(&file, &lines[..6]);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let stalled = e.host(wsl(), Vec::new());
        let alive = Arc::new(Heartbeat::default());
        h.watch(stalled.host.id.clone(), stalled.heartbeat.clone());
        h.watch("wsl:Other".into(), alive.clone());
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        let key = e.key(REJECT);
        let (before, list) = (e.seq(&key), e.w().list_seq);
        stalled
            .heartbeat
            .last_tick_ms
            .store(mono_ms() - STALL_MS - 1, Ordering::Relaxed);
        append(&file, &lines[6..]);
        e.at(now + secs(2));
        h.tick();
        assert!(stalled.heartbeat.suspect.load(Ordering::Relaxed));
        assert!(!alive.suspect.load(Ordering::Relaxed));
        assert!(e.seq(&key) > before, "Windows 갱신은 계속된다");
        // 의심이 목록에 나간다 (S46)
        let w = e.w();
        assert!(w.suspect_hosts.contains("wsl:Test"));
        assert!(w.list_seq > list);
    }

    /// Probe 가 멈춰도(wsl.exe·UNC 행) Windows 갱신과 감시자는 계속된다
    #[test]
    fn stuck_probe_does_not_stop_windows_thread() {
        let e = env();
        let root = e.dir("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        let lines = lines_of(&fixture(&format!("claude/reject/{REJECT}.jsonl")));
        append(&file, &lines[..6]);
        let site = site_of(&win(), Vendor::Claude, &root);
        let mut h = e.host(win(), vec![site.clone()]);
        let stalled = Arc::new(Heartbeat::default());
        h.watch("wsl:Test".into(), stalled.clone());
        let release = Arc::new(AtomicBool::new(false));
        let gate = release.clone();
        let entered = Arc::new(AtomicBool::new(false));
        let inside = entered.clone();
        let sites = vec![site];
        let probe: ProbeFn = Box::new(move || {
            inside.store(true, Ordering::Relaxed);
            while !gate.load(Ordering::Relaxed) {
                thread::sleep(StdDuration::from_millis(5));
            }
            (sites.clone(), Vec::new())
        });
        let stop = h.heartbeat.clone();
        let probe_thread = spawn_probe(Prober::new(&h, probe));
        while !entered.load(Ordering::Relaxed) {
            thread::sleep(StdDuration::from_millis(5));
        }
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        let key = e.key(REJECT);
        let before = e.seq(&key);
        stalled
            .last_tick_ms
            .store(mono_ms() - STALL_MS - 1, Ordering::Relaxed);
        append(&file, &lines[6..]);
        e.at(now + secs(2));
        h.tick();
        assert!(e.seq(&key) > before);
        assert!(stalled.suspect.load(Ordering::Relaxed));
        release.store(true, Ordering::Relaxed);
        stop.stop.store(true, Ordering::Relaxed);
        probe_thread.join().unwrap();
    }

    #[test]
    fn reads_while_suspect_are_dropped_and_resume_applies_each_line_once() {
        let id = "dcc68dd7-8ab5-ede5-fb28-c09de28115be";
        let lines = lines_of(&fixture(&format!(
            "claude/bg-fail-reject-compact/{id}.jsonl"
        )));
        let e = env();
        let root = e.dir("wsl/projects");
        let file = root.join("p1").join(format!("{id}.jsonl"));
        append(&file, &lines[..20]);
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        let mut h = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        probe_now(&h, &sites);
        e.at(now);
        h.tick();
        let key = e.key(id);
        let before = e.seq(&key);
        // 읽기가 끝나기 전에 감시자가 의심으로 바꿨다
        append(&file, &lines[20..]);
        h.heartbeat.suspect.store(true, Ordering::Relaxed);
        e.at(now + secs(3));
        h.poll_team(&key, std::slice::from_ref(&file), now + secs(3), false);
        e.at(now + secs(4));
        h.tick();
        assert_eq!(e.seq(&key), before, "의심 중 읽은 구간은 적용하지 않는다");
        probe_now(&h, &sites);
        e.at(now + secs(6));
        h.tick();
        let got = e.team(id);

        // 같은 파일을 한 번에 읽은 것과 같다
        let f = env();
        let mut g = f.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        f.at(now + secs(6));
        g.tick();
        assert_eq!(got, f.team(id));
    }

    #[test]
    fn swapped_file_resets_at_next_scan_and_team_follows_new_content() {
        let e = env();
        let root = e.dir("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        assert_eq!(e.team(REJECT).started, t0());
        // 머리만 다르고 끝 4KB 는 같은 새 파일. 이어 읽기의 해시 대조로는 못 잡고 파일 ID 대조만 잡는다
        let old = fs::read_to_string(&file).unwrap();
        let new = old.replacen("2026-03-02T09:00:00.000Z", "2026-03-02T08:00:00.000Z", 1)
            + &user_line(REJECT, t0() + secs(140), "새 파일")
            + "\n";
        fs::rename(&file, file.with_extension("old")).unwrap();
        fs::write(&file, new).unwrap();
        e.at(now + secs(3));
        h.tick();
        let team = e.team(REJECT);
        assert_eq!(team.started, t0() - Duration::hours(1));
        assert_eq!(team.props.boss_visit.as_deref(), Some("새 파일"));
        // 다시 연 리더도 세대를 잇는다. 0 으로 돌아가면 옛 커서와 겹친다
        assert_eq!(h.files[&file].generation(), 1);
    }

    /// 퇴근 팀은 매 Scan 대신 Cold 폴링 때 바꿔치기를 본다 (docs/CONTRACTS.md C5)
    #[test]
    fn off_team_is_checked_for_swap_at_cold_poll_not_every_scan() {
        let e = env();
        let root = e.dir("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        assert_eq!(e.team(REJECT).phase, TeamPhase::Off);
        let old = fs::read_to_string(&file).unwrap();
        fs::rename(&file, file.with_extension("old")).unwrap();
        fs::write(&file, old.replacen("09:00:00.000Z", "08:00:00.000Z", 1)).unwrap();
        e.at(now + secs(3));
        h.tick();
        assert_eq!(e.team(REJECT).started, t0());
        e.at(now + secs(60));
        h.tick();
        assert_eq!(e.team(REJECT).started, t0() - Duration::hours(1));
    }

    /// fixture 하나를 (a) 한 번에 전체 읽기 (b) 줄마다 그 논리 시각에 append 하며 틱.
    /// 같은 논리 시점에서 팀 상태 전체가 같다 (docs/CONTRACTS.md C4)
    #[test]
    fn appending_line_by_line_matches_full_read_under_logical_clock() {
        let cases = [
            ("claude/async-subagent", Vendor::Claude),
            ("claude/sync-subagent", Vendor::Claude),
            ("claude/bg-fail-reject-compact", Vendor::Claude),
            ("claude/compaction", Vendor::Claude),
            ("claude/internal-tool-failure", Vendor::Claude),
            ("claude/parallel-reversed", Vendor::Claude),
            ("claude/reject", Vendor::Claude),
            ("claude/test-fail-fix-pass", Vendor::Claude),
            ("codex/aborted", Vendor::Codex),
            ("codex/ask-reject-compact", Vendor::Codex),
            ("codex/codex-app", Vendor::Codex),
        ];
        let shared = tempfile::tempdir().unwrap();
        let root = shared.path().join("root");
        for (dir, vendor) in cases {
            let src = fixture(dir);
            let sub = match vendor {
                Vendor::Claude => "p1",
                _ => "2026/03/02",
            };
            // (파일 상대 경로, 줄, ts) 를 docs/CONTRACTS.md C4 병합 순서(파일별 누적 최대 ts, 메인 먼저, 줄 순서)로
            let mut jsonl = Vec::new();
            let mut stack = vec![src.clone()];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).unwrap().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x == "jsonl") {
                        jsonl.push(p);
                    }
                }
            }
            jsonl.sort_by_key(|p| (is_subagent(p), p.clone()));
            let mut order = Vec::new();
            for (f, p) in jsonl.iter().enumerate() {
                let rel = p.strip_prefix(&src).unwrap().to_path_buf();
                let mut key = None;
                for (n, l) in lines_of(p).into_iter().enumerate() {
                    let ts = ts_of(&l);
                    key = key.max(ts);
                    order.push((key, f, n, rel.clone(), l));
                }
            }
            order.sort_by_key(|o| (o.0, o.1, o.2));
            let last = order.iter().filter_map(|o| o.0).max().unwrap();
            let session = jsonl[0].file_stem().unwrap().to_string_lossy().into_owned();
            let session = if vendor == Vendor::Codex {
                codex_id(&session).to_string()
            } else {
                session
            };
            // 마지막 줄 뒤 Scan(3초)이 한 번은 돈 시점부터 비교한다. 시각 없는 첫 줄만 있을 때는 팀을 만들지 않는다
            for end in [
                last + SCAN,
                last + Duration::minutes(10),
                last + Duration::minutes(40),
            ] {
                let want = in_same_dir(&root, |e, root| {
                    copy_dir(&src, &root.join(sub));
                    let mut h = e.host(win(), vec![site_of(&win(), vendor, root)]);
                    e.at(end);
                    h.tick();
                    e.team(&session)
                });
                let got = in_same_dir(&root, |e, root| {
                    let mut h = e.host(win(), vec![site_of(&win(), vendor, root)]);
                    for (key, _, _, rel, line) in &order {
                        if let Some(t) = key {
                            e.at(*t);
                        }
                        let dst = root.join(sub).join(rel);
                        // meta.json 은 서브의 첫 줄과 함께 생긴다
                        let meta = src.join(rel).with_extension("meta.json");
                        if meta.is_file() && !dst.exists() {
                            fs::create_dir_all(dst.parent().unwrap()).unwrap();
                            fs::copy(&meta, dst.with_extension("meta.json")).unwrap();
                        }
                        append(&dst, std::slice::from_ref(line));
                        h.tick();
                        // 팀이 생기면 선택해 Hot 으로 읽는다. 줄마다 그 틱에 읽혀야 실제 간격 append 와 같다
                        let mut w = e.w();
                        if w.selected.is_none()
                            && let Some(k) = w.teams.keys().next().cloned()
                        {
                            let office = w.office_of(&k);
                            w.select(Some(office));
                        }
                    }
                    e.at(end);
                    h.tick();
                    e.team(&session)
                });
                assert_eq!(got, want, "{dir} {end}");
            }
        }
    }

    /// 한 틱에 메인과 서브에 함께 붙은 줄은 docs/CONTRACTS.md C4 순서로 합쳐 적용한다. 파일마다 따로 적용하면
    /// 서브의 40분 줄이 메인의 30분30초 결과보다 먼저 들어가 31분 포기가 앞당겨진다 (T12 리뷰 재현)
    #[test]
    fn lines_batched_across_main_and_sub_merge_by_ts() {
        let sub = "a1";
        let m = |s: i64| t0() + secs(s);
        let main_lines = [
            user_line(REJECT, m(0), "빌드"),
            rec(
                REJECT,
                None,
                m(60),
                "assistant",
                serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "cargo build"}}]),
            ),
            rec(
                REJECT,
                None,
                m(1830),
                "user",
                serde_json::json!([{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok", "is_error": false}]),
            ),
        ];
        let sub_lines = [
            rec(REJECT, Some(sub), m(10), "user", serde_json::json!("조사")),
            rec(
                REJECT,
                Some(sub),
                m(2400),
                "assistant",
                serde_json::json!([{"type": "text", "text": "끝"}]),
            ),
        ];
        let shared = tempfile::tempdir().unwrap();
        let root = shared.path().join("root");
        let paths = |root: &Path| {
            let p1 = root.join("p1");
            (
                p1.join(format!("{REJECT}.jsonl")),
                p1.join(REJECT)
                    .join("subagents")
                    .join(format!("agent-{sub}.jsonl")),
            )
        };
        let end = m(2460);
        let want = in_same_dir(&root, |e, root| {
            let (main, s) = paths(root);
            append(&main, &main_lines);
            append(&s, &sub_lines);
            let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, root)]);
            e.at(end);
            h.tick();
            e.team(REJECT)
        });
        let got = in_same_dir(&root, |e, root| {
            let (main, s) = paths(root);
            append(&main, &main_lines[..2]);
            append(&s, &sub_lines[..1]);
            let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, root)]);
            e.at(m(120));
            h.tick();
            append(&main, &main_lines[2..]);
            append(&s, &sub_lines[1..]);
            e.at(end);
            h.tick();
            e.team(REJECT)
        });
        assert_eq!(got, want);
    }

    /// 잠든 노트북·의심이 풀린 WSL 처럼 시계가 크게 건너뛴 뒤 밀린 줄을 한 번에 읽어도 전체 읽기와 같다.
    /// 결과보다 30분 포기가 먼저 일어나야 한다 (docs/CONTRACTS.md C4, docs/CONTRACTS.md C8)
    #[test]
    fn batch_after_clock_jump_matches_full_read() {
        let lines = [
            user_line(REJECT, t0(), "빌드"),
            rec(
                REJECT,
                None,
                t0() + secs(1),
                "assistant",
                serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "cargo build"}}]),
            ),
            rec(
                REJECT,
                None,
                t0() + Duration::minutes(31),
                "user",
                serde_json::json!([{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok", "is_error": false}]),
            ),
        ];
        let end = t0() + Duration::minutes(32);
        let shared = tempfile::tempdir().unwrap();
        let root = shared.path().join("root");
        let run = |split: usize| {
            in_same_dir(&root, |e, root| {
                let file = root.join("p1").join(format!("{REJECT}.jsonl"));
                let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, root)]);
                append(&file, &lines[..split]);
                e.at(t0() + secs(1));
                h.tick();
                append(&file, &lines[split..]);
                e.at(end);
                h.tick();
                e.team(REJECT)
            })
        };
        assert_eq!(run(2), run(3));
    }

    #[test]
    fn wsl_registry_updates_only_after_probe_and_keeps_idle_team_alive() {
        let e = env();
        let root = e.dir("wsl/claude/projects");
        let home = root.parent().unwrap();
        let reg = home.join("sessions/7.json");
        fs::create_dir_all(reg.parent().unwrap()).unwrap();
        fs::write(&reg, format!(r#"{{"pid":7,"sessionId":"{REJECT}","procStart":"42","startedAt":20000,"updatedAt":1,"name":"wsl-name","nameSource":"user","entrypoint":"cli"}}"#)).unwrap();
        let proc_root = e.dir("wsl/proc");
        fs::create_dir(proc_root.join("7")).unwrap();
        fs::write(proc_root.join("stat"), "btime 10\n").unwrap();
        fs::write(
            proc_root.join("7/stat"),
            format!("7 (claude) S {} 42 0\n", "0 ".repeat(18)),
        )
        .unwrap();
        let mut line: serde_json::Value =
            serde_json::from_str(&user_line(REJECT, t0(), "지시")).unwrap();
        line.as_object_mut().unwrap().remove("entrypoint");
        append(
            &root.join("p1").join(format!("{REJECT}.jsonl")),
            &[line.to_string()],
        );
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        let mut h = e.host(wsl(), sites.to_vec());
        h.proc_root_override = Some(proc_root.clone());
        e.at(t0() + secs(1));
        h.tick();
        assert_eq!(h.path_ops, 0);
        assert!(h.alive.is_empty());
        probe_now(&h, &sites);
        h.tick();
        assert!(e.team(REJECT).alive_in_registry);
        assert_eq!(e.team(REJECT).title, "wsl-name");
        assert_eq!(e.team(REJECT).product, Product::ClaudeCli);
        e.select(REJECT);
        let ops = h.path_ops;
        fs::remove_file(proc_root.join("7/stat")).unwrap();
        e.at(t0() + Duration::milliseconds(1200));
        h.tick();
        assert_eq!(
            h.path_ops, ops,
            "Hot 단독 틱은 등록부와 proc 를 읽지 않는다"
        );
        assert!(e.team(REJECT).alive_in_registry);
        e.at(t0() + Duration::hours(1));
        h.tick();
        assert_eq!(e.team(REJECT).phase, TeamPhase::Dozing);
        assert_eq!(h.path_ops, ops);
        probe_now(&h, &sites);
        h.mark_suspect();
        h.tick();
        assert_eq!(
            h.path_ops, ops,
            "의심 중에는 Probe 전달이 있어도 읽지 않는다"
        );
        probe_now(&h, &sites);
        h.tick();
        assert!(!e.team(REJECT).alive_in_registry);
        assert_ne!(e.team(REJECT).title, "wsl-name");
        assert_eq!(e.team(REJECT).phase, TeamPhase::Off);
    }

    #[test]
    fn wsl_proc_path_uses_only_matching_probe_share() {
        for (home, want) in [
            (
                r"\\wsl.localhost\Test\home\user\.claude",
                r"\\wsl.localhost\Test\proc",
            ),
            (r"\\wsl$\Test\home\user\.claude", r"\\wsl$\Test\proc"),
            (
                r"\\?\UNC\wsl.localhost\Test\home\user\.claude",
                r"\\wsl.localhost\Test\proc",
            ),
        ] {
            assert_eq!(
                wsl_proc_root(Path::new(home), "Test"),
                Some(PathBuf::from(want))
            );
            assert!(wsl_proc_root(Path::new(home), "Other").is_none());
        }
        assert!(wsl_proc_root(Path::new(r"C:\home\.claude"), "Test").is_none());
        assert!(wsl_proc_root(Path::new(r"\\server\Test\home\.claude"), "Test").is_none());
    }

    #[test]
    fn wsl_touches_paths_only_right_after_probe() {
        let e = env();
        let root = e.dir("wsl/projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/reject"), &p1);
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        let mut h = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        assert!(e.w().teams.is_empty(), "Probe 전에는 나열하지 않는다");
        assert_eq!(h.path_ops, 0);
        probe_now(&h, &sites);
        e.at(now + secs(1));
        h.tick();
        assert!(h.path_ops > 0);
        let key = e.key(REJECT);
        copy_dir(&fixture("claude/compaction"), &p1);
        append(
            &p1.join(format!("{REJECT}.jsonl")),
            &[user_line(REJECT, t0() + secs(140), "핸들로 읽기")],
        );
        let ops = h.path_ops;
        for s in [4, 7, 10] {
            e.at(now + secs(s));
            h.tick();
        }
        assert_eq!(h.path_ops, ops, "Probe 밖에서는 경로를 건드리지 않는다");
        let w = e.w();
        assert_eq!(w.teams.len(), 1, "Scan 은 Probe 직후에만 돈다");
        assert_eq!(
            w.teams[&key].props.boss_visit.as_deref(),
            Some("핸들로 읽기")
        );
        drop(w);
        probe_now(&h, &sites);
        e.at(now + secs(11));
        h.tick();
        assert_eq!(e.w().teams.len(), 2);
    }

    /// 근무 중이 아닌 WSL 팀의 핸들은 Probe 직후에 닫고, 바뀌면 다음 Probe 직후에 다시 연다 (docs/CONTRACTS.md C5)
    #[test]
    fn idle_wsl_team_closes_handles_and_reopens_when_file_changes() {
        let e = env();
        let root = e.dir("wsl/projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        let mut h = e.host(wsl(), Vec::new());
        // 마지막 줄 20분 뒤라 졸음 (고민 10분이 지났고 퇴근 30분 전)
        let now = t0() + Duration::minutes(20);
        probe_now(&h, &sites);
        e.at(now);
        h.tick();
        assert_eq!(e.team(REJECT).phase, TeamPhase::Dozing);
        assert!(h.files[&file].reader.is_none(), "졸음 팀은 핸들을 닫는다");
        append(&file, &[user_line(REJECT, now + secs(5), "다시 일")]);
        e.at(now + secs(10));
        h.tick();
        assert_ne!(
            e.team(REJECT).props.boss_visit.as_deref(),
            Some("다시 일"),
            "닫힌 동안은 안 읽는다"
        );
        probe_now(&h, &sites);
        e.at(now + secs(30));
        h.tick();
        let team = e.team(REJECT);
        assert_eq!(team.props.boss_visit.as_deref(), Some("다시 일"));
        assert_eq!(team.phase, TeamPhase::Working);
        assert!(h.files[&file].reader.is_some(), "근무 중이면 핸들을 쥔다");
    }

    /// 근무 중으로 만든 WSL 팀이 다음 틱에 졸고, Probe 직전 폴링이 막 끝난 상태를 만든다.
    /// 그 뒤 Probe 직후 틱의 Warm 폴링(2초)은 건너뛰어 close_idle 의 읽기만 남는다
    fn dozing_wsl_team(e: &Env) -> (HostLoop, PathBuf, [Site; 1], DateTime<Utc>) {
        let root = e.dir("wsl/projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        let mut h = e.host(wsl(), Vec::new());
        probe_now(&h, &sites);
        e.at(t0() + secs(150));
        h.tick();
        assert_eq!(e.team(REJECT).phase, TeamPhase::Working);
        let now = t0() + Duration::minutes(20);
        e.at(now);
        h.tick();
        assert_eq!(e.team(REJECT).phase, TeamPhase::Dozing);
        assert!(
            h.files[&file].reader.is_some(),
            "Probe 밖에서는 닫지 않는다"
        );
        (h, file, sites, now)
    }

    #[test]
    fn diagnostic_request_keeps_closed_offset_and_never_touches_suspect_paths() {
        let e = env();
        let (mut h, file, sites, now) = dozing_wsl_team(&e);
        let offset = h.files[&file].reader.as_ref().unwrap().offset();
        probe_now(&h, &sites);
        e.at(now + secs(1));
        h.tick();
        assert!(h.files[&file].reader.is_none());
        h.heartbeat.suspect.store(true, Ordering::Relaxed);
        let ops = h.path_ops;
        let (tx, rx) = mpsc::channel();
        *h.heartbeat.diagnostic.lock().unwrap() = Some(tx);
        h.tick();
        let snapshot = rx.try_recv().unwrap();
        assert_eq!(h.path_ops, ops);
        assert!(snapshot.suspect);
        assert_eq!(
            snapshot
                .files
                .iter()
                .find(|f| f.path == file)
                .unwrap()
                .offset,
            offset
        );
        assert!(h.heartbeat.diagnostic.lock().unwrap().is_none());
    }

    #[test]
    fn probe_result_scans_windows_now_and_only_explicit_removal_deletes_a_team() {
        let e = env();
        let root = e.dir("claude/custom-records");
        let site = site_of(&win(), Vendor::Claude, &root);
        let mut h = e.host(win(), vec![site.clone()]);
        h.tick();
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        e.at(t0() + Duration::milliseconds(200));
        probe_now(&h, std::slice::from_ref(&site));
        h.tick();
        assert!(e.w().teams.keys().any(|k| k.session == REJECT));
        probe_now(&h, &[]);
        h.tick();
        assert!(
            e.w().teams.keys().any(|k| k.session == REJECT),
            "Probe 누락으로 팀을 지우지 않는다"
        );
        *h.heartbeat.probed.lock().unwrap() = Some(ProbeUpdate {
            sites: Vec::new(),
            removed: vec![(Vendor::Claude, root)],
        });
        h.tick();
        assert!(!e.w().teams.keys().any(|k| k.session == REJECT));
    }

    #[test]
    fn multiple_registry_roots_keep_failed_rows_rank_duplicates_and_remove_custom_root() {
        let e = env();
        let a = e.dir("a-home/custom-records");
        let z = e.dir("z-home/other-records");
        let ar = a.parent().unwrap().join("sessions");
        let zr = z.parent().unwrap().join("sessions");
        fs::create_dir_all(&ar).unwrap();
        fs::create_dir_all(&zr).unwrap();
        let pid = std::process::id();
        let row = |id: &str, name: &str, source: &str| {
            serde_json::json!({ "pid":pid, "sessionId":id, "name":name, "nameSource":source })
                .to_string()
        };
        fs::write(
            ar.join(format!("{pid}.json")),
            row("shared", "사용자 이름", "user"),
        )
        .unwrap();
        fs::write(
            zr.join(format!("{pid}.json")),
            row("shared", "자동 이름", "auto"),
        )
        .unwrap();
        let mut h = e.host(
            win(),
            vec![
                site_of(&win(), Vendor::Claude, &a),
                site_of(&win(), Vendor::Claude, &z),
            ],
        );
        h.tick();
        assert_eq!(h.alive["shared"].user_name(), Some("사용자 이름"));
        // 첫 루트 전체가 읽히지 않아도 두 번째 루트의 새 행을 읽는다.
        fs::rename(&ar, ar.with_file_name("saved-sessions")).unwrap();
        fs::write(&ar, "not a directory").unwrap();
        fs::write(
            zr.join(format!("{pid}.json")),
            row("new-session", "새 이름", "user"),
        )
        .unwrap();
        e.at(t0() + secs(3));
        h.tick();
        assert!(h.alive.contains_key("shared"));
        assert!(h.alive.contains_key("new-session"));
        *h.heartbeat.probed.lock().unwrap() = Some(ProbeUpdate {
            sites: vec![site_of(&win(), Vendor::Claude, &z)],
            removed: vec![(Vendor::Claude, a)],
        });
        h.tick();
        assert!(!h.alive.contains_key("shared"));
        assert!(h.alive.contains_key("new-session"));
        // 자동 탐색에서 빠진 루트의 캐시도 다음 허용 Scan에서 실제 생존을 다시 본다.
        fs::remove_file(zr.join(format!("{pid}.json"))).unwrap();
        probe_now(&h, &[]);
        h.tick();
        assert!(h.alive.is_empty());
    }

    #[test]
    fn removing_parent_manual_root_preserves_nested_automatic_site() {
        let e = env();
        let manual = e.dir("base");
        let automatic = manual.join(".claude/projects");
        copy_dir(&fixture("claude/reject"), &automatic.join("p1"));
        let manual_file = manual.join("p2/manual.jsonl");
        fs::create_dir_all(manual_file.parent().unwrap()).unwrap();
        fs::write(
            &manual_file,
            format!("{}\n", user_line("manual", t0(), "수동 팀")),
        )
        .unwrap();
        let automatic_site = site_of(&win(), Vendor::Claude, &automatic);
        let mut h = e.host(
            win(),
            vec![
                site_of(&win(), Vendor::Claude, &manual),
                automatic_site.clone(),
            ],
        );
        h.tick();
        assert!(e.w().teams.keys().any(|k| k.session == "manual"));
        let kept_file = automatic.join("p1").join(format!("{REJECT}.jsonl"));
        let offset = h.files[&kept_file].reader.as_ref().unwrap().offset();
        let removed = crate::settings::ManualPath {
            vendor: crate::settings::ManualVendor::Claude,
            root: manual.to_string_lossy().into_owned(),
        };
        h.runtime.manual_changed(&[removed], &[]);
        h.runtime.discovered(
            &crate::discover::ScanResult {
                sites: vec![automatic_site.clone()],
                distros: Vec::new(),
                candidates: Vec::new(),
            },
            &[],
        );
        // 탐색과 전달 사이에 Windows 틱이 돌아도 이전 Site로 제거를 적용하지 않는다.
        h.tick();
        assert!(e.w().teams.keys().any(|k| k.session == "manual"));
        h.runtime
            .publish_probe(&win(), &h.heartbeat, vec![automatic_site]);
        h.tick();
        assert!(!e.w().teams.keys().any(|k| k.session == "manual"));
        assert!(e.w().teams.keys().any(|k| k.session == REJECT));
        assert_eq!(
            h.files[&kept_file].reader.as_ref().unwrap().offset(),
            offset
        );
    }

    /// 졸음 팀에 8MB 넘는 backlog 가 있으면 닫기 전 읽기 한 번으로 다 못 읽는다. 덜 읽은 채 닫지 않는다
    #[test]
    fn idle_wsl_team_with_backlog_over_8mib_reads_every_line_before_closing() {
        let e = env();
        let (mut h, file, sites, now) = dozing_wsl_team(&e);
        let big = "x".repeat(9 * 1024 * 1024);
        append(
            &file,
            &[
                user_line(REJECT, t0() + secs(151), &big),
                user_line(REJECT, t0() + secs(152), "밀린 끝"),
            ],
        );
        probe_now(&h, &sites);
        e.at(now + secs(1));
        h.tick();
        // 남은 줄은 다음 틱이 읽고, 다 읽은 뒤의 Probe 가 닫는다
        e.at(now + secs(3));
        h.tick();
        probe_now(&h, &sites);
        e.at(now + secs(30));
        h.tick();
        assert_eq!(e.team(REJECT).props.boss_visit.as_deref(), Some("밀린 끝"));
        assert!(
            h.files[&file].reader.is_none(),
            "다 읽은 졸음 팀은 결국 닫는다"
        );
    }

    /// 닫기 전 마지막 읽기로 근무 중이 된 팀은 핸들을 쥔다. 닫으면 다음 Probe(30초)까지 못 읽는다
    #[test]
    fn idle_wsl_team_woken_by_last_read_keeps_handles() {
        let e = env();
        let (mut h, file, sites, now) = dozing_wsl_team(&e);
        append(&file, &[user_line(REJECT, now, "막 깨움")]);
        probe_now(&h, &sites);
        e.at(now + secs(1));
        h.tick();
        let team = e.team(REJECT);
        assert_eq!(team.props.boss_visit.as_deref(), Some("막 깨움"));
        assert_eq!(team.phase, TeamPhase::Working);
        assert!(h.files[&file].reader.is_some());
    }

    /// 닫아 둔 동안 파일이 줄었으면 Reset 이라 세대를 올려 다시 만든다 (docs/CONTRACTS.md C6)
    #[test]
    fn closed_file_that_shrank_rebuilds_with_new_generation() {
        let e = env();
        let (mut h, file, sites, now) = dozing_wsl_team(&e);
        probe_now(&h, &sites);
        e.at(now + secs(1));
        h.tick();
        assert!(h.files[&file].reader.is_none());
        fs::write(
            &file,
            user_line(REJECT, t0() + secs(145), "줄어듦")
                + "
",
        )
        .unwrap();
        probe_now(&h, &sites);
        e.at(now + secs(30));
        h.tick();
        assert_eq!(e.team(REJECT).props.boss_visit.as_deref(), Some("줄어듦"));
        assert_eq!(h.files[&file].generation(), 1);
    }

    /// 닫아 둔 파일을 `len` 바이트짜리 사용자 줄 하나로 바꿔 쓰고 그 글을 돌려준다
    fn replace_with_len(file: &Path, len: u64, text: &str) -> String {
        let ts = t0() + secs(145);
        let pad = len as usize - user_line(REJECT, ts, text).len() - 1;
        let text = format!("{text}{}", "x".repeat(pad));
        fs::write(file, user_line(REJECT, ts, &text) + "\n").unwrap();
        assert_eq!(fs::metadata(file).unwrap().len(), len);
        text
    }

    /// 닫아 둔 동안 같은 길이·더 긴 다른 내용으로 교체됐으면 Reset 이라 세대를 올린다 (docs/CONTRACTS.md C6·C9)
    #[test]
    fn closed_file_replaced_with_same_or_longer_content_rebuilds_with_new_generation() {
        for extra in [0, 100] {
            let e = env();
            let (mut h, file, sites, now) = dozing_wsl_team(&e);
            probe_now(&h, &sites);
            e.at(now + secs(1));
            h.tick();
            assert!(h.files[&file].reader.is_none());
            let len = fs::metadata(&file).unwrap().len();
            let text = replace_with_len(&file, len + extra, "교체");
            probe_now(&h, &sites);
            e.at(now + secs(30));
            h.tick();
            assert_eq!(
                e.team(REJECT).props.boss_visit.as_deref(),
                Some(text.as_str())
            );
            assert_eq!(h.files[&file].generation(), 1, "늘어난 길이 {extra}");
        }
    }

    /// 닫아 둔 동안 줄이 덧붙기만 했으면 Reset 이 아니라 세대를 그대로 두고 다시 만든다
    #[test]
    fn closed_file_appended_rebuilds_with_same_generation() {
        let e = env();
        let (mut h, file, sites, now) = dozing_wsl_team(&e);
        probe_now(&h, &sites);
        e.at(now + secs(1));
        h.tick();
        assert!(h.files[&file].reader.is_none());
        append(&file, &[user_line(REJECT, t0() + secs(151), "덧붙임")]);
        probe_now(&h, &sites);
        e.at(now + secs(30));
        h.tick();
        assert_eq!(e.team(REJECT).props.boss_visit.as_deref(), Some("덧붙임"));
        assert_eq!(h.files[&file].generation(), 0);
    }

    /// 메인의 Reset 을 본 뒤 서브 핸들 에러로 빠져도 Reset 을 잃지 않는다. 다시 만들 때 메인 세대가 오른다
    #[test]
    fn reset_seen_before_handle_error_still_bumps_generation_on_rebuild() {
        let e = env();
        let root = e.dir("wsl/projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/sync-subagent"), &p1);
        let main = p1.join(format!("{SYNC}.jsonl"));
        let sub = p1
            .join(SYNC)
            .join("subagents")
            .join(format!("agent-{SYNC_SUB}.jsonl"));
        let now = t0() + secs(300);
        append(&main, &[user_line(SYNC, now - secs(1), "일하는 중")]);
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        let mut h = e.host(wsl(), Vec::new());
        probe_now(&h, &sites);
        e.at(now);
        h.tick();
        assert!(h.files[&sub].reader.is_some());
        // 메인은 짧게 다시 쓰이고, 서브는 늘었는데 다른 프로세스가 잠가 핸들 읽기가 실패한다
        fs::write(
            &main,
            user_line(SYNC, now + secs(1), "새 내용")
                + "
",
        )
        .unwrap();
        append(
            &sub,
            &[rec(
                SYNC,
                Some(SYNC_SUB),
                now + secs(1),
                "user",
                serde_json::json!("잠김"),
            )],
        );
        let lock = fs::File::open(&sub).unwrap();
        lock.lock().unwrap();
        e.at(now + secs(2));
        h.tick();
        assert!(h.heartbeat.suspect.load(Ordering::Relaxed));
        assert!(e.w().read_failed_teams.is_empty());
        drop(lock);
        probe_now(&h, &sites);
        e.at(now + secs(30));
        h.tick();
        assert_eq!(e.team(SYNC).props.boss_visit.as_deref(), Some("새 내용"));
        assert_eq!(h.files[&main].generation(), 1);
    }

    #[test]
    fn wsl_listing_failure_stops_further_path_access() {
        let e = env();
        let missing = e.tmp.path().join("없음");
        let root = e.dir("wsl/projects");
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let mut h = e.host(wsl(), Vec::new());
        probe_now(
            &h,
            &[
                site_of(&wsl(), Vendor::Claude, &missing),
                site_of(&wsl(), Vendor::Codex, &root),
            ],
        );
        e.at(t0() + secs(150));
        h.tick();
        assert!(h.heartbeat.suspect.load(Ordering::Relaxed));
        assert_eq!(
            h.path_ops, 1,
            "첫 나열 실패 뒤 다음 Site 를 건드리지 않는다"
        );
        assert!(e.w().suspect_hosts.contains("wsl:Test"));
    }

    #[test]
    fn partial_listing_keeps_failed_project_and_discovers_healthy_project() {
        let e = env();
        let root = e.dir("projects");
        let blocked = root.join("blocked");
        copy_dir(&fixture("claude/reject"), &blocked);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        assert_eq!(e.w().teams.len(), 1);
        copy_dir(&fixture("claude/sync-subagent"), &root.join("healthy"));
        for n in 1..=3 {
            h.scan_listing(now + secs(n * 3), |site, ops| {
                list_site_with(site, ops, &mut |path, missing_ok, ops| {
                    if path == blocked {
                        Err(io::Error::from(io::ErrorKind::PermissionDenied))
                    } else {
                        entries(path, missing_ok, ops)
                    }
                })
            });
        }
        assert_eq!(e.w().teams.len(), 2);
        assert!(h.files.keys().any(|p| p.starts_with(&blocked)));
        assert!(
            h.files
                .values()
                .filter(|t| t.key.session == REJECT)
                .all(|t| t.missing == 0)
        );
        assert_eq!(e.team(SYNC).id, SYNC);
        fs::remove_dir_all(&blocked).unwrap();
        h.scan(now + secs(12));
        h.scan(now + secs(15));
        assert_eq!(e.w().teams.len(), 1);
    }

    #[test]
    fn codex_partial_listing_continues_after_unreadable_month() {
        let e = env();
        let root = e.dir("sessions");
        let blocked = root.join("2026/01");
        fs::create_dir_all(&blocked).unwrap();
        copy_dir(&fixture("codex/codex-app"), &root.join("2026/02/01"));
        let listing = list_site_with(
            &site_of(&win(), Vendor::Codex, &root),
            &mut 0,
            &mut |path, missing_ok, ops| {
                if path == blocked {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    entries(path, missing_ok, ops)
                }
            },
        )
        .unwrap();
        assert_eq!(listing.files.len(), 1);
        assert_eq!(listing.failed_subtrees, vec![blocked]);
    }

    /// `\\wsl.localhost\X` 와 `\\wsl$\X` 처럼 같은 폴더의 다른 경로로 루트가 바뀌어도 줄을 두 번 적용하지 않는다
    #[test]
    fn site_root_change_does_not_apply_lines_twice() {
        let e = env();
        // `\\?\` 경로는 `/` 를 받지 않는다
        let root = e.dir("wsl").join("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let verbatim = PathBuf::from(format!(r"\\?\{}", root.display()));
        let mut h = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        probe_now(&h, &[site_of(&wsl(), Vendor::Claude, &root)]);
        e.at(now);
        h.tick();
        append(&file, &[user_line(REJECT, t0() + secs(140), "한 번만")]);
        // 옛 루트의 핸들이 먼저 읽는다
        e.at(now + secs(2));
        h.tick();
        assert_eq!(e.team(REJECT).props.boss_visit.as_deref(), Some("한 번만"));
        probe_now(&h, &[site_of(&wsl(), Vendor::Claude, &verbatim)]);
        e.at(now + secs(3));
        h.tick();
        e.at(now + secs(6));
        h.tick();
        assert_eq!(h.files.len(), 1);
        let got = e.team(REJECT);
        assert_eq!(got.props.boss_visit.as_deref(), Some("한 번만"));
        let boss = &got.employees[0];
        let f = env();
        let mut g = f.host(win(), vec![site_of(&win(), Vendor::Claude, &verbatim)]);
        f.at(now + secs(6));
        g.tick();
        assert_eq!(boss.counters, f.team(REJECT).employees[0].counters);
    }

    #[test]
    fn probe_turns_suspect_on_for_stopped_distro_and_off_when_running() {
        let e = env();
        let remote = e.host(wsl(), Vec::new());
        let running: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(Some(false)));
        let state = running.clone();
        let root = e.dir("wsl/projects");
        let site = site_of(&wsl(), Vendor::Claude, &root);
        let probe: ProbeFn = Box::new(move || {
            let running = *state.lock().unwrap();
            let sites = if running == Some(true) {
                vec![site.clone()]
            } else {
                Vec::new()
            };
            (
                Vec::new(),
                vec![WslDistro {
                    host: wsl(),
                    running,
                    sites,
                }],
            )
        });
        let mut h = e.host(win(), Vec::new());
        h.watch(remote.host.id.clone(), remote.heartbeat.clone());
        let mut p = Prober::new(&h, probe);
        let suspect = || remote.heartbeat.suspect.load(Ordering::Relaxed);
        let probed = || remote.heartbeat.probed.lock().unwrap().is_some();
        p.run();
        assert!(suspect() && !probed(), "꺼진 배포판은 건드리지 않는다");
        assert!(e.w().suspect_hosts.contains("wsl:Test"));
        *running.lock().unwrap() = None;
        p.run();
        assert!(suspect() && !probed(), "모름은 의심을 풀지 않는다");
        *running.lock().unwrap() = Some(true);
        let list = e.w().list_seq;
        p.run();
        assert!(!suspect() && probed());
        let w = e.w();
        assert!(w.suspect_hosts.is_empty());
        assert!(w.list_seq > list);
    }

    /// 시간 전이는 자기 호스트 팀만, 의심 중인 호스트의 팀은 멈춘다 (docs/CONTRACTS.md C4)
    #[test]
    fn tick_moves_only_own_host_teams_and_freezes_suspect_host() {
        let e = env();
        let win_root = e.dir("projects");
        let wsl_root = e.dir("wsl/projects");
        copy_dir(&fixture("claude/reject"), &win_root.join("p1"));
        copy_dir(&fixture("claude/compaction"), &wsl_root.join("p1"));
        let mut w = e.host(win(), vec![site_of(&win(), Vendor::Claude, &win_root)]);
        let mut l = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        e.at(now);
        probe_now(&l, &[site_of(&wsl(), Vendor::Claude, &wsl_root)]);
        l.tick();
        let wsl_session = "2d416c44-e80b-dfec-6627-ef150260a525";
        let frozen = e.team(wsl_session);
        assert_eq!(frozen.phase, TeamPhase::Working);
        // 의심 중에 한 시간이 지났다. Windows 스레드가 돌아도 WSL 팀은 그대로다
        l.heartbeat.suspect.store(true, Ordering::Relaxed);
        e.at(now + Duration::hours(1));
        w.tick();
        l.tick();
        assert_eq!(e.team(REJECT).phase, TeamPhase::Off);
        assert_eq!(e.team(wsl_session), frozen);
    }

    #[test]
    fn registry_session_is_picked_up_within_a_second_and_marked_alive() {
        let e = env();
        let home = e.dir("claude");
        let root = home.join("projects");
        fs::create_dir_all(root.join("p1")).unwrap();
        fs::create_dir_all(home.join("sessions")).unwrap();
        fs::write(
            home.join("sessions").join("1.json"),
            format!(r#"{{"pid":{},"sessionId":"{REJECT}"}}"#, std::process::id()),
        )
        .unwrap();
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        assert!(h.awaited.contains(REJECT));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        e.at(now + secs(1));
        h.tick();
        assert!(e.team(REJECT).alive_in_registry);
        assert!(h.awaited.is_empty());
    }

    #[test]
    fn registry_user_name_titles_the_team_until_it_leaves() {
        let e = env();
        let home = e.dir("claude");
        let root = home.join("projects");
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let reg = home.join("sessions").join("1.json");
        fs::create_dir_all(reg.parent().unwrap()).unwrap();
        let pid = std::process::id();
        fs::write(
            &reg,
            format!(r#"{{"pid":{pid},"sessionId":"{REJECT}","name":"mine","nameSource":"user"}}"#),
        )
        .unwrap();
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        assert_eq!(e.team(REJECT).title, "mine");
        e.w().outbox.clear();
        fs::write(&reg, format!(r#"{{"pid":{pid},"sessionId":"{REJECT}"}}"#)).unwrap();
        e.at(now + secs(3));
        h.tick();
        assert_ne!(e.team(REJECT).title, "mine");
        assert!(
            e.w()
                .outbox
                .iter()
                .any(|(_, _, c)| matches!(c, Cue::FrameSwap { .. }))
        );
    }

    #[test]
    fn broken_registry_file_keeps_name_and_life_for_that_scan() {
        let e = env();
        let home = e.dir("claude");
        let root = home.join("projects");
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let pid = std::process::id();
        let reg = home.join("sessions").join(format!("{pid}.json"));
        fs::create_dir_all(reg.parent().unwrap()).unwrap();
        fs::write(
            &reg,
            format!(r#"{{"pid":{pid},"sessionId":"{REJECT}","name":"mine","nameSource":"user"}}"#),
        )
        .unwrap();
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        assert_eq!(e.team(REJECT).title, "mine");
        e.w().outbox.clear();
        // 쓰는 중이라 반쯤 잘린 파일
        fs::write(&reg, format!(r#"{{"pid":{pid},"sessionId":"#)).unwrap();
        e.at(now + secs(3));
        h.tick();
        assert_eq!(e.team(REJECT).title, "mine");
        assert!(e.team(REJECT).alive_in_registry);
        assert!(
            !e.w()
                .outbox
                .iter()
                .any(|(_, _, c)| matches!(c, Cue::FrameSwap { .. }))
        );
    }

    /// 서브 종료 10분 뒤(논리 시계) 퇴근해 자리가 비고, 서브 파일의 새 줄로 재출근한다 (docs/CONTRACTS.md C8, S60)
    #[test]
    fn sub_departs_ten_minutes_after_done_and_rehires_on_a_new_line() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/async-subagent"), &p1);
        let main = p1.join(format!("{ASYNC}.jsonl"));
        let sub = p1
            .join(ASYNC)
            .join("subagents")
            .join(format!("agent-{ASYNC_SUB}.jsonl"));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let state = |e: &Env| {
            let team = e.team(ASYNC);
            let s = team.employees.iter().find(|x| x.id == ASYNC_SUB).unwrap();
            (s.retired_at, s.departed_at, s.seat)
        };
        e.at(t0() + secs(300));
        h.tick();
        assert!(state(&e).2.is_some(), "일하는 서브는 자리가 있다");
        // 메인 파일에 그 서브의 완료 알림
        let done = t0() + secs(310);
        let note = format!(
            "<task-notification>
<task-id>{ASYNC_SUB}</task-id>
<status>completed</status>
<summary>Agent completed</summary>
<usage>total_tokens: 1</usage>
</task-notification>"
        );
        append(&main, &[user_line(ASYNC, done, &note)]);
        e.at(done);
        h.tick();
        assert_eq!(state(&e), (Some(done), None, None));
        let gone = done + Duration::minutes(10);
        e.at(gone);
        h.tick();
        assert_eq!(state(&e).1, None, "문턱 시각까지는 탕비실에 있다");
        e.at(gone + secs(1));
        h.tick();
        assert_eq!(state(&e), (Some(done), Some(gone), None));
        let back = gone + secs(2);
        append(
            &sub,
            &[rec(
                ASYNC,
                Some(ASYNC_SUB),
                back,
                "assistant",
                serde_json::json!([{"type": "text", "text": "다시 확인"}]),
            )],
        );
        e.at(back);
        h.tick();
        let (retired, departed, _) = state(&e);
        assert_eq!((retired, departed), (None, None));
    }

    #[test]
    fn spawned_sub_file_is_read_without_waiting_for_scan() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        let src = fixture("claude/async-subagent");
        let lines = lines_of(&src.join(format!("{ASYNC}.jsonl")));
        let main = p1.join(format!("{ASYNC}.jsonl"));
        append(&main, &lines[..3]);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        e.select(ASYNC);
        e.w().rebuilt.clear();
        copy_dir(&src.join(ASYNC), &p1.join(ASYNC));
        append(&main, &lines[3..]);
        e.at(now + ms(200));
        h.tick();
        {
            // 서브가 보여 다시 만든 것은 조용한 재구성이 아니다. 진행 중 연출과 출근을 끊지 않는다
            let w = e.w();
            assert!(w.rebuilt.is_empty(), "서브 추가 재구성이 rebuilt 로 나간다");
            assert!(
                w.outbox
                    .iter()
                    .any(|(.., c)| matches!(c, Cue::Enter { agent } if agent == ASYNC_SUB)),
                "새 서브의 출근이 남는다"
            );
        }
        let sub = p1
            .join(ASYNC)
            .join("subagents")
            .join(format!("agent-{ASYNC_SUB}.jsonl"));
        assert!(h.files.contains_key(&sub), "Spawn 을 본 틱에 등록한다");
        e.at(now + ms(400));
        h.tick();
        let team = e.team(ASYNC);
        let s = team.employees.iter().find(|x| x.id == ASYNC_SUB).unwrap();
        assert!(
            s.last_event > s.hired_at,
            "Scan(3초) 전에 서브 파일을 읽었다"
        );
    }

    #[test]
    fn local_read_failures_preserve_records_until_every_file_recovers() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/sync-subagent"), &p1);
        let main = p1.join(format!("{SYNC}.jsonl"));
        let sub = p1
            .join(SYNC)
            .join("subagents")
            .join(format!("agent-{SYNC_SUB}.jsonl"));
        let now = t0() + secs(300);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        h.scan(now);
        let key = e.key(SYNC);
        let before = e.team(SYNC);
        let initial_seq = e.w().list_seq;
        append(&main, &[user_line(SYNC, now, "메인 계속")]);
        append(
            &sub,
            &[rec(
                SYNC,
                Some(SYNC_SUB),
                now,
                "user",
                serde_json::json!("서브 계속"),
            )],
        );
        let main_lock = fs::File::open(&main).unwrap();
        let sub_lock = fs::File::open(&sub).unwrap();
        main_lock.lock().unwrap();
        sub_lock.lock().unwrap();
        h.poll_team(&key, &[main.clone(), sub.clone()], now, false);
        assert!(e.w().read_failed_teams.contains(&key));
        assert!(e.w().list_seq > initial_seq);
        assert_eq!(e.team(SYNC), before);

        drop(main_lock);
        h.poll_team(&key, std::slice::from_ref(&main), now + secs(1), false);
        assert_eq!(e.team(SYNC).props.boss_visit.as_deref(), Some("메인 계속"));
        assert!(
            e.w().read_failed_teams.contains(&key),
            "서브 실패가 남아 있다"
        );
        let before_rebuild = e.team(SYNC);
        h.try_rebuild(&key, now + secs(1), &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(
            e.team(SYNC),
            before_rebuild,
            "실패한 서브의 기록도 보존한다"
        );
        assert!(e.w().read_failed_teams.contains(&key));

        drop(sub_lock);
        let failed_seq = e.w().list_seq;
        h.try_rebuild(&key, now + secs(2), &BTreeSet::new(), &BTreeSet::new());
        assert!(!e.w().read_failed_teams.contains(&key));
        assert!(e.w().list_seq > failed_seq);
        let recovered = e.team(SYNC);
        assert_eq!(recovered.employees.len(), before.employees.len());
        let old_sub = before.employees.iter().find(|e| e.id == SYNC_SUB).unwrap();
        let new_sub = recovered
            .employees
            .iter()
            .find(|e| e.id == SYNC_SUB)
            .unwrap();
        assert!(new_sub.record.timeline.len() > old_sub.record.timeline.len());
    }

    #[test]
    fn first_read_failure_creates_no_fake_team_and_keeps_unread_sub_warning() {
        use std::os::windows::fs::OpenOptionsExt;
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/sync-subagent"), &p1);
        let main = p1.join(format!("{SYNC}.jsonl"));
        let sub = p1
            .join(SYNC)
            .join("subagents")
            .join(format!("agent-{SYNC_SUB}.jsonl"));
        let now = t0() + secs(300);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let main_lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&main)
            .unwrap();
        let sub_lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&sub)
            .unwrap();
        h.scan(now);
        assert!(e.w().teams.is_empty());
        assert!(e.w().read_failed_teams.is_empty());
        drop(main_lock);
        h.scan(now + secs(3));
        let key = e.key(SYNC);
        assert!(e.w().read_failed_teams.contains(&key));
        assert!(!h.files.contains_key(&sub));
        h.poll_team(&key, std::slice::from_ref(&main), now + secs(4), false);
        assert!(e.w().read_failed_teams.contains(&key));
        drop(sub_lock);
        h.scan(now + secs(6));
        assert!(!e.w().read_failed_teams.contains(&key));
        assert!(h.files.contains_key(&sub));
        append(
            &main,
            &[
                "{broken json".into(),
                rec(
                    SYNC,
                    None,
                    now + secs(7),
                    "assistant",
                    serde_json::json!([{
                        "type": "tool_use", "id": "unknown", "name": "FutureTool", "input": {}
                    }]),
                ),
            ],
        );
        h.poll_team(&key, std::slice::from_ref(&main), now + secs(8), false);
        assert!(!e.w().read_failed_teams.contains(&key));
    }

    #[test]
    fn failed_files_are_forgotten_when_sub_team_or_manual_root_is_removed() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/sync-subagent"), &p1);
        let main = p1.join(format!("{SYNC}.jsonl"));
        let sub = p1
            .join(SYNC)
            .join("subagents")
            .join(format!("agent-{SYNC_SUB}.jsonl"));
        let now = t0() + secs(300);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        h.scan(now);
        let key = e.key(SYNC);
        let lock = fs::File::open(&sub).unwrap();
        lock.lock().unwrap();
        h.try_rebuild(&key, now, &BTreeSet::new(), &BTreeSet::new());
        assert!(e.w().read_failed_teams.contains(&key));
        drop(lock);
        fs::remove_file(&sub).unwrap();
        fs::remove_file(sub.with_extension("meta.json")).unwrap();
        h.scan(now + secs(3));
        assert!(!e.w().read_failed_teams.contains(&key));
        assert!(e.w().teams.contains_key(&key));

        let lock = fs::File::open(&main).unwrap();
        lock.lock().unwrap();
        h.try_rebuild(&key, now, &BTreeSet::new(), &BTreeSet::new());
        assert!(e.w().read_failed_teams.contains(&key));
        drop(lock);
        fs::remove_file(&main).unwrap();
        h.scan(now + secs(6));
        h.scan(now + secs(9));
        assert!(!e.w().teams.contains_key(&key));
        assert!(!e.w().read_failed_teams.contains(&key));
        assert!(h.read_failures.is_empty());

        append(&main, &[user_line(SYNC, now, "새로 읽음")]);
        h.scan(now + secs(12));
        let lock = fs::File::open(&main).unwrap();
        lock.lock().unwrap();
        h.try_rebuild(&key, now, &BTreeSet::new(), &BTreeSet::new());
        assert!(e.w().read_failed_teams.contains(&key));
        h.sites.clear();
        h.remove_manual_roots(vec![(Vendor::Claude, root)]);
        assert!(!e.w().teams.contains_key(&key));
        assert!(!e.w().read_failed_teams.contains(&key));
        assert!(h.read_failures.is_empty());
        drop(lock);
    }

    /// 서브를 다시 못 여는 동안에도 메인의 기존 리더는 계속 읽는다
    #[test]
    fn unopenable_file_does_not_freeze_the_team() {
        use std::os::windows::fs::OpenOptionsExt;
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        let src = fixture("claude/sync-subagent");
        let main = p1.join(format!("{SYNC}.jsonl"));
        append(&main, &lines_of(&src.join(format!("{SYNC}.jsonl"))));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + secs(300);
        e.at(now);
        h.tick();
        // 서브 파일이 생겼는데 다른 프로세스가 독점으로 쥐고 있다
        copy_dir(&src.join(SYNC), &p1.join(SYNC));
        let sub = p1
            .join(SYNC)
            .join("subagents")
            .join(format!("agent-{SYNC_SUB}.jsonl"));
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&sub)
            .unwrap();
        append(&main, &[user_line(SYNC, t0() + secs(290), "서브 없이")]);
        e.at(now + secs(3));
        h.tick();
        assert_eq!(e.team(SYNC).props.boss_visit.as_deref(), Some("서브 없이"));
        assert!(!h.files.contains_key(&sub));
        drop(lock);
        e.at(now + secs(6));
        h.tick();
        assert!(
            h.files.contains_key(&sub),
            "풀리면 다음 Scan 이 다시 집는다"
        );

        // 메인이 옛 방식으로 지워져(지우기 대기) 나열에는 보이는데 열리지 않는다.
        // 새 서브 파일로 재구성이 실패해도 팀은 얼지 않고 서브의 쥔 핸들은 계속 읽는다
        delete_pending(&main);
        append(
            &sub.with_file_name("agent-b2.jsonl"),
            &[rec(
                SYNC,
                Some("b2"),
                t0() + secs(292),
                "user",
                serde_json::json!("둘째"),
            )],
        );
        e.at(now + secs(9));
        h.tick();
        assert!(h.rebuild_due.contains_key(&e.key(SYNC)));
        let key = e.key(SYNC);
        assert!(e.w().read_failed_teams.contains(&key));
        append(
            &sub,
            &[rec(
                SYNC,
                Some(SYNC_SUB),
                t0() + secs(296),
                "user",
                serde_json::json!("계속"),
            )],
        );
        e.at(now + secs(11));
        h.tick();
        let team = e.team(SYNC);
        let s = team.employees.iter().find(|x| x.id == SYNC_SUB).unwrap();
        assert_eq!(s.last_event, t0() + secs(296));
    }

    /// 다른 핸들이 남아 있는 동안 이름이 남는 옛 방식 삭제 (POSIX 삭제가 아닌 FileDispositionInfo)
    fn delete_pending(path: &Path) {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        let f = fs::OpenOptions::new()
            .access_mode(DELETE)
            .share_mode(7)
            .open(path)
            .unwrap();
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: 핸들은 f 가 살아 있는 동안 유효하고 구조체 크기를 맞춰 넘긴다
        let ok = unsafe {
            SetFileInformationByHandle(
                f.as_raw_handle(),
                FileDispositionInfo,
                (&raw const info).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        assert_ne!(ok, 0);
    }

    #[test]
    fn read_all_starts_over_when_file_is_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("{REJECT}.jsonl"));
        let lines = lines_of(&fixture(&format!("claude/reject/{REJECT}.jsonl")));
        append(&path, &lines);
        let key = TeamKey {
            host: "windows".into(),
            vendor: Vendor::Claude,
            session: REJECT.into(),
        };
        let mut t = open(&path, key.clone(), t0(), 0).unwrap();
        let first = read_all(&mut t, &path, &mut Vec::new()).unwrap();
        // 짧게 다시 쓰였다. 처음부터 다시 모아야 옛 줄이 남지 않는다
        fs::write(&path, format!("{}\n", lines[0])).unwrap();
        let again = read_all(&mut t, &path, &mut Vec::new()).unwrap();
        let mut fresh = open(&path, key, t0(), 0).unwrap();
        let want = read_all(&mut fresh, &path, &mut Vec::new()).unwrap();
        assert!(first.len() > want.len());
        assert_eq!(again.len(), want.len());
        assert!(again.iter().all(|e| e.raw_ref.generation == 1));
    }

    /// Reset 한 파일의 새 내용에 시각이 없으면 리더는 바꾸고 상태는 그대로 둔다. 다음 append 부터 적용한다
    #[test]
    fn reset_to_content_without_timestamps_keeps_state_and_stops_resetting() {
        let e = env();
        let root = e.dir("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        let before = e.team(REJECT);
        fs::write(
            &file,
            format!("{{\"type\":\"atis-latch\",\"atis\":\"\",\"sessionId\":\"{REJECT}\"}}\n"),
        )
        .unwrap();
        e.at(now + secs(2));
        h.tick();
        assert_eq!(e.team(REJECT).started, before.started);
        let r = h.files.get_mut(&file).unwrap().reader.as_mut().unwrap();
        assert!(
            !r.changed().unwrap(),
            "같은 Reset 을 매 폴링 되풀이하지 않는다"
        );
        append(&file, &[user_line(REJECT, t0() + secs(145), "이어서")]);
        e.at(now + secs(4));
        h.tick();
        let after = e.team(REJECT);
        assert_eq!(after.props.boss_visit.as_deref(), Some("이어서"));
        assert_eq!(after.started, before.started);
    }

    /// Probe 직후 틱의 핸들 에러는 배포판 재시작으로 죽은 핸들이라 보고 경로로 다시 연다. 의심으로 두지 않는다
    #[test]
    fn handle_error_right_after_probe_reopens_by_path() {
        let e = env();
        let root = e.dir("wsl/projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let mut h = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        probe_now(&h, &[site_of(&wsl(), Vendor::Claude, &root)]);
        e.at(now);
        h.tick();
        let key = e.key(REJECT);
        append(&file, &[user_line(REJECT, t0() + secs(140), "재시작 뒤")]);
        let err = io::Error::from_raw_os_error(87);
        h.on_wsl_error(&key, &file, &err, now + secs(1), true, &BTreeSet::new());
        assert!(!h.heartbeat.suspect.load(Ordering::Relaxed));
        assert_eq!(
            e.team(REJECT).props.boss_visit.as_deref(),
            Some("재시작 뒤")
        );
        // Probe 밖이면 의심으로 둔다
        h.on_wsl_error(&key, &file, &err, now + secs(2), false, &BTreeSet::new());
        assert!(h.heartbeat.suspect.load(Ordering::Relaxed));
    }

    /// Reset 재구성이 경로 접근 불가로 미뤄졌다 다음 Scan 에서 성공해도 조용한 재구성(rebuilt)으로 나간다
    #[test]
    fn deferred_reset_rebuild_stays_quiet_when_it_finally_succeeds() {
        let e = env();
        let root = e.dir("wsl/projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        copy_dir(&fixture("claude/reject"), &root.join("p1"));
        let mut h = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        let sites = [site_of(&wsl(), Vendor::Claude, &root)];
        probe_now(&h, &sites);
        e.at(now);
        h.tick();
        e.select(REJECT);
        // 파일이 짧게 다시 쓰였다. Probe 가 없는 틱은 Reset 만 보고 재구성은 미룬다
        fs::write(
            &file,
            user_line(REJECT, t0() + secs(145), "새 내용")
                + "
",
        )
        .unwrap();
        e.at(now + secs(1));
        h.tick();
        assert!(h.rebuild_due[&e.key(REJECT)].quiet);
        e.w().rebuilt.clear();
        probe_now(&h, &sites);
        e.at(now + secs(2));
        h.tick();
        assert!(h.rebuild_due.is_empty());
        assert!(
            !e.w().rebuilt.is_empty(),
            "미뤄졌어도 Reset 이었음을 잃는다"
        );
        // 새 내용은 새 세대로 기록돼야 옛 커서가 같은 바이트 자리의 새 줄에 붙지 않는다
        assert_eq!(h.files[&file].generation(), 1);
        let team = e.team(REJECT);
        assert!(
            team.employees[0]
                .record
                .timeline
                .iter()
                .all(|m| m.generation == 1)
        );
    }

    #[test]
    fn pace_slows_after_five_slow_ticks_and_recovers_after_five_fast() {
        let mut p = Pace {
            slow: false,
            run: 0,
        };
        let slow = StdDuration::from_millis(250);
        let fast = StdDuration::from_millis(10);
        for _ in 0..4 {
            assert_eq!(p.next("h", slow), StdDuration::ZERO);
        }
        assert_eq!(p.next("h", slow), StdDuration::from_millis(150));
        assert_eq!(p.next("h", fast), StdDuration::from_millis(390));
        for _ in 0..3 {
            p.next("h", fast);
        }
        assert_eq!(p.next("h", fast), StdDuration::from_millis(190));
    }

    /// Codex rollout 의 첫 줄 session_meta. `extra` 를 payload 에 덧붙인다
    fn codex_meta(id: &str, extra: serde_json::Value) -> String {
        let mut p = serde_json::json!({"id": id, "cwd": "C:\\dir1",
                                       "originator": "Codex Desktop", "source": "vscode"});
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        serde_json::json!({"timestamp": "2026-03-02T09:00:00Z", "ordinal": 0,
                           "type": "session_meta", "payload": p})
        .to_string()
    }

    fn rollout(root: &Path, id: &str) -> PathBuf {
        root.join("2026/03/02")
            .join(format!("rollout-2026-03-02T09-00-00-{id}.jsonl"))
    }

    #[test]
    fn codex_child_and_internal_rollouts_make_no_team_and_are_not_reread() {
        const PARENT: &str = "11111111-1111-4111-8111-111111111111";
        const CHILD: &str = "22222222-2222-4222-8222-222222222222";
        const GUARD: &str = "33333333-3333-4333-8333-333333333333";
        let parent = codex_meta(PARENT, serde_json::json!({}));
        let e = env();
        let root = e.dir("sessions");
        append(
            &rollout(&root, CHILD),
            &[codex_meta(
                CHILD,
                serde_json::json!({"parent_thread_id": PARENT, "thread_source": "subagent",
                    "subagent_history_start_ordinal": 1,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": PARENT, "depth": 1}}}}),
            )],
        );
        append(
            &rollout(&root, GUARD),
            &[codex_meta(
                GUARD,
                serde_json::json!({"source": {"subagent": {"other": "guardian"}}}),
            )],
        );
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Codex, &root)]);
        let mut now = t0() + secs(300);
        e.at(now);
        h.tick();
        assert!(e.w().teams.is_empty());

        append(&rollout(&root, PARENT), std::slice::from_ref(&parent));
        now += secs(3);
        e.at(now);
        h.tick();
        let sessions: Vec<String> = e.w().teams.keys().map(|k| k.session.clone()).collect();
        assert_eq!(sessions, [PARENT]);
        assert_eq!(h.files.len(), 1);

        // 다음 Scan 의 경로 접근은 부모만 있는 Site 와 같다: 자식·내부 파일을 다시 열지 않는다
        let c = env();
        let croot = c.dir("sessions");
        append(&rollout(&croot, PARENT), &[parent]);
        let mut ch = c.host(win(), vec![site_of(&win(), Vendor::Codex, &croot)]);
        c.at(now);
        ch.tick();
        let (ops, cops) = (h.path_ops, ch.path_ops);
        now += secs(3);
        e.at(now);
        c.at(now);
        h.tick();
        ch.tick();
        assert_eq!(h.path_ops - ops, ch.path_ops - cops);
        assert!(e.w().teams.len() == 1 && h.files.len() == 1);
    }

    #[test]
    fn new_file_with_first_line_over_8mib_builds_team_with_both_lines() {
        let e = env();
        let root = e.dir("projects");
        let big = "x".repeat(9 * 1024 * 1024);
        append(
            &root.join("p1").join(format!("{REJECT}.jsonl")),
            &[
                user_line(REJECT, t0(), &big),
                user_line(REJECT, t0() + secs(1), "둘째 지시"),
            ],
        );
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        e.at(t0() + secs(300));
        h.tick();
        assert_eq!(e.team(REJECT).turns, 2);
    }

    /// 결재(허가 대기)가 cue 없이 끝나고, 승인 창(require_escalated)이 뜬 채 세션이 끊기는 Codex 세션 (docs/CONTRACTS.md C11).
    /// 끊긴 것은 그 호출의 반려 줄 앞에서 잘라 재생한다
    const STATE_FIXTURE: &str = "codex/ask-reject-compact";

    fn state_host(e: &Env) -> HostLoop {
        e.host(
            win(),
            vec![site_of(&win(), Vendor::Codex, &e.dir("sessions"))],
        )
    }

    /// 빈 폴더에 한 줄씩 붙이며 틱을 돈다 (실행 중 append 재생). 붙인 파일과 마지막 시각에서 40분 뒤를 돌려준다
    fn replay_state_fixture(e: &Env, h: &mut HostLoop) -> (PathBuf, DateTime<Utc>) {
        let src = fs::read_dir(fixture(STATE_FIXTURE))
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let dst = e.dir("sessions/2026/03/02").join(src.file_name().unwrap());
        let lines: Vec<String> = lines_of(&src)
            .into_iter()
            .take_while(|l| !l.contains("rejected by user"))
            .collect();
        assert!(lines.last().unwrap().contains("token_usage_record"));
        let mut last = None;
        for l in &lines {
            if let Some(t) = ts_of(l) {
                e.at(t);
                last = last.max(Some(t));
            }
            append(&dst, std::slice::from_ref(l));
            h.tick();
        }
        let end = last.unwrap() + Duration::minutes(40);
        e.at(end);
        h.tick();
        (dst, end)
    }

    /// unhandled.jsonl 의 state_conflict·scenario_gap 줄: (kind, key) → count
    fn state_lines(diag: &Mutex<Diag>, dir: &Path) -> BTreeMap<(String, String), u64> {
        diag.lock().unwrap().flush().unwrap();
        let text = fs::read_to_string(dir.join("unhandled.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| v["kind"] == "state_conflict" || v["kind"] == "scenario_gap")
            .map(|v| {
                let s = |k: &str| v[k].as_str().unwrap().to_string();
                ((s("kind"), s("key")), v["count"].as_u64().unwrap())
            })
            .collect()
    }

    #[test]
    fn fixture_replay_records_state_conflict_and_scenario_gap() {
        let e = env();
        let mut h = state_host(&e);
        replay_state_fixture(&e, &mut h);
        // 미대응 기록은 프론트 연출로 나가지 않는다
        assert!(e.w().outbox.iter().all(|(.., c)| c.shown()));
        let lines = state_lines(&e.diag, e.tmp.path());
        let own = |kind: &str, key: &str| ((kind.to_string(), key.to_string()), 1);
        assert_eq!(
            lines,
            BTreeMap::from([
                own("scenario_gap", "허가 대기>한가함@ToolResult"),
                own("state_conflict", "codex_app:퇴근>허가 대기"),
            ])
        );
    }

    #[test]
    fn state_records_are_not_counted_again_on_reset_or_restart() {
        let e = env();
        let mut h = state_host(&e);
        let (file, end) = replay_state_fixture(&e, &mut h);
        let counted = state_lines(&e.diag, e.tmp.path());
        assert_eq!(counted.len(), 2);
        // 같은 내용의 새 파일로 바꾸면 Reset 으로 팀을 처음부터 다시 만든다. 퇴근 팀은 Cold 폴링 때 본다
        let text = fs::read_to_string(&file).unwrap();
        fs::rename(&file, file.with_extension("old")).unwrap();
        fs::write(&file, text).unwrap();
        e.at(end + COLD);
        h.tick();
        assert_eq!(h.files[&file].generation(), 1);
        assert_eq!(state_lines(&e.diag, e.tmp.path()), counted);
        // 재시작: 새 world 와 다시 연 미대응 로그로 같은 파일을 처음부터 읽는다
        let world = Arc::new(Mutex::new(OfficeWorld::new(
            Scale::Large,
            NameBook::load(e.tmp.path()),
            FixedOffset::east_opt(0).unwrap(),
        )));
        let diag = Arc::new(Mutex::new(Diag::open(e.tmp.path(), false)));
        let clock = Arc::new(LogicalClock::new(end + COLD + SCAN, 0.0));
        let sites = h.sites.clone();
        let mut again = HostLoop::new(win(), sites, world.clone(), clock.clone(), diag.clone());
        again.tick();
        clock.observe(end + Duration::minutes(10));
        again.tick();
        assert_eq!(world.lock().unwrap().teams.len(), 1);
        assert_eq!(state_lines(&diag, e.tmp.path()), counted);
    }
}
