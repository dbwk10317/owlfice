//! 팝업 두 종류(PLAN 3.7)의 재료: 직원 타임라인 페이징, 직원·세션 상세, 탐색기 열기 (4.8).
//! 원문은 메모리에 들지 않고 RawRef 로 원본 줄을 다시 읽는다. 읽은 원문은 돌려주기만 하고 로그에 남기지 않는다 (4.11)

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::discover::{HostKind, Vendor};
use crate::ipc::{
    ClipDto, CursorDto, DetailError, EmployeeDetailDto, EventDto, HeadcountDto, OpenTarget,
    OutcomeDto, PrDto, SessionDetailDto, TokensDto, building, parse_office_id, team_id_str,
};
use crate::office::classify::sentence;
use crate::office::world::{OfficeId, OfficeWorld, TeamKey};
use crate::office::{Employee, Mark, PendingCall, Rank, Team, TimelineKind, call_hash};
use crate::parse::{Event, EventKind, RawRef, ToolKind, ToolOutcome, ToolSummary};
use crate::tail::scheduler::{is_subagent, make_parser};

/// IPC 를 오래 막지 않는 상한 (4.4.2: 켜진 배포판도 UNC 가 멈출 수 있다)
const READ_LIMIT: Duration = Duration::from_secs(2);
const PAGE: u32 = 50;
const PAGE_MAX: u32 = 200;
/// 세션 상세의 최근 사건 (3.7)
const RECENT: usize = 20;
/// 결과·본문은 앞뒤 2KB (3.7)
const BODY_CLIP: usize = 2048;
const INPUT_CLIP: usize = 256;
/// tail 의 줄 상한과 같다. 이보다 긴 줄은 처음부터 이벤트가 되지 못했다
const LINE_MAX: u64 = 64 * 1024 * 1024;
const SUMMARY_CHARS: usize = 60;
const UNREADABLE: &str = "원본 줄을 다시 읽지 못했습니다";
const MISSING: &str = "원본 줄에서 이 기록을 찾지 못했습니다";

/// 다시 읽을 칸 하나. 잠금 안에서 모으고 읽기는 잠금 밖에서 한다 (4.3)
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    agent: String,
    path: PathBuf,
    vendor: Vendor,
    mark: Mark,
}

/// 탐색기로 열 것
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// 파일을 골라 둔 채 그 폴더를 연다
    Select(PathBuf),
    Folder(PathBuf),
    /// 그 칸의 줄을 읽어야 아는 분리된 결과 파일
    Sidecar(Want),
}

/// 다시 읽을 줄: (세대, 시작 바이트, 줄 번호)
type At = (u64, u64, u64);

struct Line {
    rec: Option<Value>,
    events: Vec<Event>,
}

fn lock(m: &Mutex<OfficeWorld>) -> MutexGuard<'_, OfficeWorld> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn parse_office(s: &str) -> Result<OfficeId, DetailError> {
    parse_office_id(s).ok_or(DetailError::NotFound)
}

/// 사무실 안의 그 직원. agent_id 는 사무실 안에서 겹치지 않는다 (부장은 세션 id, 서브는 고유 id)
fn find_employee<'a>(
    w: &'a OfficeWorld,
    office: &OfficeId,
    agent: &str,
) -> Result<(&'a TeamKey, &'a Team, &'a Employee), DetailError> {
    w.teams
        .iter()
        .filter(|(k, _)| w.office_of(k) == *office)
        .find_map(|(k, t)| {
            t.employees
                .iter()
                .find(|e| e.id == agent)
                .map(|e| (k, t, e))
        })
        .ok_or(DetailError::NotFound)
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

/// 꺼졌거나 통신 지연인 WSL 배포판의 파일은 건드리지 않는다. 경로 접근이 배포판을 깨운다 (4.4.2).
/// 모르는 호스트도 같다
fn host_ready(w: &OfficeWorld, key: &TeamKey) -> Result<(), DetailError> {
    match w.hosts.get(&key.host).map(|h| &h.kind) {
        Some(HostKind::Windows) => Ok(()),
        Some(HostKind::Wsl { .. }) if !w.suspect_hosts.contains(&key.host) => Ok(()),
        _ => Err(DetailError::HostOffline),
    }
}

fn want(t: &Team, key: &TeamKey, e: &Employee, m: &Mark) -> Want {
    Want {
        agent: e.id.clone(),
        path: t
            .sources
            .get(m.source as usize)
            .cloned()
            .unwrap_or_default(),
        vendor: key.vendor,
        mark: *m,
    }
}

/// 그 칸이 든 줄의 첫 칸. 한 줄의 이벤트가 페이지 경계에서 갈리지 않게 한다
fn line_start(tl: &[Mark], mut i: usize) -> usize {
    while i > 0 && i < tl.len() && tl[i - 1].same_line(&tl[i]) {
        i -= 1;
    }
    i
}

fn find_mark(e: &Employee, c: CursorDto, kind: Option<TimelineKind>) -> Result<&Mark, DetailError> {
    e.record
        .timeline
        .iter()
        .rev()
        .filter(|m| kind.is_none_or(|k| m.kind == k))
        .find(|m| m.generation == c.generation && m.byte_offset == c.byte_offset)
        .ok_or(DetailError::CursorInvalid)
}

/// 직원 타임라인 한 페이지, 최신부터 (4.8 `get_employee_timeline`)
pub fn employee_timeline(
    world: &Mutex<OfficeWorld>,
    office_id: &str,
    agent_id: &str,
    before: Option<CursorDto>,
    limit: Option<u32>,
) -> Result<Vec<EventDto>, DetailError> {
    let office = parse_office(office_id)?;
    let wants: Vec<Want> = {
        let w = lock(world);
        let (k, t, e) = find_employee(&w, &office, agent_id)?;
        let tl = &e.record.timeline;
        let end = match before {
            None => tl.len(),
            Some(c) => {
                let at = tl
                    .iter()
                    .rposition(|m| m.generation == c.generation && m.byte_offset == c.byte_offset)
                    .ok_or(DetailError::CursorInvalid)?;
                line_start(tl, at)
            }
        };
        let limit = limit.unwrap_or(PAGE).clamp(1, PAGE_MAX) as usize;
        let start = line_start(tl, end.saturating_sub(limit));
        if start == end {
            return Ok(Vec::new());
        }
        host_ready(&w, k)?;
        tl[start..end]
            .iter()
            .rev()
            .map(|m| want(t, k, e, m))
            .collect()
    };
    bounded(move |deadline| read_events(&wants, deadline))
}

/// 직원 상세 팝업 중 메모리에 있는 것 (3.7)
pub fn employee_detail(
    world: &Mutex<OfficeWorld>,
    office_id: &str,
    agent_id: &str,
) -> Result<EmployeeDetailDto, DetailError> {
    let office = parse_office(office_id)?;
    let w = lock(world);
    let (_, t, e) = find_employee(&w, &office, agent_id)?;
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
        files_read: r.files_read.clone(),
        files_edited: r.files_edited.clone(),
        commands: r.commands.clone(),
    })
}

/// 세션 상세 팝업 (3.7, 4.8 `get_session_detail`)
pub fn session_detail(
    world: &Mutex<OfficeWorld>,
    office_id: &str,
    team_id: &str,
) -> Result<SessionDetailDto, DetailError> {
    let office = parse_office(office_id)?;
    let (dto, wants) = {
        let w = lock(world);
        let (k, t) = find_team(&w, &office, team_id)?;
        host_ready(&w, k)?;
        let mut all: Vec<(&Employee, &Mark)> = t
            .employees
            .iter()
            .flat_map(|e| e.record.timeline.iter().map(move |m| (e, m)))
            .collect();
        all.sort_by_key(|(_, m)| std::cmp::Reverse(m.seq));
        let wants: Vec<Want> = all
            .into_iter()
            .take(RECENT)
            .map(|(e, m)| want(t, k, e, m))
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
            team_id: team_id_str(k),
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
            tokens: t.tokens.map(|x| TokensDto {
                input: x.input,
                output: x.output,
            }),
            headcount,
            pr: t.props.pr.as_ref().map(|(number, url)| PrDto {
                number: *number,
                url: url.clone(),
            }),
            recent: Vec::new(),
        };
        (dto, wants)
    };
    let recent = bounded(move |deadline| read_events(&wants, deadline))?;
    Ok(SessionDetailDto { recent, ..dto })
}

/// 백엔드가 아는 경로만 탐색기로 연다
pub fn open_path(world: &Mutex<OfficeWorld>, target: OpenTarget) -> Result<(), DetailError> {
    let plan = resolve_open(world, target)?;
    bounded(move |deadline| launch(plan, deadline))
}

/// 대상을 경로로 바꾼다. 파일은 건드리지 않는다
pub fn resolve_open(world: &Mutex<OfficeWorld>, target: OpenTarget) -> Result<Launch, DetailError> {
    let w = lock(world);
    match target {
        OpenTarget::SourceFolder { office_id, team_id } => {
            let (k, t) = find_team(&w, &parse_office(&office_id)?, &team_id)?;
            host_ready(&w, k)?;
            main_file(t)
                .map(Launch::Select)
                .ok_or(DetailError::NotFound)
        }
        OpenTarget::WorkFolder { office_id, team_id } => {
            let (k, t) = find_team(&w, &parse_office(&office_id)?, &team_id)?;
            host_ready(&w, k)?;
            let kind = &w.hosts.get(&k.host).ok_or(DetailError::HostOffline)?.kind;
            work_dir(&t.cwd, kind, main_file(t).as_deref())
                .map(Launch::Folder)
                .ok_or(DetailError::OpenFailed)
        }
        OpenTarget::SourceLine {
            office_id,
            agent_id,
            cursor,
        } => {
            let (k, t, e) = find_employee(&w, &parse_office(&office_id)?, &agent_id)?;
            host_ready(&w, k)?;
            let m = find_mark(e, cursor, None)?;
            Ok(Launch::Select(want(t, k, e, m).path))
        }
        OpenTarget::Sidecar {
            office_id,
            agent_id,
            cursor,
        } => {
            let (k, t, e) = find_employee(&w, &parse_office(&office_id)?, &agent_id)?;
            host_ready(&w, k)?;
            let m = find_mark(e, cursor, Some(TimelineKind::ToolResult))?;
            Ok(Launch::Sidecar(want(t, k, e, m)))
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
            if s.contains('\\') || rest.split('/').any(|p| p == "..") {
                return None;
            }
            let root = wsl_root(main?)?;
            Some(
                rest.split('/')
                    .filter(|p| !p.is_empty())
                    .fold(root, |acc, p| acc.join(p)),
            )
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

fn launch(plan: Launch, deadline: Instant) -> Result<(), DetailError> {
    let (path, select) = match plan {
        Launch::Select(p) => (p, true),
        Launch::Folder(p) => (p, false),
        Launch::Sidecar(w) => {
            let m = &w.mark;
            let lines = read_lines(
                &w.path,
                w.vendor,
                vec![(m.generation, m.byte_offset, m.line_no)],
                deadline,
            )?;
            let line = lines.get(&m.byte_offset).ok_or(DetailError::OpenFailed)?;
            let rec = line.rec.as_ref();
            let text = rec
                .and_then(|r| raw_result(r, m.call, w.vendor))
                .map(|r| r.text)
                .unwrap_or_default();
            let dir = session_dir(&w.path).ok_or(DetailError::NotFound)?;
            (
                sidecar(rec, &text, &dir).ok_or(DetailError::NotFound)?,
                true,
            )
        }
    };
    // 따옴표는 인자 경계를 깬다. Windows 경로에는 없지만 WSL 경로에는 있을 수 있다
    let s = path
        .to_str()
        .filter(|s| !s.contains('"'))
        .ok_or(DetailError::OpenFailed)?;
    // 폴더가 아닌 것(실행 파일 등)을 그냥 넘기면 explorer 가 실행한다. 파일은 고르기로만 연다
    let arg = if select {
        if !path.is_file() {
            return Err(DetailError::OpenFailed);
        }
        format!("/select,\"{s}\"")
    } else {
        if !path.is_dir() {
            return Err(DetailError::OpenFailed);
        }
        format!("\"{s}\"")
    };
    Command::new("explorer.exe")
        .raw_arg(arg)
        .spawn()
        .map(drop)
        .map_err(|_| DetailError::OpenFailed)
}

/// 파일 일을 다른 스레드에서 하고 상한까지만 기다린다. 멈춘 UNC 읽기 안의 스레드는 자기 시간을 잴 수 없다.
/// 상한을 넘긴 스레드는 버려 두고, 그 스레드도 줄 사이마다 기한을 보고 스스로 그만둔다
fn bounded<T: Send + 'static>(
    work: impl FnOnce(Instant) -> Result<T, DetailError> + Send + 'static,
) -> Result<T, DetailError> {
    let deadline = Instant::now() + READ_LIMIT;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(work(deadline));
    });
    rx.recv_timeout(READ_LIMIT)
        .unwrap_or(Err(DetailError::Timeout))
}

fn read_events(wants: &[Want], deadline: Instant) -> Result<Vec<EventDto>, DetailError> {
    let mut files: BTreeMap<(&Path, Vendor), Vec<At>> = BTreeMap::new();
    for w in wants {
        let m = &w.mark;
        files.entry((&w.path, w.vendor)).or_default().push((
            m.generation,
            m.byte_offset,
            m.line_no,
        ));
    }
    let mut read = HashMap::new();
    for ((path, vendor), at) in files {
        read.insert(path, read_lines(path, vendor, at, deadline)?);
    }
    Ok(wants
        .iter()
        .map(|w| {
            let line = read
                .get(w.path.as_path())
                .and_then(|m| m.get(&w.mark.byte_offset));
            event_dto(w, line)
        })
        .collect())
}

/// 파일 하나를 한 번 열어 앞 줄부터 같은 파서에 넣는다. 못 열면 빈 결과라 칸마다 "못 읽음"이 된다
fn read_lines(
    path: &Path,
    vendor: Vendor,
    mut at: Vec<At>,
    deadline: Instant,
) -> Result<HashMap<u64, Line>, DetailError> {
    at.sort_by_key(|a| a.1);
    at.dedup_by_key(|a| a.1);
    let mut out = HashMap::new();
    let Ok(file) = File::open(path) else {
        return Ok(out);
    };
    let mut reader = BufReader::new(file);
    let mut parser = make_parser(path, vendor);
    let mut unhandled = Vec::new();
    for (generation, offset, line_no) in at {
        if Instant::now() > deadline {
            return Err(DetailError::Timeout);
        }
        let mut buf = Vec::new();
        if reader.seek(SeekFrom::Start(offset)).is_err()
            || (&mut reader)
                .take(LINE_MAX)
                .read_until(b'\n', &mut buf)
                .is_err()
        {
            continue;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        let raw = RawRef {
            path: path.to_path_buf(),
            generation,
            byte_offset: offset,
            line_no,
        };
        let events = parser.parse_line(&buf, &raw, &mut unhandled);
        // 미대응 재료는 처음 읽을 때 이미 셌다
        unhandled.clear();
        out.insert(
            offset,
            Line {
                rec: serde_json::from_slice(&buf).ok(),
                events,
            },
        );
    }
    Ok(out)
}

fn outcome_dto(o: ToolOutcome) -> OutcomeDto {
    match o {
        ToolOutcome::Ok => OutcomeDto::Ok,
        ToolOutcome::Err => OutcomeDto::Err,
        ToolOutcome::Rejected => OutcomeDto::Rejected,
        ToolOutcome::Started => OutcomeDto::Started,
        ToolOutcome::Unknown => OutcomeDto::Unknown,
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
        outcome: m.outcome.map(|o| outcome_dto(o.0)),
        body: None,
        path: w.path.display().to_string(),
        line_no: m.line_no,
        sidecar: None,
    };
    let Some(line) = line else {
        d.summary = UNREADABLE.to_string();
        return d;
    };
    let ev = find_event(m, &line.events);
    let rec = line.rec.as_ref();
    match m.kind {
        TimelineKind::ToolCall => {
            if let Some(EventKind::ToolCall {
                call_id,
                tool,
                summary,
            }) = ev
            {
                d.call_id = Some(call_id.clone());
                d.summary = call_summary(tool, summary, m);
            }
            if let Some(raw) = rec.and_then(|r| raw_call(r, m.call, w.vendor)) {
                d.tool = Some(raw.name);
                d.call_id = Some(raw.id);
                d.input = Some(clip(&raw.input, INPUT_CLIP));
            }
        }
        TimelineKind::ToolResult => {
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
        _ => {
            if let Some(ev) = ev {
                fill_other(&mut d, ev);
            }
        }
    }
    if d.summary.is_empty() {
        d.summary = MISSING.to_string();
    }
    d
}

fn call_id_of(k: &EventKind) -> Option<&str> {
    match k {
        EventKind::ToolCall { call_id, .. } | EventKind::ToolResult { call_id, .. } => {
            Some(call_id)
        }
        _ => None,
    }
}

/// 다시 파싱한 줄에서 그 칸의 이벤트. 호출·결과는 call_id 로, 그 밖은 같은 종류의 몇 번째로 찾는다
fn find_event<'a>(m: &Mark, events: &'a [Event]) -> Option<&'a EventKind> {
    let mut same = events
        .iter()
        .map(|e| &e.kind)
        .filter(|k| TimelineKind::of(k) == Some(m.kind));
    match m.kind {
        TimelineKind::ToolCall | TimelineKind::ToolResult => {
            same.find(|k| call_id_of(k).is_some_and(|c| call_hash(c) == m.call))
        }
        _ => same.nth(m.nth as usize),
    }
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

/// 원문을 앞뒤 `n` 바이트로. UTF-8 글자 경계에서 자른다
fn clip(text: &str, n: usize) -> ClipDto {
    let bytes = text.len() as u64;
    if text.len() <= n * 2 {
        return ClipDto {
            head: text.to_string(),
            tail: String::new(),
            truncated: false,
            bytes,
        };
    }
    ClipDto {
        head: text[..text.floor_char_boundary(n)].to_string(),
        tail: text[text.ceil_char_boundary(text.len() - n)..].to_string(),
        truncated: true,
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
                input: text_of(p.get(key)),
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
                    text: text_of(b.get("content")),
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
                text: text_of(p.get("output")),
            })
        }
        Vendor::Gemini => None,
    }
}

/// 문자열이면 그대로, text 블록 배열이면 이어서, 그 밖은 JSON 그대로
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
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
