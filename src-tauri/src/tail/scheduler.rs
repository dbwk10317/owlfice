//! 호스트별 폴링 스레드 (PLAN 4.5 폴링 계층, 4.4.2 깨우지 않기)

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration as StdDuration, Instant, SystemTime};

use chrono::{DateTime, Duration, Utc};

use super::{MAX_READ_BYTES, Pending, TailReader};
use crate::diag::Diag;
use crate::discover::registry;
use crate::discover::wsl::WslDistro;
use crate::discover::{Host, HostKind, Product, Site, Vendor};
use crate::office::classify::{SubMeta, rebuild};
use crate::office::world::{OfficeId, OfficeWorld, TeamKey};
use crate::office::{Cue, TeamPhase};
use crate::parse::claude::ClaudeParser;
use crate::parse::codex::CodexParser;
use crate::parse::{Event, EventKind, Parser, RawRef, Unhandled};

const HOT: Duration = Duration::milliseconds(200);
const WARM: Duration = Duration::seconds(2);
const COLD: Duration = Duration::seconds(60);
const SCAN: Duration = Duration::seconds(3);
const PROBE: StdDuration = StdDuration::from_secs(30);
/// 등록부에 있는데 jsonl 이 아직 없는 세션을 보는 간격 (4.5 신규 세션 발견 1번)
const AWAIT: Duration = Duration::seconds(1);
/// 첫 레코드가 이보다 최근이면 방금 시작된 세션이라 출근 연출을 낸다 (4.5 신규 세션 발견 3번)
const JUST_STARTED: Duration = Duration::seconds(60);
/// 다른 호스트의 심장박동이 이만큼 멈추면 의심 (4.5 감시자)
const STALL_MS: i64 = 2000;
const TICK: StdDuration = StdDuration::from_millis(200);
const SLOW_TICK: StdDuration = StdDuration::from_millis(400);
/// 이만큼 연달아 틱이 200ms 를 넘기면 그 호스트만 늦춘다 (4.10)
const SLOW_AFTER: u32 = 5;

/// 실행 중에는 벽시계, replay·테스트에서는 논리 시계 (4.3 시계 규칙)
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

/// 마지막으로 읽은 이벤트 ts + 그 뒤 벽시계 경과 × 배속 (4.3).
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

/// 다른 스레드가 이 호스트를 지켜보고 지시하는 값 (4.5 감시자). 멈춘 I/O 안의 스레드는 자기 지연을 잴 수 없다
pub struct Heartbeat {
    /// `mono_ms` 로 잰 마지막 틱
    pub last_tick_ms: AtomicI64,
    /// 의심 상태. Probe 가 켜짐을 다시 확인할 때까지 UNC 를 건드리지 않는다 (4.4.2).
    /// 바꿀 때는 `set_suspect` 로 world 목록에도 알린다
    // ponytail: 소등과 통신 지연을 이 깃발 하나로 둔다, 사이드바가 둘을 나눠 그리는 티켓에서 나눈다
    pub suspect: AtomicBool,
    /// Probe 가 넘긴 Site. WSL 은 이것을 받은 틱만 경로 접근(나열, verify_path, 열기·닫기)을 한다 (4.4.2)
    pub probed: Mutex<Option<Vec<Site>>>,
    /// 켜면 스레드가 다음 틱 전에 끝난다
    pub stop: AtomicBool,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self {
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
    /// None 이면 WSL 졸음·퇴근 팀이라 핸들을 닫아 두었다 (4.4.2). 다음 Probe 직후 Scan 에서 바뀌었으면 다시 연다
    reader: Option<TailReader>,
    /// 핸들을 닫을 때의 (길이, mtime)
    closed: Option<(u64, SystemTime)>,
    parser: Box<dyn Parser + Send>,
    last_read: DateTime<Utc>,
    /// 연속으로 Scan 에서 못 본 횟수
    missing: u8,
    /// 서브 파일 옆 meta.json 을 마지막 Scan 에서 봤나
    has_meta: bool,
    /// 읽기 실패를 이미 알렸나. 실패가 이어져도 한 번만 남긴다
    failed: bool,
    /// 이 파일에서 지금까지 읽은 줄의 최대 ts. 실행 중 병합의 정렬 키 (4.3 순서)
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

pub struct HostLoop {
    pub host: Host,
    pub sites: Vec<Site>,
    files: BTreeMap<PathBuf, Tracked>,
    world: Arc<Mutex<OfficeWorld>>,
    clock: Arc<dyn Clock>,
    pub heartbeat: Arc<Heartbeat>,
    diag: Arc<Mutex<Diag>>,
    last_scan: Option<DateTime<Utc>>,
    /// 이번 틱에 난 연출 신호. `spawn` 이 틱마다 world 의 outbox 로 넘긴다
    pub cues: Vec<(OfficeId, Cue)>,
    /// 다시 만들어야 하는데 아직 못 만든 팀. 경로 접근이 되는 다음 Scan 에서 만든다. 그동안 기존 리더는 계속 읽는다
    rebuild_due: BTreeSet<TeamKey>,
    /// 파일이 없어져 퇴장시킨 직원과 그 시각. 재구성이 메인의 Spawn 으로 되살리면 다시 퇴장시킨다
    gone: BTreeMap<TeamKey, BTreeMap<String, DateTime<Utc>>>,
    /// Windows 스레드만: 감시할 다른 호스트
    remotes: Remotes,
    /// Windows Claude 만: 등록부에 살아 있는 세션과 그중 jsonl 이 아직 없는 세션 (4.5 신규 세션 발견 1번)
    alive: HashSet<String>,
    awaited: BTreeSet<String>,
    last_await: Option<DateTime<Utc>>,
    project_dirs: Vec<PathBuf>,
    /// 경로로 한 접근(나열, 열기, verify_path, stat) 수. WSL 에서는 Probe 직후 틱 밖에서 늘면 안 된다
    pub path_ops: u64,
}

impl HostLoop {
    pub fn new(
        host: Host,
        sites: Vec<Site>,
        world: Arc<Mutex<OfficeWorld>>,
        clock: Arc<dyn Clock>,
        diag: Arc<Mutex<Diag>>,
    ) -> Self {
        Self {
            host,
            sites,
            files: BTreeMap::new(),
            world,
            clock,
            heartbeat: Arc::new(Heartbeat::default()),
            diag,
            last_scan: None,
            cues: Vec::new(),
            rebuild_due: BTreeSet::new(),
            gone: BTreeMap::new(),
            remotes: Arc::new(Mutex::new(Vec::new())),
            alive: HashSet::new(),
            awaited: BTreeSet::new(),
            last_await: None,
            project_dirs: Vec::new(),
            path_ops: 0,
        }
    }

    /// 다른 호스트의 심장박동을 매 틱 본다 (4.5 감시자)
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
            eprintln!(
                "{} 의심: Probe 가 켜짐을 확인할 때까지 UNC 를 멈춘다",
                self.host.id
            );
        }
    }

    fn lock_world(&self) -> std::sync::MutexGuard<'_, OfficeWorld> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 한 틱. 읽기·파싱은 잠금 밖에서, 적용만 잠금 안에서 한다 (4.3)
    pub fn tick(&mut self) {
        let now = self.clock.now();
        self.heartbeat
            .last_tick_ms
            .store(mono_ms(), Ordering::Relaxed);
        self.cues.clear();
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
        // WSL 은 경로 접근이 Probe 직후에만 된다. 나열도 경로 기반이라 Scan 도 그때만 돈다 (4.4.2)
        let path_ok = !self.is_wsl() || probed.is_some();
        if let Some(sites) = probed {
            self.set_sites(sites);
        }
        let scan = if self.is_wsl() {
            path_ok
        } else {
            self.awaited_appeared(now) || self.last_scan.is_none_or(|t| now - t >= SCAN)
        };
        if scan {
            self.scan(now);
            self.last_scan = Some(now);
            for key in std::mem::take(&mut self.rebuild_due) {
                if self.suspect() {
                    self.rebuild_due.insert(key);
                    continue;
                }
                self.try_rebuild(&key, now, &BTreeSet::new(), &BTreeSet::new());
            }
        }
        self.poll_teams(now, path_ok);
        if self.is_wsl() && path_ok && !self.suspect() {
            self.close_idle(now);
        }
        if self.suspect() {
            return;
        }
        let cues = self.lock_world().tick(now, &self.host.id);
        self.cues.extend(cues);
        let names = self.lock_world().unsaved_names();
        if let Some(Err(e)) = names.map(|n| n.write()) {
            // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
            eprintln!("이름 저장 실패: {e}");
        }
        if let Err(e) = self
            .diag
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush_if_due(now)
        {
            eprintln!("미대응 로그 쓰기 실패: {e}");
        }
    }

    /// Probe 가 넘긴 Site 로 바꾼다. 같은 배포판의 루트가 `\\wsl.localhost` 와 `\\wsl$` 를 오가면
    /// 옛 루트의 리더가 남아 같은 줄이 두 번 적용되므로, 없어진 루트 아래 리더를 버리고 새 루트에서 다시 찾는다
    fn set_sites(&mut self, sites: Vec<Site>) {
        let roots: Vec<&PathBuf> = sites.iter().map(|s| &s.root).collect();
        self.files
            .retain(|p, _| roots.iter().any(|r| p.starts_with(r)));
        self.sites = sites;
    }

    /// 선택 사무실의 팀은 Hot, 퇴근 팀은 Cold, 나머지 Warm (4.5 표).
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

    /// 때가 된 팀의 파일을 팀 단위로 읽는다. 한 팀의 여러 파일은 한 번에 병합해 적용한다 (4.3 순서)
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
            // 퇴근 팀은 매 Scan 대신 Cold 폴링 때 바꿔치기를 본다 (4.4.2, 실제 기록 수백 개에 매 3초 열기를 피한다)
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

    /// 한 팀의 파일들을 읽어 4.3 순서로 합쳐 한 번 적용하고, 적용이 끝난 뒤에만 모두 commit 한다 (4.5 2단계 commit)
    fn poll_team(&mut self, key: &TeamKey, paths: &[PathBuf], now: DateTime<Utc>, path_ok: bool) {
        let mut batch: Vec<(PathBuf, Pending)> = Vec::new();
        let mut reset = BTreeSet::new();
        let wsl = self.is_wsl();
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
                        // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
                        eprintln!("64MB 넘는 줄 버림 {}", p.display());
                    }
                    t.failed = false;
                    batch.push((p.clone(), pending));
                }
                Ok(None) => t.failed = false,
                Err(e) => {
                    if wsl {
                        self.on_wsl_error(key, p, &e, now, path_ok);
                        return;
                    }
                    // 로컬 파일 하나의 실패로 팀 전체를 멈추지 않는다. 다음 폴링에 다시 본다
                    if !t.failed {
                        t.failed = true;
                        eprintln!("읽기 실패 {}: {e}", p.display());
                    }
                }
            }
        }
        // 멈췄던 I/O 의 결과는 버리고 commit 하지 않는다. 재개한 첫 틱이 같은 구간을 다시 읽는다 (4.5 감시자).
        // 파서에 넣기 전에 본다. 넣고 버리면 파서가 같은 줄을 두 번 본다
        if self.suspect() {
            return;
        }
        if !reset.is_empty() {
            // 어느 파일의 Reset 이든 팀 전체를 다시 만든다 (4.3 범위). 다른 파일의 이번 읽기는 재구성이 다시 읽는다
            for p in &reset {
                eprintln!("Reset {}", p.display());
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
            self.rebuild_due.insert(key.clone());
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
        let (cues, product, office) = {
            let mut w = self.lock_world();
            let cues = if events.is_empty() {
                Vec::new()
            } else {
                w.apply(key, &events, now)
            };
            let product = w.teams.get(key).map(|t| t.product);
            (cues, product, w.office_of(key))
        };
        for ((p, pending), max) in batch.into_iter().zip(maxes) {
            if let Some(t) = self.files.get_mut(&p) {
                t.max_ts = max;
                if let Some(r) = t.reader.as_mut() {
                    r.commit(pending);
                }
            }
        }
        self.cues
            .extend(cues.into_iter().map(|c| (office.clone(), c)));
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
    /// 아니면 꺼졌거나 공유가 끊긴 것이라 의심으로 두고, Probe 가 켜짐을 본 뒤 다시 만든다 (4.4.2)
    fn on_wsl_error(
        &mut self,
        key: &TeamKey,
        p: &Path,
        e: &io::Error,
        now: DateTime<Utc>,
        path_ok: bool,
    ) {
        // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
        eprintln!("읽기 실패 {}: {e}", p.display());
        if path_ok {
            self.try_rebuild(key, now, &BTreeSet::new(), &BTreeSet::new());
            return;
        }
        self.mark_suspect();
        self.rebuild_due.insert(key.clone());
    }

    /// Probe 직후 틱에 근무 중이 아닌 WSL 팀의 핸들을 닫는다 (4.4.2). 열어 둔 핸들은 배포판이 스스로 꺼지지 못하게 하고,
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
            for p in &paths {
                self.path_ops += 1;
                let Ok(m) = fs::metadata(p) else {
                    continue;
                };
                let Some(t) = self.files.get_mut(p) else {
                    continue;
                };
                if t.stale {
                    continue;
                }
                t.generation = t.generation();
                t.closed = Some((m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
                t.reader = None;
            }
        }
    }

    /// Spawn 결과로 안 서브 파일은 Scan 을 기다리지 않고 바로 읽는다. S19 가 끊기지 않게 (4.5 서브 파일 발견).
    /// 서브 파일에는 Spawn 보다 이른 줄이 있어 이어 읽지 않고 팀을 다시 만든다 (4.3 직원 사전 등록).
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

    /// 실패하면 기존 상태를 그대로 두고 다음 Scan 에 다시 한다
    fn try_rebuild(
        &mut self,
        key: &TeamKey,
        now: DateTime<Utc>,
        new: &BTreeSet<PathBuf>,
        reset: &BTreeSet<PathBuf>,
    ) {
        if let Err(e) = self.rebuild_team(key, now, new, reset) {
            // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
            eprintln!("팀 재구성 실패 {}: {e}", key.session);
            if self.is_wsl() {
                self.mark_suspect();
            }
            // 새 팀이면 파일이 아직 등록 전이라 다음 Scan 이 새 파일로 다시 찾는다
            if self.files.values().any(|t| &t.key == key) {
                self.rebuild_due.insert(key.clone());
            }
        }
    }

    /// 팀의 파일을 모두 처음부터 다시 읽어 만든다 (새 세션, Reset. 4.3 재구성 규칙).
    /// `new` 는 이번에 처음 찾은 파일, `reset` 은 Reset 을 본 파일이다 (세대를 하나 올린다).
    /// 서브 파일 하나가 안 열리면 그 파일만 "못 봄"으로 빼고 나머지로 만든다. 메인이 안 열리면 기존 상태를 둔다.
    /// 방금 시작된 새 파일이면 출근 cue 를 내고 미대응 재료를 센다. 늦게 발견한 옛 파일은 세지 않는다 (재시작마다 다시 세지 않게)
    fn rebuild_team(
        &mut self,
        key: &TeamKey,
        now: DateTime<Utc>,
        new: &BTreeSet<PathBuf>,
        reset: &BTreeSet<PathBuf>,
    ) -> io::Result<()> {
        let mut paths: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, t)| &t.key == key)
            .map(|(p, _)| p.clone())
            .chain(new.iter().cloned())
            .collect();
        // 메인 파일이 먼저다 (4.3 순서)
        paths.sort_by_key(|p| (is_subagent(p), p.clone()));
        paths.dedup();
        if paths.is_empty() {
            return Ok(());
        }
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
                Ok(r) => r,
                Err(e) if is_subagent(path) && !self.is_wsl() => {
                    eprintln!("서브 파일 열기 실패, 빼고 재구성 {}: {e}", path.display());
                    unseen.push(path.clone());
                    continue;
                }
                Err(e) => return Err(e),
            };
            if let Some(o) = self.files.get(path) {
                t.missing = o.missing;
            }
            self.path_ops += 1;
            t.has_meta = is_subagent(path) && path.with_extension("meta.json").is_file();
            t.max_ts = events.iter().filter_map(|e| e.ts).max();
            last_ts = last_ts.max(t.max_ts);
            if new.contains(path) && self.just_started(path, &events, now) {
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
        let exists = self.lock_world().teams.contains_key(key);
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
        team.alive_in_registry = self.alive.contains(&key.session);
        let product = team.product;
        let boss = team.employees[0].id.clone();
        let gone = self.gone.get(key).cloned().unwrap_or_default();
        let (office, known) = {
            let mut w = self.lock_world();
            // Spawn 을 먼저 적용해 이미 출근한 서브는 다시 출근하지 않는다
            let known: HashSet<String> = w
                .teams
                .get(key)
                .map(|t| t.employees.iter().map(|e| e.id.clone()).collect())
                .unwrap_or_default();
            w.put_team(key.clone(), team, now);
            // 파일이 없어 퇴장시킨 직원을 메인의 Spawn 이 되살렸으면 조용히 다시 퇴장시킨다
            for (agent, at) in &gone {
                w.retire_employee(key, agent, *at);
            }
            (w.office_of(key), known)
        };
        self.install(fresh, &unseen);
        if let Some(ts) = last_ts {
            self.clock.observe(ts);
        }
        for path in arrived {
            let agent = if is_subagent(&path) {
                agent_of(&path).unwrap_or_default()
            } else {
                boss.clone()
            };
            if !known.contains(&agent) {
                self.cues.push((office.clone(), Cue::Enter { agent }));
            }
        }
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

    /// 첫 레코드 ts(없으면 파일 생성 시각)가 지금부터 60초 안인가 (4.5 신규 세션 발견 3번)
    fn just_started(&mut self, path: &Path, events: &[Event], now: DateTime<Utc>) -> bool {
        let first = events.iter().find_map(|e| e.ts).or_else(|| {
            self.path_ops += 1;
            let created = fs::metadata(path).ok()?.created().ok()?;
            Some(DateTime::<Utc>::from(created))
        });
        first.is_some_and(|t| now - t < JUST_STARTED)
    }

    /// 새 세션 파일·서브 파일·meta.json 과 사라진 파일을 찾는다 (4.5 신규 세션 발견).
    /// 접근에 실패한 Site 의 파일은 "없음"이 아니라 "못 봄"이라 아무것도 지우지 않는다.
    /// 경로 접근마다 앞에서 의심을 보고 멈춘다
    fn scan(&mut self, now: DateTime<Utc>) {
        if !self.is_wsl() {
            self.read_registry();
        }
        let mut seen = HashSet::new();
        let mut metas = HashSet::new();
        let mut ok_roots = Vec::new();
        let mut project_dirs = Vec::new();
        let mut found: BTreeMap<TeamKey, BTreeSet<PathBuf>> = BTreeMap::new();
        for site in self.sites.clone() {
            if self.suspect() {
                return;
            }
            let listing = match list_site(&site, &mut self.path_ops) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("세션 폴더 나열 실패 {}: {e}", site.root.display());
                    if self.is_wsl() {
                        // Probe 직후인데 나열이 안 된다. 배포판이 꺼졌거나 공유가 끊겼다
                        self.mark_suspect();
                        return;
                    }
                    continue;
                }
            };
            ok_roots.push(site.root.clone());
            metas.extend(listing.metas);
            project_dirs.extend(listing.projects);
            for (path, session) in listing.files {
                if !self.files.contains_key(&path) {
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

        let tiers = self.tiers();
        let mut gone_teams = BTreeSet::new();
        let mut gone_subs = Vec::new();
        let mut meta_gone = Vec::new();
        let mut rebuild = BTreeSet::new();
        let mut verify = Vec::new();
        for (path, t) in &mut self.files {
            if !ok_roots.iter().any(|r| path.starts_with(r)) {
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
                    rebuild.insert(t.key.clone());
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
        // 로컬은 매 Scan, WSL 은 Probe 직후 Scan 에 근무 중·졸음 팀 파일만 경로의 파일과 핸들의 파일을 대조한다 (4.4.2).
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
                (None, Some(stat)) => fs::metadata(&path).map(|m| {
                    if (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)) != stat {
                        rebuild.insert(t.key.clone());
                    }
                }),
                (None, None) => Ok(()),
            };
            if let Err(e) = r
                && wsl
            {
                // 나열에는 보이는데 열리지 않는다. 그 배포판의 경로를 더 건드리지 않는다
                eprintln!("경로 대조 실패 {}: {e}", path.display());
                self.mark_suspect();
                return;
            }
            // 로컬 실패(그새 지워짐 등)는 다음 Scan 의 없음 판정이 맡는다
        }

        for key in &gone_teams {
            self.files.retain(|_, t| &t.key != key);
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
            rebuild.remove(key);
            for p in new {
                self.revive(key, p);
            }
            self.try_rebuild(key, now, new, &BTreeSet::new());
        }
        for key in rebuild {
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
            self.try_rebuild(&key, now, &BTreeSet::new(), &BTreeSet::new());
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
                .iter()
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
        let (cues, office) = {
            let mut w = self.lock_world();
            (w.retire_employee(key, &agent, now), w.office_of(key))
        };
        self.cues
            .extend(cues.into_iter().map(|c| (office.clone(), c)));
    }

    /// Windows Claude 세션 등록부 (4.4.1): 살아 있는 세션을 팀에 표시하고, jsonl 이 아직 없는 세션을 기다린다
    fn read_registry(&mut self) {
        let home = self
            .sites
            .iter()
            .find(|s| s.vendor == Vendor::Claude)
            .and_then(|s| s.root.parent())
            .map(Path::to_path_buf);
        if home.is_some() {
            self.path_ops += 1;
        }
        self.alive = home
            .map(|h| registry::live_sessions(&h))
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.session_id)
            .filter(|id| safe_id(id))
            .collect();
        let mut w = self.world.lock().unwrap_or_else(|e| e.into_inner());
        for (k, t) in w.teams.iter_mut() {
            if k.host == self.host.id && k.vendor == Vendor::Claude {
                t.alive_in_registry = self.alive.contains(&k.session);
            }
        }
    }

    /// 기다리는 세션의 jsonl 이 생겼나. 그 경로만 1초 간격으로 본다 (4.5 신규 세션 발견 1번).
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

    /// 2초 넘게 멈춘 호스트를 의심으로 바꾼다. 멈춘 I/O 는 취소하지 않는다 (4.5 감시자)
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
                eprintln!("{id} 스레드가 멈췄다. Probe 가 켜짐을 확인할 때까지 의심");
            }
        }
    }
}

/// 30초마다 Windows Site 와 WSL 배포판의 켜짐·꺼짐을 각 호스트에 넘긴다 (4.4.2).
/// `wsl.exe` 와 켜진 배포판의 UNC 를 쓰므로 자기 스레드에서 돈다. 멈춰도 Windows 갱신과 감시자는 계속된다 (4.5)
pub struct Prober {
    probe: ProbeFn,
    windows: Arc<Heartbeat>,
    remotes: Remotes,
    world: Arc<Mutex<OfficeWorld>>,
    clock: Arc<dyn Clock>,
    diag: Arc<Mutex<Diag>>,
    threads: Vec<(Arc<Heartbeat>, thread::JoinHandle<()>)>,
}

impl Prober {
    /// Windows 스레드와 감시 목록·world 를 함께 쓴다
    pub fn new(windows: &HostLoop, probe: ProbeFn) -> Self {
        Self {
            probe,
            windows: windows.heartbeat.clone(),
            remotes: windows.remotes.clone(),
            world: windows.world.clone(),
            clock: windows.clock.clone(),
            diag: windows.diag.clone(),
            threads: Vec::new(),
        }
    }

    /// 한 번 본다. 꺼진 배포판은 UNC 를 건드리지 않고, 의심 상태를 푸는 것도 여기서만 한다
    pub fn run(&mut self) {
        let (local, distros) = (self.probe)();
        // Windows 는 언제나 경로 접근을 하니 Site 만 바꾼다
        *self
            .windows
            .probed
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(local);
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
                        *hb.probed.lock().unwrap_or_else(|e| e.into_inner()) = Some(d.sites);
                        set_suspect(&self.world, &d.host.id, &hb, false);
                    }
                    Some(false) => {
                        set_suspect(&self.world, &d.host.id, &hb, true);
                    }
                    // wsl.exe 실패는 모름이다. 핸들 읽기는 이어 가고 경로 접근만 안 한다
                    None => {}
                }
            } else if d.running == Some(true) && !d.sites.is_empty() {
                let h = HostLoop::new(
                    d.host.clone(),
                    Vec::new(),
                    self.world.clone(),
                    self.clock.clone(),
                    self.diag.clone(),
                );
                *h.heartbeat.probed.lock().unwrap_or_else(|e| e.into_inner()) = Some(d.sites);
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
                thread::sleep(TICK);
            }
        }
        for (hb, t) in p.threads.drain(..) {
            hb.stop.store(true, Ordering::Relaxed);
            let _ = t.join();
        }
    })
}

/// 틱 간격. 연달아 5번 200ms 를 넘기면 그 호스트만 400ms 로 늦추고, 5번 연달아 안 넘기면 되돌린다 (4.10)
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
            // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
            eprintln!(
                "{host} 틱 {}ms 로 {}",
                if over { 400 } else { 200 },
                if over { "늦춤" } else { "되돌림" }
            );
        }
        let every = if self.slow { SLOW_TICK } else { TICK };
        every.saturating_sub(spent)
    }
}

/// 호스트 하나의 루프를 스레드로 돌린다. 한 호스트가 UNC 에서 멈춰도 다른 호스트는 돈다 (4.5).
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
            if !host.cues.is_empty() {
                let cues = std::mem::take(&mut host.cues);
                host.lock_world().outbox.extend(cues);
            }
            let spent = start.elapsed();
            if spent > TICK {
                eprintln!("{} 틱 {}ms", host.host.id, spent.as_millis());
            }
            thread::sleep(pace.next(&host.host.id, spent));
        }
        // 꺼진 배포판의 핸들은 닫기만 해도 배포판을 다시 켠다 (4.4.2). 의심 중이면 닫지 않고 둔다
        if host.suspect() {
            std::mem::forget(host);
        }
    })
}

/// 한 틱에 같은 팀의 여러 파일에서 읽은 이벤트를 적용 순서로 늘어놓는다 (4.3 순서, `classify::merge` 와 같은 규칙).
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
        key,
        last_read,
        missing: 0,
        has_meta: false,
        failed: false,
        max_ts: None,
        generation,
        stale: false,
    })
}

/// 재구성용으로 끝까지 읽는다. 8MB 상한이 있어 backlog 가 남으면 이어 읽는다 (4.5).
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
    loop {
        let pending = reader.read_new_lines(MAX_READ_BYTES)?;
        if pending.reset {
            events.clear();
            unhandled.clear();
            t.parser = make_parser(path, t.key.vendor);
        } else if pending.lines.is_empty() {
            reader.commit(pending);
            return Ok(events);
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
    }
}

pub fn is_subagent(p: &Path) -> bool {
    p.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "subagents")
}

/// Codex 세션 id 는 파일명 끝 UUID 36자 (4.6.2)
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

/// 옆의 `agent-<id>.meta.json` (4.6.1 서브에이전트)
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
struct Listing {
    /// 세션 파일과 그 세션 id
    files: Vec<(PathBuf, String)>,
    /// 서브 파일 옆 `*.meta.json`
    metas: HashSet<PathBuf>,
    /// Claude 프로젝트 폴더 (등록부 세션 기다리기용)
    projects: Vec<PathBuf>,
}

/// Site 아래 세션 파일과 그 세션 id. Claude 는 `<프로젝트>/<uuid>.jsonl` 과 `<프로젝트>/<uuid>/subagents/*.jsonl`,
/// Codex 는 `YYYY/MM/DD/rollout-*.jsonl` (4.4.1). 어디서든 나열이 실패하면 Site 전체를 "못 봄"으로 돌려준다
fn list_site(site: &Site, ops: &mut u64) -> io::Result<Listing> {
    let mut out = Listing::default();
    match site.vendor {
        Vendor::Claude => {
            for project in dirs(&site.root, false, ops)? {
                for f in files(&project, ops)? {
                    if f.extension().is_none_or(|e| e != "jsonl") {
                        continue;
                    }
                    let Some(id) = f.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                        continue;
                    };
                    for sub in files(&project.join(&id).join("subagents"), ops)? {
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
            for y in dirs(&site.root, false, ops)? {
                for m in dirs(&y, true, ops)? {
                    for d in dirs(&m, true, ops)? {
                        for f in files(&d, ops)? {
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

/// 폴더 안 (경로, 폴더인가). 종류는 나열 결과에서 읽어 항목마다 stat 하지 않는다 (4.10 Scan 예산).
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

fn dirs(p: &Path, missing_ok: bool, ops: &mut u64) -> io::Result<Vec<PathBuf>> {
    Ok(entries(p, missing_ok, ops)?
        .into_iter()
        .filter(|e| e.1)
        .map(|e| e.0)
        .collect())
}

fn files(p: &Path, ops: &mut u64) -> io::Result<Vec<PathBuf>> {
    Ok(entries(p, true, ops)?
        .into_iter()
        .filter(|e| !e.1)
        .map(|e| e.0)
        .collect())
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
        *h.heartbeat.probed.lock().unwrap() = Some(sites.to_vec());
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
            let entered = h
                .cues
                .iter()
                .any(|(_, c)| matches!(c, Cue::Enter { agent } if agent == REJECT));
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
        assert!(h.cues.is_empty(), "조용한 재구성에서 cue: {:?}", h.cues);
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

    /// 퇴근 팀은 매 Scan 대신 Cold 폴링 때 바꿔치기를 본다 (4.4.2 D3)
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
    /// 같은 논리 시점에서 팀 상태 전체가 같다 (4.3 시계·순서)
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
            // (파일 상대 경로, 줄, ts) 를 4.3 병합 순서(파일별 누적 최대 ts, 메인 먼저, 줄 순서)로
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

    /// 한 틱에 메인과 서브에 함께 붙은 줄은 4.3 순서로 합쳐 적용한다. 파일마다 따로 적용하면
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
    /// 결과보다 30분 포기가 먼저 일어나야 한다 (4.3 재구성 규칙, 4.7.2)
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

    /// 근무 중이 아닌 WSL 팀의 핸들은 Probe 직후에 닫고, 바뀌면 다음 Probe 직후에 다시 연다 (4.4.2 D1)
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

    /// 시간 전이는 자기 호스트 팀만, 의심 중인 호스트의 팀은 멈춘다 (4.3 시계)
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
        copy_dir(&src.join(ASYNC), &p1.join(ASYNC));
        append(&main, &lines[3..]);
        e.at(now + ms(200));
        h.tick();
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

    /// 안 열리는 서브 파일만 빼고 재구성하고, 메인을 다시 못 여는 동안에도 기존 리더는 계속 읽는다
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
        assert!(h.rebuild_due.contains(&e.key(SYNC)));
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
        h.on_wsl_error(&key, &file, &err, now + secs(1), true);
        assert!(!h.heartbeat.suspect.load(Ordering::Relaxed));
        assert_eq!(
            e.team(REJECT).props.boss_visit.as_deref(),
            Some("재시작 뒤")
        );
        // Probe 밖이면 의심으로 둔다
        h.on_wsl_error(&key, &file, &err, now + secs(2), false);
        assert!(h.heartbeat.suspect.load(Ordering::Relaxed));
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
}
