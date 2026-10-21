//! 팝업 두 종류(PLAN 3.7)의 재료: 직원 타임라인 페이징, 직원·세션 상세, 탐색기 열기 (4.8).
//! 원문은 메모리에 들지 않고 RawRef 로 원본 줄을 다시 읽는다. 읽은 원문은 돌려주기만 하고 로그에 남기지 않는다 (4.11)

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::discover::{HostKind, Vendor};
use crate::ipc::{
    ClipDto, CursorDto, DetailError, EmployeeDetailDto, EventDto, HeadcountDto, OpenTarget, PrDto,
    SessionDetailDto, building, lock, parse_office_id,
};
use crate::office::classify::sentence;
use crate::office::world::{OfficeId, OfficeWorld, TeamKey};
use crate::office::{Employee, Mark, PendingCall, Rank, Team, TeamPhase, TimelineKind, call_hash};
use crate::parse::claude::content_text;
use crate::parse::{Event, EventKind, Parser, RawRef, ToolKind, ToolOutcome, ToolSummary};
use crate::tail::scheduler::{is_subagent, make_parser};

/// IPC 를 오래 막지 않는 상한 (4.4.2: 켜진 배포판도 UNC 가 멈출 수 있다). 테스트는 막힌 읽기를 기다려야 해서 짧다
const READ_LIMIT: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(2)
};
const PAGE: u32 = 50;
const PAGE_MAX: u32 = 200;
/// 세션 상세의 최근 사건 (3.7). "더 보기" 는 `RECENT_MAX` 까지
const RECENT: u32 = 20;
const RECENT_MAX: u32 = 100;
/// 결과·입력·본문은 앞뒤 2KB (3.7)
const BODY_CLIP: usize = 2048;
/// tail 의 줄 상한과 같다. 이보다 긴 줄은 처음부터 이벤트가 되지 못했다
const LINE_MAX: u64 = 64 * 1024 * 1024;
const SUMMARY_CHARS: usize = 60;
const UNREADABLE: &str = "원본 줄을 다시 읽지 못했습니다";
const MISSING: &str = "원본 줄에서 이 기록을 찾지 못했습니다";

/// 다시 읽을 칸 하나. 잠금 안에서 모으고 읽기는 잠금 밖에서 한다 (4.3)
#[derive(Debug, Clone, PartialEq)]
struct Want {
    agent: String,
    path: PathBuf,
    vendor: Vendor,
    mark: Mark,
    /// 그 줄의 칸 중 몇 번째
    line_idx: u32,
    /// 결과 칸이면 같은 call 의 호출 칸과 가장 가까운 앞선 결과 칸(Started 등). Codex 파서는 앞 줄의 상태가 있어야
    /// 결과 이벤트를 내서, 앞 페이지에 있는 그 줄들을 먼저 같은 파서에 읽혀야 한다
    pair: Vec<Mark>,
}

/// 탐색기로 열 것
#[derive(Debug, Clone, PartialEq)]
enum Launch {
    /// 파일을 골라 둔 채 그 폴더를 연다
    Select(PathBuf),
    Folder(PathBuf),
    /// 그 칸의 줄을 읽어야 아는 분리된 결과 파일
    Sidecar(Want),
}

/// 다시 읽은 줄. 원문 JSON 은 도구 호출·결과 칸이 있을 때만 파싱한다
struct Line {
    rec: Option<Value>,
    events: Vec<Event>,
}

/// 파일 하나를 한 번 열어 둔 것. 줄은 앞쪽 바이트부터 같은 파서에 넣는다
struct Lines {
    reader: BufReader<File>,
    parser: Box<dyn Parser + Send>,
}

/// 상한을 넘기고도 UNC 안에서 돌고 있는 읽기 스레드: (WSL 호스트, 끝났나).
/// 이 동안 같은 호스트에 새 읽기를 만들면 클릭마다 UNC 핸들을 쥔 스레드가 늘어난다
static HUNG: Mutex<Vec<(String, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

fn hung(host: &str) -> bool {
    let mut h = HUNG.lock().unwrap_or_else(|e| e.into_inner());
    h.retain(|(_, done)| !done.load(Ordering::Relaxed));
    h.iter().any(|(id, _)| id == host)
}

/// 스레드가 어떻게 끝나든(패닉 포함) 끝났다고 알린다
struct Done(Arc<AtomicBool>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn parse_office(s: &str) -> Result<OfficeId, DetailError> {
    parse_office_id(s).ok_or(DetailError::NotFound)
}

fn find_team<'a>(
    w: &'a OfficeWorld,
    office: &OfficeId,
    team_id: &str,
) -> Result<(&'a TeamKey, &'a Team), DetailError> {
    let Some(OfficeId::Team(k)) = parse_office_id(team_id) else {
        return Err(DetailError::NotFound);
    };
    if w.office_of(&k) != *office {
        return Err(DetailError::NotFound);
    }
    w.teams.get_key_value(&k).ok_or(DetailError::NotFound)
}

/// 중소기업 사무실은 팀 여럿이 한 사무실이라 팀 id 로 먼저 좁힌다. 직원 id 는 팀 안에서만 겹치지 않는다
fn find_employee<'a>(
    w: &'a OfficeWorld,
    office: &OfficeId,
    team_id: &str,
    agent: &str,
) -> Result<(&'a TeamKey, &'a Team, &'a Employee), DetailError> {
    let (k, t) = find_team(w, office, team_id)?;
    t.employees
        .iter()
        .find(|e| e.id == agent)
        .map(|e| (k, t, e))
        .ok_or(DetailError::NotFound)
}

/// 파일에 손대도 되나. 꺼진 배포판은 경로 접근(열기·is_file·탐색기)이 깨운다 (4.4.2).
/// WSL 은 핸들을 쥔 근무 중인 팀만 배포판이 켜진 게 확실하고, Probe 사이(최대 30초)에는 "의심 아님"만으로 알 수 없다.
/// 모르는 호스트도 같다. 되면 WSL 호스트 id (읽기가 걸리면 의심으로 돌리려고)
fn host_ready(w: &OfficeWorld, key: &TeamKey, t: &Team) -> Result<Option<String>, DetailError> {
    match w.hosts.get(&key.host).map(|h| &h.kind) {
        Some(HostKind::Windows) => Ok(None),
        Some(HostKind::Wsl { .. })
            if t.phase == TeamPhase::Working
                && !w.suspect_hosts.contains(&key.host)
                && !hung(&key.host) =>
        {
            Ok(Some(key.host.clone()))
        }
        _ => Err(DetailError::HostOffline),
    }
}

/// `i` 번째 칸
fn want(t: &Team, key: &TeamKey, e: &Employee, i: usize) -> Want {
    let tl = &e.record.timeline;
    let m = &tl[i];
    Want {
        agent: e.id.clone(),
        path: t
            .sources
            .get(m.source as usize)
            .cloned()
            .unwrap_or_default(),
        vendor: key.vendor,
        mark: *m,
        line_idx: u32::try_from(i - line_start(tl, i)).unwrap_or(u32::MAX),
        pair: if m.kind == TimelineKind::ToolResult {
            // 같은 파일·세대의 같은 call 에서 호출 하나와 가장 가까운 앞선 결과 하나까지만. 페이지 밖 읽기가
            // 2초 상한을 잡아먹지 않게 한다
            let near = |kind| {
                tl[..i].iter().rev().find(|p| {
                    p.kind == kind
                        && p.call == m.call
                        && (p.source, p.generation) == (m.source, m.generation)
                })
            };
            [near(TimelineKind::ToolCall), near(TimelineKind::ToolResult)]
                .into_iter()
                .flatten()
                .copied()
                .collect()
        } else {
            Vec::new()
        },
    }
}

/// 그 칸이 든 줄의 첫 칸. 한 줄의 이벤트가 페이지 경계에서 갈리지 않게 한다
fn line_start(tl: &[Mark], mut i: usize) -> usize {
    while i > 0 && i < tl.len() && tl[i - 1].same_line(&tl[i]) {
        i -= 1;
    }
    i
}

/// 커서가 가리키는 줄의 마지막 칸
fn locate(tl: &[Mark], c: CursorDto) -> Result<usize, DetailError> {
    tl.iter()
        .rposition(|m| m.generation == c.generation && m.byte_offset == c.byte_offset)
        .ok_or(DetailError::CursorInvalid)
}

/// 커서의 줄 안에서 `line_idx` 번째 칸
fn find_mark(tl: &[Mark], c: CursorDto, line_idx: u32) -> Result<usize, DetailError> {
    let last = locate(tl, c)?;
    let i = line_start(tl, last) + line_idx as usize;
    if i > last {
        return Err(DetailError::CursorInvalid);
    }
    Ok(i)
}

/// 직원 타임라인 한 페이지, 최신부터 (4.8 `get_employee_timeline`)
pub fn employee_timeline(
    world: &Mutex<OfficeWorld>,
    office_id: &str,
    team_id: &str,
    agent_id: &str,
    before: Option<CursorDto>,
    limit: Option<u32>,
) -> Result<Vec<EventDto>, DetailError> {
    let office = parse_office(office_id)?;
    let (wants, wsl) = {
        let w = lock(world);
        let (k, t, e) = find_employee(&w, &office, team_id, agent_id)?;
        let tl = &e.record.timeline;
        let end = match before {
            None => tl.len(),
            Some(c) => line_start(tl, locate(tl, c)?),
        };
        let limit = limit.unwrap_or(PAGE).clamp(1, PAGE_MAX) as usize;
        let start = line_start(tl, end.saturating_sub(limit));
        if start == end {
            return Ok(Vec::new());
        }
        let wsl = host_ready(&w, k, t)?;
        let wants: Vec<Want> = (start..end).rev().map(|i| want(t, k, e, i)).collect();
        (wants, wsl)
    };
    bounded(world, wsl, move |deadline| read_events(&wants, deadline))
}

/// 직원 상세 팝업 중 메모리에 있는 것 (3.7)
pub fn employee_detail(
    world: &Mutex<OfficeWorld>,
    office_id: &str,
    team_id: &str,
    agent_id: &str,
) -> Result<EmployeeDetailDto, DetailError> {
    let office = parse_office(office_id)?;
    let w = lock(world);
    let (_, t, e) = find_employee(&w, &office, team_id, agent_id)?;
    let boss = &t.employees[0];
    // 상사를 모르는 서브(Spawn 을 못 본 사전 등록 직원)는 깊이 1 이므로 부장이 상사다
    let boss_name = (e.id != boss.id)
        .then(|| {
            let parent = e.parent.as_deref().unwrap_or(&boss.id);
            t.employees.iter().find(|p| p.id == parent)
        })
        .flatten()
        .map(|p| p.name.clone());
    let r = &e.record;
    Ok(EmployeeDetailDto {
        agent_id: e.id.clone(),
        name: e.name.clone(),
        rank: e.rank,
        role: e.role,
        model: e.model.clone(),
        hired_at: e.hired_at,
        retired_at: e.retired_at,
        boss_name,
        instruction: r.prompt.clone().or_else(|| r.first_message.clone()),
        files_read: r.files_read.items.clone(),
        files_edited: r.files_edited.items.clone(),
        commands: r.commands.items.clone(),
    })
}

/// 세션 상세 팝업 (3.7, 4.8 `get_session_detail`). 원본 줄을 못 읽어도 메모리 값은 돌려주고 `recent_error` 로 알린다
pub fn session_detail(
    world: &Mutex<OfficeWorld>,
    office_id: &str,
    team_id: &str,
    limit: Option<u32>,
) -> Result<SessionDetailDto, DetailError> {
    let office = parse_office(office_id)?;
    let (dto, wants, ready) = {
        let w = lock(world);
        let (k, t) = find_team(&w, &office, team_id)?;
        let n = limit.unwrap_or(RECENT).clamp(1, RECENT_MAX) as usize;
        // 직원마다 끝에서 n 칸이면 합쳐서 최신 n 개를 고르기에 모자라지 않다. 기록은 적용 순서로 쌓이니
        // 키도 적용 순서(seq)여야 자르는 기준과 같다. 칸의 ts 는 그 줄 자신의 ts 라 파일 안에서 거꾸로 갈 수 있다
        // seq 는 적용 순서라 실시간과 재구성에서 다를 수 있다. 직원 타임라인도 같은 적용 순서라 둘이 어긋나지 않게 이것을 쓴다
        let mut tail: Vec<(Mark, usize, usize)> = Vec::new();
        for (ei, e) in t.employees.iter().enumerate() {
            let tl = &e.record.timeline;
            let from = tl.len().saturating_sub(n);
            tail.extend((from..tl.len()).map(|i| (tl[i], ei, i)));
        }
        tail.sort_by_key(|(m, _, _)| std::cmp::Reverse(m.seq));
        let wants: Vec<Want> = tail
            .into_iter()
            .take(n)
            .map(|(_, ei, i)| want(t, k, &t.employees[ei], i))
            .collect();
        let mut headcount = HeadcountDto::default();
        for e in &t.employees {
            *match e.rank {
                Rank::Bujang => &mut headcount.bujang,
                Rank::Gwajang => &mut headcount.gwajang,
                Rank::Daeri => &mut headcount.daeri,
                Rank::Sawon => &mut headcount.sawon,
            } += 1;
        }
        let dto = SessionDetailDto {
            session_id: t.id.clone(),
            title: t.title.clone(),
            building: building(t.product).to_string(),
            host_label: w
                .hosts
                .get(&k.host)
                .map(|h| h.label.clone())
                .unwrap_or_default(),
            work_type: t.work_type,
            cwd: t.cwd.display().to_string(),
            git_branch: t.git_branch.clone(),
            model: t.employees[0].model.clone(),
            started: t.started,
            last_activity: t.last_activity,
            turns: t.turns,
            tokens: t.tokens,
            headcount,
            pr: t.props.pr.as_ref().map(|(number, url)| PrDto {
                number: *number,
                url: url.clone(),
            }),
            recent: Vec::new(),
            recent_error: None,
        };
        (dto, wants, host_ready(&w, k, t))
    };
    let recent = ready.and_then(|wsl| {
        if wants.is_empty() {
            return Ok(Vec::new());
        }
        bounded(world, wsl, move |deadline| read_events(&wants, deadline))
    });
    Ok(match recent {
        Ok(recent) => SessionDetailDto { recent, ..dto },
        Err(e) => SessionDetailDto {
            recent_error: Some(e),
            ..dto
        },
    })
}

/// 백엔드가 아는 경로만 탐색기로 연다
pub fn open_path(world: &Mutex<OfficeWorld>, target: OpenTarget) -> Result<(), DetailError> {
    let (plan, wsl) = resolve_open(world, target)?;
    bounded(world, wsl, move |deadline| launch(plan, deadline))
}

/// 대상을 경로로 바꾼다. 파일은 건드리지 않는다
fn resolve_open(
    world: &Mutex<OfficeWorld>,
    target: OpenTarget,
) -> Result<(Launch, Option<String>), DetailError> {
    let w = lock(world);
    match target {
        OpenTarget::SourceFolder { office_id, team_id } => {
            let (k, t) = find_team(&w, &parse_office(&office_id)?, &team_id)?;
            let wsl = host_ready(&w, k, t)?;
            let file = main_file(t).ok_or(DetailError::NotFound)?;
            Ok((Launch::Select(file), wsl))
        }
        OpenTarget::WorkFolder { office_id, team_id } => {
            let (k, t) = find_team(&w, &parse_office(&office_id)?, &team_id)?;
            let wsl = host_ready(&w, k, t)?;
            let kind = &w.hosts.get(&k.host).ok_or(DetailError::HostOffline)?.kind;
            let dir =
                work_dir(&t.cwd, kind, main_file(t).as_deref()).ok_or(DetailError::OpenFailed)?;
            Ok((Launch::Folder(dir), wsl))
        }
        OpenTarget::SourceLine {
            office_id,
            team_id,
            agent_id,
            cursor,
            line_idx,
        } => {
            let (k, t, e) = find_employee(&w, &parse_office(&office_id)?, &team_id, &agent_id)?;
            let wsl = host_ready(&w, k, t)?;
            let i = find_mark(&e.record.timeline, cursor, line_idx)?;
            Ok((Launch::Select(want(t, k, e, i).path), wsl))
        }
        OpenTarget::Sidecar {
            office_id,
            team_id,
            agent_id,
            cursor,
            line_idx,
        } => {
            let (k, t, e) = find_employee(&w, &parse_office(&office_id)?, &team_id, &agent_id)?;
            let wsl = host_ready(&w, k, t)?;
            let i = find_mark(&e.record.timeline, cursor, line_idx)?;
            if e.record.timeline[i].kind != TimelineKind::ToolResult {
                return Err(DetailError::CursorInvalid);
            }
            Ok((Launch::Sidecar(want(t, k, e, i)), wsl))
        }
    }
}

/// 세션 원본 파일: 부장의 첫 칸이 나온 파일, 없으면 서브 기록이 아닌 첫 파일
fn main_file(t: &Team) -> Option<PathBuf> {
    t.employees[0]
        .record
        .timeline
        .first()
        .and_then(|m| t.sources.get(m.source as usize))
        .or_else(|| t.sources.iter().find(|p| !is_subagent(p)))
        .cloned()
}

/// 작업 폴더. 기록에 적힌 값이라 믿지 않는다: Windows 는 로컬 드라이브 경로만
/// (UNC 를 열면 그 서버에 접속한다), WSL 은 리눅스 절대 경로를 그 배포판 공유 아래로 옮긴 것만
fn work_dir(cwd: &Path, kind: &HostKind, main: Option<&Path>) -> Option<PathBuf> {
    match kind {
        HostKind::Windows => {
            let local = matches!(
                cwd.components().next(),
                Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
            );
            let up = cwd.components().any(|c| c == Component::ParentDir);
            (local && !up).then(|| cwd.to_path_buf())
        }
        HostKind::Wsl { .. } => {
            let s = cwd.to_str()?;
            let rest = s.strip_prefix('/')?;
            // `C:` 같은 조각은 PathBuf::join 이 앞 경로를 통째로 바꾼다
            if s.contains('\\') || rest.split('/').any(|p| p == ".." || p.contains(':')) {
                return None;
            }
            let root = wsl_root(main?)?;
            let dir = rest
                .split('/')
                .filter(|p| !p.is_empty())
                .fold(root.clone(), |acc, p| acc.join(p));
            dir.starts_with(&root).then_some(dir)
        }
    }
}

/// `\\wsl.localhost\<배포판>\` 또는 `\\wsl$\<배포판>\`. 세션 파일 경로에서 뗀다
fn wsl_root(p: &Path) -> Option<PathBuf> {
    let Some(Component::Prefix(px)) = p.components().next() else {
        return None;
    };
    let (Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share)) = px.kind() else {
        return None;
    };
    let server = server.to_str()?;
    if !["wsl.localhost", "wsl$"]
        .iter()
        .any(|s| server.eq_ignore_ascii_case(s))
    {
        return None;
    }
    Some(PathBuf::from(format!(r"\\{server}\{}\", share.to_str()?)))
}

/// explorer 에 줄 인자. 끝의 `\` 는 닫는 따옴표를 이스케이프하므로 두 배로 한다 (루트 `C:\`, `\\wsl.localhost\배포판\`)
fn explorer_arg(path: &str, select: bool) -> String {
    let slashes = path.len() - path.trim_end_matches('\\').len();
    let quoted = format!("\"{path}{}\"", "\\".repeat(slashes));
    if select {
        format!("/select,{quoted}")
    } else {
        quoted
    }
}

/// 결과 파일 경로. 그 자리의 줄이 같은 기록(시각, 호출 ID)일 때만 줄에 적힌 경로를 따른다.
/// 파일을 여는 일이라 화면보다 엄격하게 timestamp 없는 줄은 받지 않는다 (find_event 는 받는다)
fn sidecar_path(w: &Want) -> Result<PathBuf, DetailError> {
    let mut file = Lines::open(&w.path, w.vendor).ok_or(DetailError::OpenFailed)?;
    let line = file
        .at(&w.path, &w.mark, true)
        .ok_or(DetailError::OpenFailed)?;
    let rec = line
        .rec
        .as_ref()
        .filter(|r| raw_ts_matches(r, w.mark.ts))
        .ok_or(DetailError::CursorInvalid)?;
    let text = raw_result(rec, w.mark.call, w.vendor)
        .ok_or(DetailError::CursorInvalid)?
        .text;
    let dir = session_dir(&w.path).ok_or(DetailError::NotFound)?;
    sidecar(Some(rec), &text, &dir).ok_or(DetailError::NotFound)
}

fn launch(plan: Launch, deadline: Instant) -> Result<(), DetailError> {
    let (path, select) = match plan {
        Launch::Select(p) => (p, true),
        Launch::Folder(p) => (p, false),
        Launch::Sidecar(w) => (sidecar_path(&w)?, true),
    };
    // 따옴표는 인자 경계를 깬다. Windows 경로에는 없지만 WSL 경로에는 있을 수 있다
    let s = path
        .to_str()
        .filter(|s| !s.contains('"'))
        .ok_or(DetailError::OpenFailed)?;
    // 폴더가 아닌 것(실행 파일 등)을 그냥 넘기면 explorer 가 실행한다. 파일은 고르기로만 연다
    let ok = if select {
        path.is_file()
    } else {
        path.is_dir()
    };
    if !ok {
        return Err(DetailError::OpenFailed);
    }
    // 위 검사가 UNC 에서 오래 걸렸으면 호출한 쪽은 이미 Timeout 을 받았다. 그 뒤에 창을 띄우지 않는다
    if Instant::now() > deadline {
        return Err(DetailError::Timeout);
    }
    // ponytail: 폴더를 열 때 is_dir 검사와 실행 사이에 폴더를 실행 파일로 바꿔치는 틈이 남는다, 폴더만 받는 explorer 인자 형식을 확실히 알게 되면 바꾼다 (`/select,` 는 실행하지 않아 파일 쪽은 안전하다)
    Command::new("explorer.exe")
        .raw_arg(explorer_arg(s, select))
        .spawn()
        .map(drop)
        .map_err(|_| DetailError::OpenFailed)
}

/// 파일 일을 다른 스레드에서 하고 상한까지만 기다린다. 멈춘 UNC 읽기 안의 스레드는 자기 시간을 잴 수 없다.
/// 상한을 넘긴 스레드는 버려 두고(줄 사이마다 기한을 보고 스스로 그만둔다), WSL 이면 그 호스트를 의심으로
/// 돌려 Probe 가 켜짐을 볼 때까지, 그리고 그 스레드가 끝날 때까지 새 읽기를 막는다
fn bounded<T: Send + 'static>(
    world: &Mutex<OfficeWorld>,
    wsl: Option<String>,
    work: impl FnOnce(Instant) -> Result<T, DetailError> + Send + 'static,
) -> Result<T, DetailError> {
    let deadline = Instant::now() + READ_LIMIT;
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicBool::new(false));
    let guard = Done(done.clone());
    thread::spawn(move || {
        let _guard = guard;
        let _ = tx.send(work(deadline));
    });
    let r = rx
        .recv_timeout(READ_LIMIT)
        .unwrap_or(Err(DetailError::Timeout));
    if matches!(r, Err(DetailError::Timeout))
        && let Some(host) = wsl
    {
        if !done.load(Ordering::Relaxed) {
            HUNG.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((host.clone(), done));
        }
        lock(world).set_suspect(&host, true);
    }
    r
}

fn needs_raw(kind: TimelineKind) -> bool {
    matches!(kind, TimelineKind::ToolCall | TimelineKind::ToolResult)
}

/// 칸들의 DTO. 입력 순서 그대로. 줄마다 DTO 를 바로 만들고 원문은 버린다
fn read_events(wants: &[Want], deadline: Instant) -> Result<Vec<EventDto>, DetailError> {
    // 파일 → 시작 바이트 → (그 줄의 칸, 그 줄의 칸 번호들).
    // 칸 번호가 빈 줄은 결과의 짝 줄이라 파서 문맥으로만 읽는다. 바이트 순이라 결과 줄보다 먼저 읽힌다
    type ByOffset<'a> = BTreeMap<u64, (&'a Mark, Vec<usize>)>;
    let mut files: BTreeMap<(&Path, Vendor), ByOffset> = BTreeMap::new();
    for (i, w) in wants.iter().enumerate() {
        let lines = files.entry((&w.path, w.vendor)).or_default();
        lines
            .entry(w.mark.byte_offset)
            .or_insert((&w.mark, Vec::new()))
            .1
            .push(i);
        for p in &w.pair {
            lines.entry(p.byte_offset).or_insert((p, Vec::new()));
        }
    }
    let mut out: Vec<Option<EventDto>> = vec![None; wants.len()];
    for ((path, vendor), lines) in files {
        // 못 열면 칸마다 "못 읽음"이 된다
        let mut file = Lines::open(path, vendor);
        for (mark, idxs) in lines.into_values() {
            if Instant::now() > deadline {
                return Err(DetailError::Timeout);
            }
            let raw = idxs.iter().any(|&i| needs_raw(wants[i].mark.kind));
            let line = file.as_mut().and_then(|f| f.at(path, mark, raw));
            for i in idxs {
                out[i] = Some(event_dto(&wants[i], line.as_ref()));
            }
        }
    }
    Ok(out.into_iter().flatten().collect())
}

impl Lines {
    fn open(path: &Path, vendor: Vendor) -> Option<Self> {
        Some(Self {
            reader: BufReader::new(File::open(path).ok()?),
            parser: make_parser(path, vendor),
        })
    }

    /// 그 칸의 줄. 미대응 재료는 처음 읽을 때 이미 셌으니 버린다.
    /// 원문 JSON 은 `raw` 일 때만 파싱한다 (큰 줄을 필요 없이 두 번 파싱하지 않으려고)
    fn at(&mut self, path: &Path, m: &Mark, raw: bool) -> Option<Line> {
        let mut buf = Vec::new();
        self.reader.seek(SeekFrom::Start(m.byte_offset)).ok()?;
        (&mut self.reader)
            .take(LINE_MAX)
            .read_until(b'\n', &mut buf)
            .ok()?;
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        let at = RawRef {
            path: path.to_path_buf(),
            generation: m.generation,
            byte_offset: m.byte_offset,
            line_no: m.line_no,
        };
        let events = self.parser.parse_line(&buf, &at, &mut Vec::new());
        Some(Line {
            rec: raw.then(|| serde_json::from_slice(&buf).ok()).flatten(),
            events,
        })
    }
}

fn event_dto(w: &Want, line: Option<&Line>) -> EventDto {
    let m = &w.mark;
    let mut d = EventDto {
        cursor: CursorDto {
            generation: m.generation,
            byte_offset: m.byte_offset,
        },
        agent_id: w.agent.clone(),
        ts: m.ts,
        kind: m.kind,
        summary: String::new(),
        confidence: m
            .outcome
            .map_or(crate::parse::Confidence::Confirmed, |o| o.1),
        tool: None,
        call_id: None,
        input: None,
        outcome: m.outcome.map(|o| o.0),
        body: None,
        path: w.path.display().to_string(),
        line_no: m.line_no,
        line_idx: w.line_idx,
        sidecar: None,
    };
    let found = line.and_then(|l| find_event(m, &l.events).map(|e| (l, &e.kind)));
    let Some((line, ev)) = found else {
        // 결과 칸은 요약이 Mark 에서, 본문·call_id 가 원문에서 오니 파서 이벤트 없이도 채운다. Codex 백그라운드
        // 완료·write_stdin 중단·Claude TaskStop 결과는 다른 줄의 파서 상태가 있어야 이벤트가 나서 페이지가
        // 갈리면 못 찾는다. 파일이 바뀐 자리에 다른 줄이 있을 수 있으니 원문 timestamp 가 있고 칸 ts 와 같을 때만
        // 받는다 (find_event 보다 엄격하다: 이벤트 없이 메모리 요약만 나가는 길이라 확인할 근거가 없으면 막는다)
        match line {
            Some(l)
                if m.kind == TimelineKind::ToolResult
                    && l.rec.as_ref().is_some_and(|r| raw_ts_matches(r, m.ts)) =>
            {
                fill_result(&mut d, w, m, None, l.rec.as_ref());
            }
            _ => d.summary = UNREADABLE.to_string(),
        }
        return d;
    };
    let rec = line.rec.as_ref();
    match m.kind {
        TimelineKind::ToolCall => {
            if let EventKind::ToolCall {
                call_id,
                tool,
                summary,
            } = ev
            {
                d.call_id = Some(call_id.clone());
                d.summary = call_summary(tool, summary, m);
            }
            if let Some(raw) = rec.and_then(|r| raw_call(r, m.call, w.vendor)) {
                d.tool = Some(raw.name);
                d.call_id = Some(raw.id);
                d.input = Some(clip(&raw.input, BODY_CLIP));
            }
        }
        TimelineKind::ToolResult => fill_result(&mut d, w, m, Some(ev), rec),
        _ => fill_other(&mut d, ev),
    }
    if d.summary.is_empty() {
        d.summary = MISSING.to_string();
    }
    d
}

fn raw_ts_matches(rec: &Value, ts: chrono::DateTime<chrono::Utc>) -> bool {
    rec.get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|t| t == ts)
}

/// 결과 칸 채우기. `ev` 가 없으면(파서가 이 줄만으론 이벤트를 못 냄) 요약과 원문에서 오는 것만 채운다
fn fill_result(d: &mut EventDto, w: &Want, m: &Mark, ev: Option<&EventKind>, rec: Option<&Value>) {
    d.summary = result_summary(m);
    if let Some(EventKind::ToolResult {
        call_id, preview, ..
    }) = ev
    {
        d.call_id = Some(call_id.clone());
        if !preview.is_empty() {
            d.body = Some(clip(preview, BODY_CLIP));
        }
    }
    // 결과 본문은 파서 미리보기(앞 200자)가 아니라 원문에서 앞뒤를 자른다
    if let Some(raw) = rec.and_then(|r| raw_result(r, m.call, w.vendor)) {
        if w.vendor == Vendor::Claude {
            d.sidecar = session_dir(&w.path)
                .and_then(|dir| sidecar(rec, &raw.text, &dir))
                .map(|p| p.display().to_string());
        }
        d.call_id = Some(raw.id);
        d.body = Some(clip(&raw.text, BODY_CLIP));
    }
}

fn call_id_of(k: &EventKind) -> Option<&str> {
    match k {
        EventKind::ToolCall { call_id, .. } | EventKind::ToolResult { call_id, .. } => {
            Some(call_id)
        }
        _ => None,
    }
}

/// 다시 파싱한 줄에서 그 칸의 이벤트. 호출·결과는 call_id 로, 그 밖은 같은 종류의 몇 번째로 찾는다.
/// 파일이 바뀌어 같은 자리에 다른 줄이 있을 수 있어 ts 가 칸과 다르면 짝짓지 않는다
/// (ts 없는 기록은 직전 ts 를 쓴 값이라 견줄 수 없어 그대로 받는다)
fn find_event<'a>(m: &Mark, events: &'a [Event]) -> Option<&'a Event> {
    let mut same = events
        .iter()
        .filter(|e| TimelineKind::of(&e.kind) == Some(m.kind));
    let ev = match m.kind {
        TimelineKind::ToolCall | TimelineKind::ToolResult => {
            same.find(|e| call_id_of(&e.kind).is_some_and(|c| call_hash(c) == m.call))
        }
        _ => same.nth(m.nth as usize),
    };
    ev.filter(|e| e.ts.is_none_or(|t| t == m.ts))
}

fn call_summary(tool: &ToolKind, summary: &ToolSummary, m: &Mark) -> String {
    let call = PendingCall {
        call_id: String::new(),
        tool: tool.clone(),
        summary: summary.clone(),
        started: m.ts,
        big_read: false,
    };
    let (label, subject) = (sentence::label(tool), sentence::subject(&call));
    if label == subject {
        label
    } else {
        format!("{label} {subject}")
    }
}

fn result_summary(m: &Mark) -> String {
    let word = match m.outcome.map(|o| o.0) {
        Some(ToolOutcome::Ok) => "성공",
        Some(ToolOutcome::Err) => "실패",
        Some(ToolOutcome::Rejected) => "반려",
        Some(ToolOutcome::Started) => "백그라운드 시작",
        Some(ToolOutcome::Unknown) | None => "결과 미확인",
    };
    match m.test {
        Some(t) => format!("{word} · 합격 {} / 반려 {}", t.passed, t.failed),
        None => word.to_string(),
    }
}

fn fill_other(d: &mut EventDto, ev: &EventKind) {
    let cut = |s: &str| sentence::cut(s, SUMMARY_CHARS);
    let body = |s: &str| (!s.is_empty()).then(|| clip(s, BODY_CLIP));
    match ev {
        EventKind::UserMessage { text } | EventKind::AssistantText { text } => {
            d.summary = cut(text);
            d.body = body(text);
        }
        EventKind::SubagentSpawn {
            prompt,
            description,
            ..
        } => {
            d.summary = format!(
                "업무 지시: {}",
                cut(description.as_deref().unwrap_or(prompt))
            );
            d.body = body(prompt);
        }
        EventKind::SubagentDone { .. } => d.summary = "업무 완료 보고".to_string(),
        EventKind::PermissionRequest { question, plan } => {
            d.summary = if *plan {
                "계획 결재 요청".to_string()
            } else {
                format!("결재 요청: {}", cut(question))
            };
            d.body = body(question);
        }
        EventKind::ContextCompaction {
            pre_tokens,
            post_tokens,
        } => {
            d.summary = match (pre_tokens, post_tokens) {
                (Some(a), Some(b)) => format!("분갈이 (컨텍스트 {a} → {b} 토큰)"),
                _ => "분갈이 (컨텍스트 압축)".to_string(),
            };
        }
        EventKind::TitleChanged { title } => d.summary = format!("사규 액자 교체: {}", cut(title)),
        EventKind::PrLinked { number, url } => {
            d.summary = format!("PR #{number} 연결");
            d.body = body(url);
        }
        _ => {}
    }
}

/// 원문을 앞뒤 `n` 바이트로. UTF-8 글자 경계에서 자른다. 안 잘렸으면 `tail` 이 빈 글이다
fn clip(text: &str, n: usize) -> ClipDto {
    let bytes = text.len() as u64;
    if text.len() <= n * 2 {
        return ClipDto {
            head: text.to_string(),
            tail: String::new(),
            bytes,
        };
    }
    ClipDto {
        head: text[..text.floor_char_boundary(n)].to_string(),
        tail: text[text.ceil_char_boundary(text.len() - n)..].to_string(),
        bytes,
    }
}

struct RawCall {
    id: String,
    name: String,
    input: String,
}

struct RawResult {
    id: String,
    text: String,
}

fn hashed(v: &Value, key: &str, call: u64) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|id| call_hash(id) == call)
        .map(str::to_string)
}

/// Codex 필드: 문자열이면 그대로, 그 밖은 JSON 그대로 (파서 미리보기와 같은 규칙)
fn codex_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 도구 호출의 원래 이름과 입력 원문 (Claude `tool_use`, Codex `function_call`·`custom_tool_call`)
fn raw_call(rec: &Value, call: u64, vendor: Vendor) -> Option<RawCall> {
    match vendor {
        Vendor::Claude => rec
            .pointer("/message/content")?
            .as_array()?
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .find_map(|b| {
                Some(RawCall {
                    id: hashed(b, "id", call)?,
                    name: b.get("name")?.as_str()?.to_string(),
                    input: b.get("input").map(Value::to_string).unwrap_or_default(),
                })
            }),
        Vendor::Codex => {
            let p = rec.get("payload")?;
            let key = match p.get("type")?.as_str()? {
                "function_call" => "arguments",
                "custom_tool_call" => "input",
                _ => return None,
            };
            Some(RawCall {
                id: hashed(p, "call_id", call)?,
                name: p.get("name")?.as_str()?.to_string(),
                input: codex_text(p.get(key)),
            })
        }
        Vendor::Gemini => None,
    }
}

/// 도구 결과 원문 (Claude `tool_result`, Codex `function_call_output`·`custom_tool_call_output`)
fn raw_result(rec: &Value, call: u64, vendor: Vendor) -> Option<RawResult> {
    match vendor {
        Vendor::Claude => rec
            .pointer("/message/content")?
            .as_array()?
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            .find_map(|b| {
                Some(RawResult {
                    id: hashed(b, "tool_use_id", call)?,
                    text: content_text(b.get("content")),
                })
            }),
        Vendor::Codex => {
            let p = rec.get("payload")?;
            if !matches!(
                p.get("type")?.as_str()?,
                "function_call_output" | "custom_tool_call_output"
            ) {
                return None;
            }
            Some(RawResult {
                id: hashed(p, "call_id", call)?,
                text: codex_text(p.get("output")),
            })
        }
        Vendor::Gemini => None,
    }
}

/// Claude 세션 폴더 `<프로젝트>/<세션>/`. 서브 기록도 결과 파일은 부모 세션 폴더에 둔다 (4.6.1)
fn session_dir(path: &Path) -> Option<PathBuf> {
    if is_subagent(path) {
        path.parent()?.parent().map(Path::to_path_buf)
    } else {
        Some(path.with_extension(""))
    }
}

/// 분리된 큰 결과 파일 (4.6.1). 구조 필드가 먼저, 없으면 본문의 `<persisted-output>` 참조.
/// 레코드 값이라 믿지 않는다: 경로에서 `<세션>/tool-results/<이름>` 만 받아 이 세션 폴더 아래로 다시 만든다.
/// 기록의 절대 경로는 WSL 이면 리눅스 경로라 그대로 열 수도 없다
fn sidecar(rec: Option<&Value>, text: &str, session_dir: &Path) -> Option<PathBuf> {
    let field = rec
        .and_then(|r| r.pointer("/toolUseResult/persistedOutputPath"))
        .and_then(Value::as_str);
    let raw = field.or_else(|| {
        text.contains("<persisted-output>")
            .then(|| {
                text.lines()
                    .flat_map(str::split_whitespace)
                    .find(|t| t.contains("tool-results"))
            })
            .flatten()
    })?;
    let mut parts = raw.rsplit(['/', '\\']);
    let (name, dir, session) = (parts.next()?, parts.next()?, parts.next()?);
    let mine = session_dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case(session));
    let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(':');
    (dir == "tool-results" && mine && plain).then(|| session_dir.join("tool-results").join(name))
}

#[cfg(test)]
mod tests;
