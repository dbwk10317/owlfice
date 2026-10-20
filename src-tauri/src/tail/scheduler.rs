//! 호스트별 폴링 스레드 (PLAN 4.5 폴링 계층, 4.4.2 깨우지 않기)

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};

use super::{MAX_READ_BYTES, TailReader};
use crate::diag::Diag;
use crate::discover::registry;
use crate::discover::wsl::WslDistro;
use crate::discover::{Host, HostKind, Product, Site, Vendor};
use crate::office::Cue;
use crate::office::classify::{SubMeta, rebuild};
use crate::office::world::{OfficeId, OfficeWorld, TeamKey};
use crate::parse::claude::ClaudeParser;
use crate::parse::codex::CodexParser;
use crate::parse::{Event, EventKind, Parser, RawRef, Unhandled};

const HOT: Duration = Duration::milliseconds(200);
const WARM: Duration = Duration::seconds(2);
const COLD: Duration = Duration::seconds(60);
const SCAN: Duration = Duration::seconds(3);
const PROBE: Duration = Duration::seconds(30);
/// 등록부에 있는데 jsonl 이 아직 없는 세션을 보는 간격 (4.5 신규 세션 발견 1번)
const AWAIT: Duration = Duration::seconds(1);
/// 첫 레코드가 이보다 최근이면 방금 시작된 세션이라 출근 연출을 낸다 (4.5 신규 세션 발견 3번)
const JUST_STARTED: Duration = Duration::seconds(60);
/// 다른 호스트의 심장박동이 이만큼 멈추면 의심 (4.5 감시자)
const STALL_MS: i64 = 2000;
const TICK: StdDuration = StdDuration::from_millis(200);

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

/// 다른 스레드가 이 호스트를 지켜보는 값 (4.5 감시자). 멈춘 I/O 안의 스레드는 자기 지연을 잴 수 없다
pub struct Heartbeat {
    /// `mono_ms` 로 잰 마지막 틱
    pub last_tick_ms: AtomicI64,
    /// 의심 상태. Probe 가 켜짐을 다시 확인할 때까지 UNC 를 건드리지 않는다 (4.4.2)
    // ponytail: 소등과 통신 지연을 이 깃발 하나로 둔다, 사이드바가 둘을 나눠 그리는 티켓에서 나눈다
    pub suspect: AtomicBool,
    /// Probe 가 켜짐을 확인하고 넘긴 Site. 이것을 받은 틱만 경로 접근(나열, verify_path, 다시 열기)을 한다 (4.4.2)
    pub probed: Mutex<Option<Vec<Site>>>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self {
            last_tick_ms: AtomicI64::new(mono_ms()),
            suspect: AtomicBool::new(false),
            probed: Mutex::new(None),
        }
    }
}

/// Windows 스레드의 Probe 가 부른다: (Windows Site, WSL 배포판). 테스트가 바꿔 끼운다
pub type ProbeFn = Box<dyn FnMut() -> (Vec<Site>, Vec<WslDistro>) + Send>;

/// 세션 파일 하나와 그 파서
struct Tracked {
    key: TeamKey,
    reader: TailReader,
    parser: Box<dyn Parser + Send>,
    last_read: DateTime<Utc>,
    /// 연속으로 Scan 에서 못 본 횟수
    missing: u8,
    /// 서브 파일 옆 meta.json 을 마지막 Scan 에서 봤나
    has_meta: bool,
    /// 읽기 실패를 이미 알렸나. 실패가 이어져도 한 번만 남긴다
    failed: bool,
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
    /// 이번 틱에 난 연출 신호
    // ponytail: 틱마다 비운다, IPC(T14)가 생기면 office-state 와 함께 보낸다
    pub cues: Vec<(OfficeId, Cue)>,
    /// 다시 만들어야 하는데 아직 못 만든 팀. 경로 접근이 되는 다음 Scan 에서 만든다
    rebuild_due: BTreeSet<TeamKey>,
    /// Windows 스레드만: Probe 와 감시할 다른 호스트
    probe: Option<ProbeFn>,
    last_probe: Option<DateTime<Utc>>,
    remotes: Vec<(String, Arc<Heartbeat>)>,
    /// Windows Claude 만: 등록부에 살아 있는 세션과 그중 jsonl 이 아직 없는 세션 (4.5 신규 세션 발견 1번)
    alive: HashSet<String>,
    awaited: BTreeSet<String>,
    last_await: Option<DateTime<Utc>>,
    project_dirs: Vec<PathBuf>,
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
            probe: None,
            last_probe: None,
            remotes: Vec::new(),
            alive: HashSet::new(),
            awaited: BTreeSet::new(),
            last_await: None,
            project_dirs: Vec::new(),
        }
    }

    /// Windows 스레드에 Probe 를 단다. 켜진 WSL 배포판마다 HostLoop 스레드를 띄운다 (4.4.2)
    pub fn with_probe(mut self, probe: ProbeFn) -> Self {
        self.probe = Some(probe);
        self
    }

    /// 다른 호스트의 심장박동을 매 틱 본다 (4.5 감시자)
    pub fn watch(&mut self, host_id: String, heartbeat: Arc<Heartbeat>) {
        self.remotes.push((host_id, heartbeat));
    }

    fn is_wsl(&self) -> bool {
        matches!(self.host.kind, HostKind::Wsl { .. })
    }

    fn suspect(&self) -> bool {
        self.heartbeat.suspect.load(Ordering::Relaxed)
    }

    /// 한 틱. 읽기·파싱은 잠금 밖에서, 적용만 잠금 안에서 한다 (4.3)
    pub fn tick(&mut self) {
        let now = self.clock.now();
        self.heartbeat
            .last_tick_ms
            .store(mono_ms(), Ordering::Relaxed);
        self.cues.clear();
        if self.probe.is_some() && self.last_probe.is_none_or(|t| now - t >= PROBE) {
            self.probe();
            self.last_probe = Some(now);
        }
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
            self.sites = sites;
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
                self.try_rebuild(&key, now, &BTreeSet::new());
            }
        }
        let paths: Vec<PathBuf> = self.files.keys().cloned().collect();
        for path in paths {
            // 읽다가 의심이 되면 그 호스트의 UNC 를 더 건드리지 않는다
            if self.suspect() {
                break;
            }
            if self.due(&path, now) {
                self.poll_logged(&path, now, path_ok);
            }
        }
        let cues = self
            .world
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tick(now);
        self.cues.extend(cues);
        if let Err(e) = self
            .diag
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush_if_due(now)
        {
            // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
            eprintln!("미대응 로그 쓰기 실패: {e}");
        }
    }

    fn due(&self, path: &Path, now: DateTime<Utc>) -> bool {
        let Some(t) = self.files.get(path) else {
            return false;
        };
        let every = match self.tier_of(&t.key) {
            Tier::Hot => HOT,
            Tier::Warm => WARM,
            Tier::Cold => COLD,
        };
        now - t.last_read >= every
    }

    /// 선택 사무실의 팀은 Hot, 퇴근 팀은 Cold, 나머지 Warm (4.5 표).
    /// 선택이 퇴근보다 앞선다. 보고 있는 퇴근 팀이 다시 일하면 60초가 아니라 바로 보여야 한다
    fn tier_of(&self, key: &TeamKey) -> Tier {
        let w = self.world.lock().unwrap_or_else(|e| e.into_inner());
        match w.teams.get(key) {
            _ if w.selected.as_ref() == Some(&w.office_of(key)) => Tier::Hot,
            Some(t) if t.phase == crate::office::TeamPhase::Off => Tier::Cold,
            _ => Tier::Warm,
        }
    }

    fn poll_logged(&mut self, path: &Path, now: DateTime<Utc>, path_ok: bool) {
        let result = self.poll(path, now, path_ok);
        let wsl = self.is_wsl();
        let Some(t) = self.files.get_mut(path) else {
            return;
        };
        match result {
            Ok(()) => t.failed = false,
            Err(e) => {
                if wsl {
                    // 핸들 에러는 배포판이 꺼졌거나 공유가 끊긴 것이다. 같은 핸들은 다시 쓰지 못하니
                    // Probe 가 켜짐을 확인한 뒤 경로로 다시 열어 팀을 만든다 (4.4.2)
                    self.heartbeat.suspect.store(true, Ordering::Relaxed);
                    self.rebuild_due.insert(t.key.clone());
                }
                if !t.failed {
                    t.failed = true;
                    // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
                    eprintln!("읽기 실패 {}: {e}", path.display());
                }
            }
        }
    }

    /// 파일 하나를 읽어 적용한다. 적용이 끝난 뒤에만 commit 한다 (4.5 2단계 commit)
    fn poll(&mut self, path: &Path, now: DateTime<Utc>, path_ok: bool) -> io::Result<()> {
        let Some(t) = self.files.get_mut(path) else {
            return Ok(());
        };
        if self.rebuild_due.contains(&t.key) {
            return Ok(());
        }
        t.last_read = now;
        if !t.reader.changed()? {
            return Ok(());
        }
        let pending = t.reader.read_new_lines(MAX_READ_BYTES)?;
        if pending.dropped_lines > 0 {
            // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
            eprintln!("64MB 넘는 줄 버림 {}", path.display());
        }
        if pending.reset {
            // 어느 파일의 Reset 이든 팀 전체를 다시 만든다 (4.3 범위)
            let key = t.key.clone();
            eprintln!("Reset {}", path.display());
            if path_ok {
                self.try_rebuild(&key, now, &BTreeSet::new());
            } else {
                self.rebuild_due.insert(key);
            }
            return Ok(());
        }
        let generation = u64::from(t.reader.generation());
        let mut events = Vec::new();
        let mut unhandled = Vec::new();
        for line in &pending.lines {
            let raw = RawRef {
                path: path.to_path_buf(),
                generation,
                byte_offset: line.offset,
                line_no: line.line_no,
            };
            events.extend(t.parser.parse_line(&line.bytes, &raw, &mut unhandled));
        }
        let key = t.key.clone();
        // 멈췄던 I/O 의 결과는 버리고 commit 하지 않는다. 재개한 첫 틱이 같은 구간을 다시 읽는다 (4.5 감시자)
        if self.suspect() {
            return Ok(());
        }
        let (cues, product, office) = {
            let mut w = self.world.lock().unwrap_or_else(|e| e.into_inner());
            let cues = w.apply(&key, &events, now);
            let product = w.teams.get(&key).map(|t| t.product);
            (cues, product, w.office_of(&key))
        };
        if let Some(t) = self.files.get_mut(path) {
            t.reader.commit(pending);
        }
        self.cues
            .extend(cues.into_iter().map(|c| (office.clone(), c)));
        if let Some(ts) = events.iter().filter_map(|e| e.ts).max() {
            self.clock.observe(ts);
        }
        let product = product.unwrap_or_else(|| default_product(key.vendor));
        self.observe(product, &events, &unhandled, now);
        if path_ok {
            self.register_subs(path, &key, &events, now);
        }
        Ok(())
    }

    /// Spawn 결과로 안 서브 파일은 Scan 을 기다리지 않고 바로 읽는다. S19 가 끊기지 않게 (4.5 서브 파일 발견).
    /// 아직 없으면 Scan 이 찾는다
    fn register_subs(&mut self, path: &Path, key: &TeamKey, events: &[Event], now: DateTime<Utc>) {
        if key.vendor != Vendor::Claude {
            return;
        }
        let dir = if is_subagent(path) {
            path.parent().and_then(Path::parent).map(Path::to_path_buf)
        } else {
            Some(path.with_extension(""))
        };
        let Some(dir) = dir else {
            return;
        };
        for ev in events {
            let EventKind::SubagentSpawn { child_id, .. } = &ev.kind else {
                continue;
            };
            // 파일 내용에서 온 값이라 경로 조각으로 쓰기 전에 거른다
            if !safe_id(child_id) {
                continue;
            }
            let sub = dir
                .join("subagents")
                .join(format!("agent-{child_id}.jsonl"));
            if self.files.contains_key(&sub) {
                continue;
            }
            // 다음 틱에 바로 읽히게 마지막 읽기를 한참 전으로 둔다
            if let Ok(mut t) = open(&sub, key.clone(), now - COLD) {
                t.has_meta = sub.with_extension("meta.json").is_file();
                self.files.insert(sub, t);
            }
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
    fn try_rebuild(&mut self, key: &TeamKey, now: DateTime<Utc>, new: &BTreeSet<PathBuf>) {
        if let Err(e) = self.rebuild_team(key, now, new) {
            // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
            eprintln!("팀 재구성 실패 {}: {e}", key.session);
            if self.is_wsl() {
                self.heartbeat.suspect.store(true, Ordering::Relaxed);
            }
            // 새 팀이면 파일이 아직 등록 전이라 다음 Scan 이 새 파일로 다시 찾는다
            if self.files.values().any(|t| &t.key == key) {
                self.rebuild_due.insert(key.clone());
            }
        }
    }

    /// 팀의 파일을 모두 처음부터 다시 읽어 만든다 (새 세션, Reset. 4.3 재구성 규칙).
    /// `new` 는 이번에 처음 찾은 파일이다. 방금 시작된 파일이면 출근 cue 를 내고 미대응 재료를 센다.
    /// 앱이 늦게 발견한 옛 파일은 조용히 넣고 세지 않는다. 재시작마다 같은 기록을 다시 세지 않게
    fn rebuild_team(
        &mut self,
        key: &TeamKey,
        now: DateTime<Utc>,
        new: &BTreeSet<PathBuf>,
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
        let mut files = Vec::new();
        let mut subs = Vec::new();
        let mut arrived = Vec::new();
        let mut heard = Vec::new();
        let mut last_ts = None;
        for path in &paths {
            let mut t = open(path, key.clone(), now)?;
            t.missing = self.files.get(path).map_or(0, |o| o.missing);
            t.has_meta = is_subagent(path) && path.with_extension("meta.json").is_file();
            let mut unhandled = Vec::new();
            let events = read_all(&mut t, path, &mut unhandled)?;
            last_ts = last_ts.max(events.iter().filter_map(|e| e.ts).max());
            if new.contains(path) && just_started(path, &events, now) {
                arrived.push(path.clone());
                self.observe_events(&events, now);
                heard.extend(unhandled);
            }
            if is_subagent(path) {
                subs.extend(sub_meta(path));
            }
            files.push(events);
            fresh.push((path.clone(), t));
        }
        // 시각 있는 레코드가 하나도 없으면 아직 시작 전이다. 지금 만들면 시작 시각이 1970년이 되고
        // 무재해·피자가 그때부터 센다. 파일을 등록하지 않아 다음 Scan 이 다시 찾는다
        if last_ts.is_none() {
            return Ok(());
        }
        let local = self.world.lock().unwrap_or_else(|e| e.into_inner()).local;
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
        let (office, known) = {
            let mut w = self.world.lock().unwrap_or_else(|e| e.into_inner());
            // Spawn 을 먼저 적용해 이미 출근한 서브는 다시 출근하지 않는다
            let known: HashSet<String> = w
                .teams
                .get(key)
                .map(|t| t.employees.iter().map(|e| e.id.clone()).collect())
                .unwrap_or_default();
            w.put_team(key.clone(), team, now);
            (w.office_of(key), known)
        };
        self.files.extend(fresh);
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
        self.observe(product, &[], &heard, now);
        Ok(())
    }

    fn observe_events(&self, events: &[Event], now: DateTime<Utc>) {
        let mut d = self.diag.lock().unwrap_or_else(|e| e.into_inner());
        for ev in events {
            d.observe_event(ev, now);
        }
    }

    /// 새 세션 파일·서브 파일·meta.json 과 사라진 파일을 찾는다 (4.5 신규 세션 발견).
    /// 접근에 실패한 Site 의 파일은 "없음"이 아니라 "못 봄"이라 아무것도 지우지 않는다
    fn scan(&mut self, now: DateTime<Utc>) {
        if !self.is_wsl() {
            self.read_registry();
        }
        let mut seen = HashSet::new();
        let mut metas = HashSet::new();
        let mut ok_roots = Vec::new();
        let mut project_dirs = Vec::new();
        let mut found: BTreeMap<TeamKey, BTreeSet<PathBuf>> = BTreeMap::new();
        for site in &self.sites {
            let listing = match list_site(site) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("세션 폴더 나열 실패 {}: {e}", site.root.display());
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
        if self.is_wsl() && ok_roots.len() < self.sites.len() {
            // Probe 직후인데 나열이 안 된다. 배포판이 꺼졌거나 공유가 끊겼다
            self.heartbeat.suspect.store(true, Ordering::Relaxed);
            return;
        }
        self.project_dirs = project_dirs;

        let mut gone_teams = BTreeSet::new();
        let mut gone_subs = Vec::new();
        let mut meta_gone = Vec::new();
        let mut meta_new = BTreeSet::new();
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
                    meta_new.insert(t.key.clone());
                } else if !has && t.has_meta {
                    meta_gone.push(path.clone());
                }
                t.has_meta = has;
            }
            // 로컬은 매 Scan, WSL 은 Probe 직후 Scan 에 경로의 파일과 핸들의 파일을 대조한다 (4.4.2).
            // 실패(그새 지워짐 등)는 다음 Scan 의 없음 판정이 맡는다
            let _ = t.reader.verify_path();
        }

        for key in &gone_teams {
            self.files.retain(|_, t| &t.key != key);
            self.rebuild_due.remove(key);
            self.world
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove_team(key);
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
            meta_new.remove(key);
            self.try_rebuild(key, now, new);
        }
        for key in meta_new {
            self.try_rebuild(&key, now, &BTreeSet::new());
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

    fn main_present(&self, key: &TeamKey) -> bool {
        self.files
            .iter()
            .any(|(p, t)| &t.key == key && !is_subagent(p) && t.missing == 0)
    }

    fn retire(&mut self, key: &TeamKey, path: &Path, now: DateTime<Utc>) {
        let Some(agent) = agent_of(path) else {
            return;
        };
        let (cues, office) = {
            let mut w = self.world.lock().unwrap_or_else(|e| e.into_inner());
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
            .and_then(|s| s.root.parent());
        self.alive = home
            .map(registry::live_sessions)
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
        self.awaited.iter().any(|id| {
            self.project_dirs
                .iter()
                .any(|d| d.join(format!("{id}.jsonl")).is_file())
        })
    }

    /// 30초마다 Windows Site 와 WSL 배포판의 켜짐·꺼짐을 반영한다 (4.4.2).
    /// 꺼진 배포판은 UNC 를 건드리지 않고, 의심 상태를 푸는 것도 여기서만 한다
    fn probe(&mut self) {
        let Some(f) = self.probe.as_mut() else {
            return;
        };
        let (local, distros) = f();
        self.sites = local;
        for d in distros {
            if let Some((_, hb)) = self.remotes.iter().find(|(id, _)| *id == d.host.id) {
                match d.running {
                    Some(true) => {
                        // Site 를 먼저 넘긴 뒤 의심을 푼다. 순서가 바뀌면 경로 접근 없이 한 틱이 돈다
                        *hb.probed.lock().unwrap_or_else(|e| e.into_inner()) = Some(d.sites);
                        hb.suspect.store(false, Ordering::Relaxed);
                    }
                    Some(false) => hb.suspect.store(true, Ordering::Relaxed),
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
                self.remotes.push((d.host.id.clone(), h.heartbeat.clone()));
                // ponytail: 레지스트리에서 사라진 배포판의 스레드는 멈추지 않는다, 배포판 삭제가 보이면 올린다
                spawn(h);
            }
        }
    }

    /// 2초 넘게 멈춘 호스트를 의심으로 바꾼다. 멈춘 I/O 는 취소하지 않는다 (4.5 감시자)
    fn watch_remotes(&self) {
        let now = mono_ms();
        for (id, hb) in &self.remotes {
            if now - hb.last_tick_ms.load(Ordering::Relaxed) > STALL_MS
                && !hb.suspect.swap(true, Ordering::Relaxed)
            {
                eprintln!("{id} 스레드가 멈췄다. Probe 가 켜짐을 확인할 때까지 의심");
            }
        }
    }
}

/// 호스트 하나의 루프를 스레드로 돌린다. 한 호스트가 UNC 에서 멈춰도 다른 호스트는 돈다 (4.5)
pub fn spawn(mut host: HostLoop) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let start = Instant::now();
            host.tick();
            let spent = start.elapsed();
            if spent > TICK {
                // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 400ms 감속(4.10)과 함께 운영 로그 티켓에서 올린다
                eprintln!("{} 틱 {}ms", host.host.id, spent.as_millis());
            }
            thread::sleep(TICK.saturating_sub(spent));
        }
    })
}

/// 첫 레코드 ts(없으면 파일 생성 시각)가 지금부터 60초 안인가 (4.5 신규 세션 발견 3번)
fn just_started(path: &Path, events: &[Event], now: DateTime<Utc>) -> bool {
    let first = events.iter().find_map(|e| e.ts).or_else(|| {
        let created = fs::metadata(path).ok()?.created().ok()?;
        Some(DateTime::<Utc>::from(created))
    });
    first.is_some_and(|t| now - t < JUST_STARTED)
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

fn open(path: &Path, key: TeamKey, last_read: DateTime<Utc>) -> io::Result<Tracked> {
    Ok(Tracked {
        parser: make_parser(path, key.vendor),
        reader: TailReader::open(path)?,
        key,
        last_read,
        missing: 0,
        has_meta: false,
        failed: false,
    })
}

/// 재구성용으로 끝까지 읽는다. 8MB 상한이 있어 backlog 가 남으면 이어 읽는다 (4.5)
fn read_all(
    t: &mut Tracked,
    path: &Path,
    unhandled: &mut Vec<Unhandled>,
) -> io::Result<Vec<Event>> {
    let mut events = Vec::new();
    loop {
        let pending = t.reader.read_new_lines(MAX_READ_BYTES)?;
        if pending.lines.is_empty() {
            t.reader.commit(pending);
            return Ok(events);
        }
        let generation = u64::from(t.reader.generation());
        for line in &pending.lines {
            let raw = RawRef {
                path: path.to_path_buf(),
                generation,
                byte_offset: line.offset,
                line_no: line.line_no,
            };
            events.extend(t.parser.parse_line(&line.bytes, &raw, unhandled));
        }
        t.reader.commit(pending);
    }
}

fn is_subagent(p: &Path) -> bool {
    p.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "subagents")
}

fn make_parser(path: &Path, vendor: Vendor) -> Box<dyn Parser + Send> {
    match vendor {
        Vendor::Codex => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            // 세션 id 는 파일명 끝 UUID (4.6.2)
            let id = stem.get(stem.len().saturating_sub(36)..).unwrap_or(stem);
            Box::new(CodexParser::new(id))
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
fn sub_meta(path: &Path) -> Option<SubMeta> {
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
fn list_site(site: &Site) -> io::Result<Listing> {
    let mut out = Listing::default();
    match site.vendor {
        Vendor::Claude => {
            for project in dirs(&site.root, false)? {
                for f in files(&project)? {
                    if f.extension().is_none_or(|e| e != "jsonl") {
                        continue;
                    }
                    let Some(id) = f.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                        continue;
                    };
                    for sub in files(&project.join(&id).join("subagents"))? {
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
            for y in dirs(&site.root, false)? {
                for m in dirs(&y, true)? {
                    for d in dirs(&m, true)? {
                        for f in files(&d)? {
                            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned());
                            let jsonl = f.extension().is_some_and(|e| e == "jsonl");
                            if let Some(stem) = stem.filter(|s| jsonl && s.starts_with("rollout-"))
                            {
                                let id = stem
                                    .get(stem.len().saturating_sub(36)..)
                                    .unwrap_or(&stem)
                                    .to_string();
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
fn entries(p: &Path, missing_ok: bool) -> io::Result<Vec<(PathBuf, bool)>> {
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

fn dirs(p: &Path, missing_ok: bool) -> io::Result<Vec<PathBuf>> {
    Ok(entries(p, missing_ok)?
        .into_iter()
        .filter(|e| e.1)
        .map(|e| e.0)
        .collect())
}

fn files(p: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(entries(p, true)?
        .into_iter()
        .filter(|e| !e.1)
        .map(|e| e.0)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{HostKind, ReadMode};
    use crate::office::names::NameBook;
    use crate::office::world::Scale;
    use chrono::FixedOffset;

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

    #[test]
    fn first_tick_finds_sessions_and_builds_named_teams() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = tmp.path().join("projects");
        copy_dir(&fixture("claude/sync-subagent"), &claude.join("p1"));
        let codex = tmp.path().join("sessions");
        copy_dir(&fixture("codex/codex-app"), &codex.join("2026/03/02"));
        let host = Host {
            id: "windows".into(),
            kind: HostKind::Windows,
            label: "Windows".into(),
        };
        let site = |vendor, root: &Path| Site {
            host: host.clone(),
            vendor,
            root: root.to_path_buf(),
            read_mode: ReadMode::Append,
        };
        let world = Arc::new(Mutex::new(OfficeWorld::new(
            Scale::Large,
            NameBook::load(tmp.path()),
            FixedOffset::east_opt(9 * 3600).unwrap(),
        )));
        let mut h = HostLoop::new(
            host.clone(),
            vec![site(Vendor::Claude, &claude), site(Vendor::Codex, &codex)],
            world.clone(),
            Arc::new(WallClock),
            Arc::new(Mutex::new(Diag::open(tmp.path(), false))),
        );
        h.tick();
        let w = world.lock().unwrap();
        assert_eq!(w.teams.len(), 2);
        // Claude 팀은 부장 + 동기 서브 하나
        let claude_team = w.teams.values().find(|t| t.employees.len() == 2).unwrap();
        assert!(claude_team.employees.iter().all(|e| !e.name.is_empty()));
        assert!(w.list_seq > 0);
    }

    const REJECT: &str = "13f4d27c-e1d7-fa23-0b0e-05ad838d1b68";
    const SYNC: &str = "ed46b23c-eb2a-4459-1e37-cb69d8ecad93";
    const SYNC_SUB: &str = "ac0c27afe76bccd54";
    const ASYNC: &str = "302222d1-a790-f2ee-6a9b-2212d4de135d";
    const ASYNC_SUB: &str = "a4fa9a3e07b34f715";

    fn t0() -> DateTime<Utc> {
        "2026-03-02T09:00:00Z".parse().unwrap()
    }

    fn secs(n: i64) -> Duration {
        Duration::seconds(n)
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
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// 사장님 지시 한 줄 (Claude)
    fn user_line(session: &str, ts: DateTime<Utc>, text: &str) -> String {
        serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "uuid": format!("u-{}", ts.timestamp_millis()),
            "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "entrypoint": "claude-desktop",
            "cwd": "C:\\dir1",
            "sessionId": session,
        })
        .to_string()
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

        fn seq(&self, key: &TeamKey) -> u64 {
            let w = self.w();
            w.state_seq.get(&w.office_of(key)).copied().unwrap_or(0)
        }
    }

    /// 직원별 (room, sentence, confidence) 와 소품·단계
    fn view(t: &crate::office::Team) -> String {
        let emps: Vec<_> = t
            .employees
            .iter()
            .map(|e| {
                (
                    &e.id,
                    e.activity.room,
                    &e.activity.sentence,
                    e.activity.confidence,
                )
            })
            .collect();
        format!("{emps:?} {:?} {:?}", t.props, t.phase)
    }

    /// view 에 실적·진행 중 호출까지. 같은 줄을 두 번 적용하면 여기서 드러난다
    fn full_view(t: &crate::office::Team) -> String {
        let extra: Vec<_> = t
            .employees
            .iter()
            .map(|e| {
                (
                    e.counters,
                    e.pending.len(),
                    e.background.len(),
                    e.retired_at,
                )
            })
            .collect();
        format!("{} {extra:?} {:?}", view(t), t.stats)
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
    fn append_reaches_world_within_500ms_on_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
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
        let _thread = spawn(h);
        let wait = |cond: &dyn Fn(&OfficeWorld) -> bool, limit: StdDuration| {
            let start = Instant::now();
            while start.elapsed() < limit {
                if cond(&world.lock().unwrap()) {
                    return Some(start.elapsed());
                }
                thread::sleep(StdDuration::from_millis(5));
            }
            None
        };
        wait(&|w| !w.teams.is_empty(), StdDuration::from_secs(5)).expect("첫 Scan");
        {
            let mut w = world.lock().unwrap();
            let key = w.teams.keys().next().unwrap().clone();
            let office = w.office_of(&key);
            assert!(w.select(Some(office)));
        }
        append(&file, &[user_line(REJECT, Utc::now(), "둘째 지시")]);
        let boss_visit = |w: &OfficeWorld| {
            w.teams
                .values()
                .any(|t| t.props.boss_visit.as_deref() == Some("둘째 지시"))
        };
        let took = wait(&boss_visit, StdDuration::from_millis(500));
        assert!(took.is_some(), "500ms 안에 반영되지 않았다");
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
            let w = e.w();
            let team = &w.teams[&key];
            let sub = team.employees.iter().find(|x| x.id == SYNC_SUB).unwrap();
            assert!(sub.retired_at.is_some());
            assert!(team.employees[0].retired_at.is_none());
            drop(w);
            assert!(!h.files.keys().any(|p| is_subagent(p)), "리더를 놓는다");
        }
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
    }

    #[test]
    fn watchdog_marks_stalled_host_suspect_while_windows_keeps_updating() {
        let e = env();
        let root = e.dir("projects");
        let file = root.join("p1").join(format!("{REJECT}.jsonl"));
        let lines = lines_of(&fixture(&format!("claude/reject/{REJECT}.jsonl")));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
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
        let before = e.seq(&key);
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
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        append(&file, &lines[..20]);
        let site = site_of(&wsl(), Vendor::Claude, &root);
        let mut h = e.host(wsl(), Vec::new());
        let probe = |h: &HostLoop| {
            *h.heartbeat.probed.lock().unwrap() = Some(vec![site.clone()]);
            h.heartbeat.suspect.store(false, Ordering::Relaxed);
        };
        let now = t0() + secs(150);
        probe(&h);
        e.at(now);
        h.tick();
        let key = e.key(id);
        let before = e.seq(&key);
        // 읽기가 끝나기 전에 감시자가 의심으로 바꿨다
        append(&file, &lines[20..]);
        h.heartbeat.suspect.store(true, Ordering::Relaxed);
        e.at(now + secs(3));
        h.poll(&file, now + secs(3), false).unwrap();
        e.at(now + secs(4));
        h.tick();
        assert_eq!(e.seq(&key), before, "의심 중 읽은 구간은 적용하지 않는다");
        probe(&h);
        e.at(now + secs(6));
        h.tick();
        let got = full_view(&e.w().teams[&key]);

        let f = env();
        let root = f.dir("wsl/projects");
        copy_dir(&fixture("claude/bg-fail-reject-compact"), &root.join("p1"));
        let mut g = f.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        f.at(now + secs(6));
        g.tick();
        let key = f.key(id);
        assert_eq!(got, full_view(&f.w().teams[&key]));
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
        let key = e.key(REJECT);
        assert_eq!(e.w().teams[&key].started, t0());
        // 머리만 다르고 끝 4KB 는 같은 새 파일. 이어 읽기의 해시 대조로는 못 잡고 파일 ID 대조만 잡는다
        let old = fs::read_to_string(&file).unwrap();
        let new = old.replacen("2026-03-02T09:00:00.000Z", "2026-03-02T08:00:00.000Z", 1)
            + &user_line(REJECT, t0() + secs(200), "새 파일")
            + "\n";
        fs::rename(&file, file.with_extension("old")).unwrap();
        fs::write(&file, new).unwrap();
        e.at(now + secs(3));
        h.tick();
        let w = e.w();
        let team = &w.teams[&key];
        assert_eq!(team.started, t0() - Duration::hours(1));
        assert_eq!(team.props.boss_visit.as_deref(), Some("새 파일"));
    }

    #[test]
    fn appending_line_by_line_matches_full_read_under_logical_clock() {
        let cases = [
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
        for (dir, vendor) in cases {
            let src = fs::read_dir(fixture(dir))
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.is_file())
                .unwrap();
            let name = src.file_name().unwrap().to_owned();
            let lines = lines_of(&src);
            let mut ts = Vec::new();
            for l in &lines {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                let t = v["timestamp"].as_str().and_then(|s| s.parse().ok());
                ts.push(t.or(ts.last().copied().flatten()));
            }
            let last = ts.iter().flatten().max().copied().unwrap();
            let sub = match vendor {
                Vendor::Claude => "p1",
                _ => "2026/03/02",
            };
            let read = |e: &Env| {
                let root = e.dir("root");
                fs::create_dir_all(root.join(sub)).unwrap();
                (
                    e.host(win(), vec![site_of(&win(), vendor, &root)]),
                    root.join(sub).join(&name),
                )
            };
            // 마지막 줄 뒤 Scan(3초)이 한 번은 돈 시점부터 비교한다. 시각 없는 첫 줄만 있을 때는 팀을 만들지 않는다
            for end in [
                last + SCAN,
                last + Duration::minutes(10),
                last + Duration::minutes(40),
            ] {
                // (a) 한 번에 전체 읽기
                let a = env();
                let (mut ha, file) = read(&a);
                append(&file, &lines);
                a.at(end);
                ha.tick();
                let want = view(a.w().teams.values().next().unwrap());

                // (b) 줄마다 그 논리 시각에 append 하며 틱
                let b = env();
                let (mut hb, file) = read(&b);
                for (i, l) in lines.iter().enumerate() {
                    if let Some(t) = ts[i] {
                        b.at(t);
                    }
                    append(&file, std::slice::from_ref(l));
                    hb.tick();
                    // 팀이 생기면 선택해 Hot 으로 읽는다. 줄마다 그 틱에 읽혀야 실제 간격 append 와 같다
                    let mut w = b.w();
                    if w.selected.is_none()
                        && let Some(key) = w.teams.keys().next().cloned()
                    {
                        let office = w.office_of(&key);
                        w.select(Some(office));
                    }
                }
                b.at(end);
                hb.tick();
                let got = view(b.w().teams.values().next().unwrap());
                assert_eq!(got, want, "{dir} {end}");

                // (c) 앞 절반을 읽은 뒤 나머지를 한 번에 (깨어난 노트북, 의심이 풀린 WSL 처럼 밀린 줄을 한 묶음으로)
                let c = env();
                let (mut hc, file) = read(&c);
                let half = lines.len() / 2;
                append(&file, &lines[..half]);
                c.at(ts[..half].iter().flatten().max().copied().unwrap_or(t0()));
                hc.tick();
                append(&file, &lines[half..]);
                c.at(end);
                hc.tick();
                let got = view(c.w().teams.values().next().unwrap());
                assert_eq!(got, want, "{dir} {end} 묶음");
            }
        }
    }

    /// 잠든 노트북·의심이 풀린 WSL 처럼 시계가 크게 건너뛴 뒤 밀린 줄을 한 번에 읽어도 전체 읽기와 같다.
    /// 결과보다 30분 포기가 먼저 일어나야 한다 (4.3 재구성 규칙, 4.7.2)
    #[test]
    fn batch_after_clock_jump_matches_full_read() {
        let rec = |ts: DateTime<Utc>, kind: &str, content: serde_json::Value| {
            serde_json::json!({
                "type": kind,
                "message": {"role": kind, "content": content},
                "uuid": format!("u-{}", ts.timestamp_millis()),
                "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "entrypoint": "claude-desktop",
                "cwd": "C:\\dir1",
                "sessionId": REJECT,
            })
            .to_string()
        };
        let lines = [
            user_line(REJECT, t0(), "빌드"),
            rec(
                t0() + secs(1),
                "assistant",
                serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "cargo build"}}]),
            ),
            rec(
                t0() + Duration::minutes(31),
                "user",
                serde_json::json!([{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok", "is_error": false}]),
            ),
        ];
        let end = t0() + Duration::minutes(32);
        let run = |split: usize| {
            let e = env();
            let root = e.dir("projects");
            fs::create_dir_all(root.join("p1")).unwrap();
            let file = root.join("p1").join(format!("{REJECT}.jsonl"));
            let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
            append(&file, &lines[..split]);
            e.at(t0() + secs(1));
            h.tick();
            append(&file, &lines[split..]);
            e.at(end);
            h.tick();
            let key = e.key(REJECT);
            full_view(&e.w().teams[&key])
        };
        assert_eq!(run(2), run(3));
    }

    #[test]
    fn wsl_lists_and_reopens_paths_only_right_after_probe() {
        let e = env();
        let root = e.dir("wsl/projects");
        let p1 = root.join("p1");
        copy_dir(&fixture("claude/reject"), &p1);
        let site = site_of(&wsl(), Vendor::Claude, &root);
        let mut h = e.host(wsl(), Vec::new());
        let now = t0() + secs(150);
        e.at(now);
        h.tick();
        assert!(e.w().teams.is_empty(), "Probe 전에는 나열하지 않는다");
        *h.heartbeat.probed.lock().unwrap() = Some(vec![site.clone()]);
        e.at(now + secs(1));
        h.tick();
        let key = e.key(REJECT);
        copy_dir(&fixture("claude/compaction"), &p1);
        append(
            &p1.join(format!("{REJECT}.jsonl")),
            &[user_line(REJECT, t0() + secs(200), "핸들로 읽기")],
        );
        e.at(now + secs(10));
        h.tick();
        let w = e.w();
        assert_eq!(w.teams.len(), 1, "Scan 은 Probe 직후에만 돈다");
        assert_eq!(
            w.teams[&key].props.boss_visit.as_deref(),
            Some("핸들로 읽기")
        );
        drop(w);
        *h.heartbeat.probed.lock().unwrap() = Some(vec![site]);
        e.at(now + secs(11));
        h.tick();
        assert_eq!(e.w().teams.len(), 2);
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
        let mut h = e.host(win(), Vec::new()).with_probe(probe);
        h.watch(remote.host.id.clone(), remote.heartbeat.clone());
        let suspect = || remote.heartbeat.suspect.load(Ordering::Relaxed);
        let probed = || remote.heartbeat.probed.lock().unwrap().is_some();
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        assert!(suspect() && !probed(), "꺼진 배포판은 건드리지 않는다");
        *running.lock().unwrap() = None;
        e.at(now + secs(30));
        h.tick();
        assert!(suspect() && !probed(), "모름은 의심을 풀지 않는다");
        e.at(now + secs(40));
        *running.lock().unwrap() = Some(true);
        h.tick();
        assert!(suspect(), "Probe 는 30초마다");
        e.at(now + secs(60));
        h.tick();
        assert!(!suspect() && probed());
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
        let key = e.key(REJECT);
        assert!(e.w().teams[&key].alive_in_registry);
        assert!(h.awaited.is_empty());
    }

    #[test]
    fn spawned_sub_file_is_read_without_waiting_for_scan() {
        let e = env();
        let root = e.dir("projects");
        let p1 = root.join("p1");
        let src = fixture("claude/async-subagent");
        let lines = lines_of(&src.join(format!("{ASYNC}.jsonl")));
        fs::create_dir_all(&p1).unwrap();
        let main = p1.join(format!("{ASYNC}.jsonl"));
        append(&main, &lines[..3]);
        let mut h = e.host(win(), vec![site_of(&win(), Vendor::Claude, &root)]);
        let now = t0() + Duration::hours(1);
        e.at(now);
        h.tick();
        let key = e.key(ASYNC);
        let office = e.w().office_of(&key);
        e.w().select(Some(office));
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
        let w = e.w();
        let s = w.teams[&key]
            .employees
            .iter()
            .find(|x| x.id == ASYNC_SUB)
            .unwrap();
        assert!(
            s.last_event > s.hired_at,
            "Scan(3초) 전에 서브 파일을 읽었다"
        );
    }

    fn ms(n: i64) -> Duration {
        Duration::milliseconds(n)
    }
}
