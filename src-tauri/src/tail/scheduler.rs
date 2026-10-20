//! 호스트별 폴링 스레드 (PLAN 4.5 폴링 계층, 4.4.2 깨우지 않기)

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};

use super::{MAX_READ_BYTES, TailReader};
use crate::discover::{Host, Product, Site, Vendor};
use crate::office::classify::{SubMeta, rebuild};
use crate::office::world::{OfficeWorld, TeamKey};
use crate::parse::claude::ClaudeParser;
use crate::parse::codex::CodexParser;
use crate::parse::{Event, Parser, RawRef, Unhandled};

const HOT: Duration = Duration::milliseconds(200);
const WARM: Duration = Duration::seconds(2);
const COLD: Duration = Duration::seconds(60);
const SCAN: Duration = Duration::seconds(3);

/// 실행 중에는 벽시계, replay·테스트에서는 논리 시계 (4.3 시계 규칙)
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Hot,
    Warm,
    Cold,
}

/// 다른 스레드가 이 호스트를 지켜보는 값 (4.5 감시자). 멈춘 I/O 안의 스레드는 자기 지연을 잴 수 없다
#[derive(Default)]
pub struct Heartbeat {
    pub last_tick_ms: AtomicI64,
    /// 의심 상태. Probe 가 켜짐을 다시 확인할 때까지 UNC 를 건드리지 않는다 (4.4.2)
    pub suspect: AtomicBool,
}

/// 세션 파일 하나와 그 파서
struct Tracked {
    key: TeamKey,
    reader: TailReader,
    parser: Box<dyn Parser + Send>,
    last_read: DateTime<Utc>,
}

pub struct HostLoop {
    pub host: Host,
    pub sites: Vec<Site>,
    files: BTreeMap<PathBuf, Tracked>,
    world: Arc<Mutex<OfficeWorld>>,
    clock: Arc<dyn Clock>,
    pub heartbeat: Arc<Heartbeat>,
    last_scan: Option<DateTime<Utc>>,
    /// 파서가 낸 미대응 재료. diag 연결은 T12 채우기에서
    unhandled: Vec<Unhandled>,
}

impl HostLoop {
    pub fn new(
        host: Host,
        sites: Vec<Site>,
        world: Arc<Mutex<OfficeWorld>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            host,
            sites,
            files: BTreeMap::new(),
            world,
            clock,
            heartbeat: Arc::new(Heartbeat::default()),
            last_scan: None,
            unhandled: Vec::new(),
        }
    }

    /// 한 틱. 읽기·파싱은 잠금 밖에서, 적용만 잠금 안에서 한다 (4.3)
    pub fn tick(&mut self) {
        let now = self.clock.now();
        self.heartbeat
            .last_tick_ms
            .store(now.timestamp_millis(), Ordering::Relaxed);
        if self.heartbeat.suspect.load(Ordering::Relaxed) {
            return;
        }
        if self.last_scan.is_none_or(|t| now - t >= SCAN) {
            self.scan(now);
            self.last_scan = Some(now);
        }
        let paths: Vec<PathBuf> = self.files.keys().cloned().collect();
        for path in paths {
            if self.due(&path, now) {
                // ponytail: 읽기 실패는 다음 틱에 다시 본다, 운영 로그·의심 전환은 T12 채우기에서
                let _ = self.poll(&path, now);
            }
        }
        self.world
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tick(now);
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

    /// 선택 사무실의 팀은 Hot, 퇴근 팀은 Cold, 나머지 Warm (4.5 표)
    fn tier_of(&self, key: &TeamKey) -> Tier {
        let w = self.world.lock().unwrap_or_else(|e| e.into_inner());
        match w.teams.get(key) {
            Some(t) if t.phase == crate::office::TeamPhase::Off => Tier::Cold,
            _ if w.selected.as_ref() == Some(&w.office_of(key)) => Tier::Hot,
            _ => Tier::Warm,
        }
    }

    /// 파일 하나를 읽어 적용한다. 적용이 끝난 뒤에만 commit 한다 (4.5 2단계 commit)
    fn poll(&mut self, path: &Path, now: DateTime<Utc>) -> io::Result<()> {
        let Some(t) = self.files.get_mut(path) else {
            return Ok(());
        };
        t.last_read = now;
        if !t.reader.changed()? {
            return Ok(());
        }
        let pending = t.reader.read_new_lines(MAX_READ_BYTES)?;
        if pending.reset {
            // 어느 파일의 Reset 이든 팀 전체를 다시 만든다 (4.3 범위)
            let key = t.key.clone();
            return self.rebuild_team(&key, now);
        }
        let generation = u64::from(t.reader.generation());
        let mut events = Vec::new();
        for line in &pending.lines {
            let raw = RawRef {
                path: path.to_path_buf(),
                generation,
                byte_offset: line.offset,
                line_no: line.line_no,
            };
            events.extend(t.parser.parse_line(&line.bytes, &raw, &mut self.unhandled));
        }
        // 멈췄던 I/O 의 결과는 버리고 commit 하지 않는다. 재개한 첫 틱이 같은 구간을 다시 읽는다 (4.5 감시자)
        if self.heartbeat.suspect.load(Ordering::Relaxed) {
            return Ok(());
        }
        let key = t.key.clone();
        let _cues = self
            .world
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .apply(&key, &events, now);
        if let Some(t) = self.files.get_mut(path) {
            t.reader.commit(pending);
        }
        // ponytail: cue 는 아직 버린다, IPC(T14)가 생기면 office-state 와 함께 보낸다
        Ok(())
    }

    /// 팀의 파일을 모두 처음부터 다시 읽어 만든다 (새 세션, Reset. 4.3 재구성 규칙)
    fn rebuild_team(&mut self, key: &TeamKey, now: DateTime<Utc>) -> io::Result<()> {
        let mut paths: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, t)| &t.key == key)
            .map(|(p, _)| p.clone())
            .collect();
        // 메인 파일이 먼저다 (4.3 순서)
        paths.sort_by_key(|p| (is_subagent(p), p.clone()));
        let mut files = Vec::new();
        let mut subs = Vec::new();
        for path in &paths {
            let mut tracked = open(path, key.clone(), now)?;
            files.push(read_all(&mut tracked, path, &mut self.unhandled)?);
            if is_subagent(path) {
                subs.extend(sub_meta(path));
            }
            self.files.insert(path.clone(), tracked);
        }
        let local = self.world.lock().unwrap_or_else(|e| e.into_inner()).local;
        let team = rebuild(
            &key.session,
            default_product(key.vendor),
            &subs,
            files,
            local,
        );
        self.world
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put_team(key.clone(), team, now);
        Ok(())
    }

    /// 새 세션 파일·서브 파일을 찾는다 (4.5 신규 세션 발견). 사라진 파일과 WSL 의 Probe 직후 제한은 T12 채우기에서
    fn scan(&mut self, now: DateTime<Utc>) {
        let mut new_teams = Vec::new();
        for site in &self.sites {
            for (path, session) in session_files(site) {
                if self.files.contains_key(&path) {
                    continue;
                }
                let key = TeamKey {
                    host: self.host.id.clone(),
                    vendor: site.vendor,
                    session,
                };
                if !new_teams.contains(&key) {
                    new_teams.push(key.clone());
                }
                if let Ok(t) = open(&path, key, now) {
                    self.files.insert(path, t);
                }
            }
        }
        for key in new_teams {
            let _ = self.rebuild_team(&key, now);
        }
    }
}

/// 호스트 하나의 루프를 스레드로 돌린다. 한 호스트가 UNC 에서 멈춰도 다른 호스트는 돈다 (4.5)
pub fn spawn(mut host: HostLoop) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let start = Instant::now();
            host.tick();
            // ponytail: 틱 200ms 초과 로그는 운영 로그(4.11)가 생기면 단다
            let spent = start.elapsed();
            thread::sleep(StdDuration::from_millis(200).saturating_sub(spent));
        }
    })
}

fn open(path: &Path, key: TeamKey, now: DateTime<Utc>) -> io::Result<Tracked> {
    Ok(Tracked {
        parser: make_parser(path, key.vendor),
        reader: TailReader::open(path)?,
        key,
        last_read: now,
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

/// Site 아래 세션 파일과 그 세션 id. Claude 는 `<프로젝트>/<uuid>.jsonl` 과 `<프로젝트>/<uuid>/subagents/*.jsonl`,
/// Codex 는 `YYYY/MM/DD/rollout-*.jsonl` (4.4.1)
fn session_files(site: &Site) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    match site.vendor {
        Vendor::Claude => {
            for project in dirs(&site.root) {
                for f in files(&project, "jsonl") {
                    let Some(id) = f.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                        continue;
                    };
                    for sub in files(&project.join(&id).join("subagents"), "jsonl") {
                        out.push((sub, id.clone()));
                    }
                    out.push((f, id));
                }
            }
        }
        Vendor::Codex => {
            for y in dirs(&site.root) {
                for m in dirs(&y) {
                    for d in dirs(&m) {
                        for f in files(&d, "jsonl") {
                            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned());
                            if let Some(stem) = stem.filter(|s| s.starts_with("rollout-")) {
                                let id = stem
                                    .get(stem.len().saturating_sub(36)..)
                                    .unwrap_or(&stem)
                                    .to_string();
                                out.push((f, id));
                            }
                        }
                    }
                }
            }
        }
        Vendor::Gemini => {}
    }
    out
}

fn dirs(p: &Path) -> Vec<PathBuf> {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

fn files(p: &Path, ext: &str) -> Vec<PathBuf> {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect()
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
        );
        h.tick();
        let w = world.lock().unwrap();
        assert_eq!(w.teams.len(), 2);
        // Claude 팀은 부장 + 동기 서브 하나
        let claude_team = w.teams.values().find(|t| t.employees.len() == 2).unwrap();
        assert!(claude_team.employees.iter().all(|e| !e.name.is_empty()));
        assert!(w.list_seq > 0);
    }
}
