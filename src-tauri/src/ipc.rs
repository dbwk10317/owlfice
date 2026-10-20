//! IPC 계약 (PLAN 4.8). 프론트 타입은 여기서 ts-rs 로 만든다 (`src/types/ipc.ts`)

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use ts_rs::TS;

use crate::discover::{Host, HostKind, Product, Vendor, windows_host};
use crate::office::names::RoomBook;
use crate::office::world::{OfficeId, OfficeWorld, Scale, TeamKey};
use crate::office::{
    Activity, Counters, Cue, Employee, Incident, Light, Rank, Team, TeamPhase, TeamProps,
};
use crate::parse::{Confidence, TodoItem};

/// 최대 5회/초 (4.8)
const PUBLISH_EVERY: StdDuration = StdDuration::from_millis(200);

/// `office-list` 페이로드
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OfficeListDto {
    pub list_seq: u64,
    pub rebuilt: bool,
    /// 사이드바 구조(동별 목록, 층 고정 행)가 규모로 갈린다
    pub scale: Scale,
    pub offices: Vec<OfficeSummaryDto>,
}

/// `get_world_snapshot` 반환. 목록 필드를 그대로 품어 store 가 목록처럼 받는다
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorldDto {
    #[serde(flatten)]
    pub list: OfficeListDto,
    pub selected: Option<OfficeDto>,
}

/// 사이드바 한 줄 (3.3). 대기업은 팀 하나, 중소기업은 층 하나
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OfficeSummaryDto {
    pub office_id: String,
    /// 대기업만: 동 글자 (A~F)
    pub building: Option<String>,
    pub floor: Option<u32>,
    /// 대기업만. 아직 번호를 못 받았으면 null
    pub room: Option<u32>,
    pub label: String,
    /// 팀 중 가장 활발한 단계
    pub phase: TeamPhase,
    /// 팀 중 가장 급한 점 (3.3 중소기업 층 점)
    pub dot: Dot,
    pub asking: bool,
    pub border: Border,
    pub delayed: bool,
    pub teams: Vec<TeamSummaryDto>,
}

/// 팀 한 줄 (3.3). 대기업에서는 사무실 줄의 둘째 줄 재료
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummaryDto {
    pub team_id: String,
    pub boss_name: String,
    pub title: String,
    /// 작업 폴더 이름만. 전체 경로는 세션 상세가 준다
    pub folder: String,
    /// "부장 외 n명": 퇴장하지 않은 서브 수
    pub others: u32,
    /// 활성 경과는 프론트가 이것으로 센다. 매초 다시 보내지 않으려고
    pub started: DateTime<Utc>,
    pub phase: TeamPhase,
    pub dot: Dot,
    /// 점 옆 결재판 (허가 대기)
    pub asking: bool,
    pub border: Border,
    /// 통신 지연 (점선 테두리, WSL 의심 상태)
    pub delayed: bool,
    /// 호스트 배지 (`WSL`). Windows 는 null
    pub host_badge: Option<String>,
}

/// 상태 점 (3.3 범례). 선언 순서가 급한 순서다 (장애 > 근무 > 결재·응답 대기 > 졸음 > 퇴근)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
pub enum Dot {
    Incident,
    Working,
    /// 초록 깜빡임: 결재 대기 또는 응답 대기
    Waiting,
    Dozing,
    Off,
}

/// 줄 테두리 (3.3 범례)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum Border {
    Plain,
    /// 마지막 빌드·테스트 실패
    Failed,
    /// 빌드·테스트 진행 중
    Running,
}

/// `office-state` 페이로드. 스냅샷의 `selected`, `select_office` 응답도 같은 꼴이다
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDto {
    pub state_seq: u64,
    pub office_id: String,
    pub rebuilt: bool,
    pub teams: Vec<TeamDto>,
    pub shared: SharedPropsDto,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SelectResponseDto {
    #[serde(flatten)]
    pub office: OfficeDto,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TeamDto {
    pub team_id: String,
    pub title: String,
    pub product: Product,
    pub building: String,
    pub host_badge: Option<String>,
    pub folder: String,
    pub started: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub phase: TeamPhase,
    /// 0~3 팀장석, 4~5 간이 책상, null 은 카드만 (4.7.9)
    pub desk_slot: Option<u8>,
    /// 첫 번째가 부장
    pub employees: Vec<EmployeeDto>,
    pub props: TeamProps,
    pub incident: Option<Incident>,
}

/// 직원 카드 한 장 (3.5)
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EmployeeDto {
    pub agent_id: String,
    pub name: String,
    pub rank: Rank,
    pub model: Option<String>,
    pub description: Option<String>,
    pub activity: Activity,
    /// 최근 행적, 최신이 먼저
    pub recent: Vec<Activity>,
    pub counters: Counters,
    pub last_say: Option<String>,
    pub hired_at: DateTime<Utc>,
    /// 있으면 퇴장한(또는 퇴장으로 짐작한) 직원
    pub retired_at: Option<DateTime<Utc>>,
    pub seat: Option<u8>,
    /// 상사 agent_id. 깊이 2 이상 카드의 "상사" 줄
    pub parent: Option<String>,
    pub depth: u8,
    pub confidence: Confidence,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
}

/// 사무실 공유 소품 (4.7.9 합성 규칙). 대기업은 팀 하나라 그 팀 값과 같다
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedPropsDto {
    /// 가장 최근 활동한 팀 중 신호등이 켜진 팀의 색
    pub light: Light,
    /// 무재해 = 팀 중 최솟값이라 가장 늦은 시작 시각
    pub safe_since: DateTime<Utc>,
    pub trash: u32,
    pub pizza: u32,
    /// 사규 액자와 칸반은 가장 최근 활동 팀 것
    pub frame_title: String,
    pub kanban: Vec<TodoItem>,
    /// 장애 상황실 = 어느 팀이든 장애
    pub incident: bool,
    pub night: bool,
}

/// 대기업 `team/<vendor>/<session>/<host>`, 중소기업 `vendor/<vendor>`.
/// 호스트 id 에 `:` 가 있고(`wsl:Ubuntu`) 세션 id 는 파일 이름이라 `/` 가 없어서 호스트를 끝에 둔다
pub fn office_id_str(id: &OfficeId) -> String {
    match id {
        OfficeId::Team(k) => team_id_str(k),
        OfficeId::Vendor(v) => format!("vendor/{}", vendor_str(*v)),
    }
}

fn team_id_str(k: &TeamKey) -> String {
    format!("team/{}/{}/{}", vendor_str(k.vendor), k.session, k.host)
}

/// 프론트가 보낸 값이라 형식이 틀리면 None
pub fn parse_office_id(s: &str) -> Option<OfficeId> {
    let mut it = s.splitn(4, '/');
    match (it.next()?, it.next(), it.next(), it.next()) {
        ("vendor", Some(v), None, None) => Some(OfficeId::Vendor(parse_vendor(v)?)),
        ("team", Some(v), Some(session), Some(host)) if !session.is_empty() && !host.is_empty() => {
            Some(OfficeId::Team(TeamKey {
                host: host.to_string(),
                vendor: parse_vendor(v)?,
                session: session.to_string(),
            }))
        }
        _ => None,
    }
}

fn vendor_str(v: Vendor) -> &'static str {
    match v {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Gemini => "gemini",
    }
}

fn parse_vendor(s: &str) -> Option<Vendor> {
    [Vendor::Claude, Vendor::Codex, Vendor::Gemini]
        .into_iter()
        .find(|v| vendor_str(*v) == s)
}

/// 중소기업 층 (2.1)
fn vendor_floor(v: Vendor) -> u32 {
    match v {
        Vendor::Claude => 1,
        Vendor::Codex => 2,
        Vendor::Gemini => 3,
    }
}

/// 동 (2.1)
fn building(p: Product) -> &'static str {
    match p {
        Product::ClaudeDesktop => "A",
        Product::ClaudeCli => "B",
        Product::CodexApp => "C",
        Product::CodexCli => "D",
        Product::AntigravityApp => "E",
        Product::AntigravityCli => "F",
    }
}

/// RoomBook 은 호스트 종류와 id 만 본다
fn host_of(id: &str) -> Host {
    let win = windows_host();
    if id == win.id {
        return win;
    }
    let distro = id.strip_prefix("wsl:").unwrap_or(id).to_string();
    Host {
        id: id.to_string(),
        kind: HostKind::Wsl {
            distro: distro.clone(),
        },
        label: distro,
    }
}

fn host_badge(host: &str) -> Option<String> {
    (host != windows_host().id).then(|| "WSL".to_string())
}

fn folder(t: &Team) -> String {
    t.cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn active_subs(t: &Team) -> impl Iterator<Item = &Employee> {
    t.employees
        .iter()
        .skip(1)
        .filter(|e| e.retired_at.is_none())
}

fn team_dot(t: &Team, now: DateTime<Utc>) -> Dot {
    if t.incident.is_some() {
        return Dot::Incident;
    }
    match t.phase {
        TeamPhase::Off => Dot::Off,
        TeamPhase::Dozing => Dot::Dozing,
        TeamPhase::Working => {
            // 서브가 일하는 동안 부장이 기다리는 것은 팀이 일하는 것이다
            let boss = &t.employees[0];
            let idle_subs = active_subs(t).all(|e| e.pending.is_empty() && e.background.is_empty());
            if t.awaiting(boss, now) && idle_subs {
                Dot::Waiting
            } else {
                Dot::Working
            }
        }
    }
}

fn team_asking(t: &Team, now: DateTime<Utc>) -> bool {
    t.employees
        .iter()
        .filter(|e| e.retired_at.is_none())
        .any(|e| t.asking(e, now))
}

fn border(light: Light) -> Border {
    match light {
        Light::Red => Border::Failed,
        Light::Yellow => Border::Running,
        Light::Off | Light::Green => Border::Plain,
    }
}

/// 목록 정렬과 사무실 단계 합성용. 활발한 것이 작다
fn phase_rank(p: TeamPhase) -> u8 {
    match p {
        TeamPhase::Working => 0,
        TeamPhase::Dozing => 1,
        TeamPhase::Off => 2,
    }
}

fn team_summary(w: &OfficeWorld, k: &TeamKey, t: &Team, now: DateTime<Utc>) -> TeamSummaryDto {
    TeamSummaryDto {
        team_id: team_id_str(k),
        boss_name: t.employees[0].name.clone(),
        title: t.title.clone(),
        folder: folder(t),
        others: active_subs(t).count() as u32,
        started: t.started,
        phase: t.phase,
        dot: team_dot(t, now),
        asking: team_asking(t, now),
        border: border(t.props.light),
        delayed: w.suspect_hosts.contains(&k.host),
        host_badge: host_badge(&k.host),
    }
}

/// 사무실마다 팀을 팀장석 순으로
fn grouped(w: &OfficeWorld) -> BTreeMap<OfficeId, Vec<(&TeamKey, &Team)>> {
    let mut by: BTreeMap<OfficeId, Vec<(&TeamKey, &Team)>> = BTreeMap::new();
    for (k, t) in &w.teams {
        by.entry(w.office_of(k)).or_default().push((k, t));
    }
    for teams in by.values_mut() {
        teams.sort_by_key(|(k, t)| (t.desk_slot.is_none(), t.desk_slot, *k));
    }
    by
}

fn summaries(
    w: &OfficeWorld,
    rooms: &HashMap<String, u32>,
    now: DateTime<Utc>,
) -> Vec<OfficeSummaryDto> {
    let mut out: Vec<OfficeSummaryDto> = grouped(w)
        .into_iter()
        .map(|(id, teams)| {
            let rows: Vec<TeamSummaryDto> = teams
                .iter()
                .map(|(k, t)| team_summary(w, k, t, now))
                .collect();
            let (building, floor, room, label) = match &id {
                OfficeId::Team(k) => {
                    let b = building(teams[0].1.product);
                    let room = rooms.get(&k.session).copied();
                    let label = match room {
                        Some(r) => format!("{b}동 {r}호"),
                        None => format!("{b}동"),
                    };
                    (Some(b.to_string()), room.map(|r| r / 100), room, label)
                }
                OfficeId::Vendor(v) => {
                    let f = vendor_floor(*v);
                    (None, Some(f), None, format!("{f}층 {v:?}"))
                }
            };
            let border = if rows.iter().any(|r| r.border == Border::Running) {
                Border::Running
            } else if rows.iter().any(|r| r.border == Border::Failed) {
                Border::Failed
            } else {
                Border::Plain
            };
            OfficeSummaryDto {
                office_id: office_id_str(&id),
                building,
                floor,
                room,
                label,
                phase: rows
                    .iter()
                    .map(|r| r.phase)
                    .min_by_key(|p| phase_rank(*p))
                    .unwrap_or(TeamPhase::Off),
                dot: rows.iter().map(|r| r.dot).min().unwrap_or(Dot::Off),
                asking: rows.iter().any(|r| r.asking),
                border,
                delayed: rows.iter().any(|r| r.delayed),
                teams: rows,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (&a.building, a.floor, a.room, &a.office_id).cmp(&(
            &b.building,
            b.floor,
            b.room,
            &b.office_id,
        ))
    });
    out
}

fn employee_dto(e: &Employee) -> EmployeeDto {
    EmployeeDto {
        agent_id: e.id.clone(),
        name: e.name.clone(),
        rank: e.rank,
        model: e.model.clone(),
        description: e.description.clone(),
        activity: e.activity.clone(),
        recent: e.recent.iter().cloned().collect(),
        counters: e.counters,
        last_say: e.last_say.clone(),
        hired_at: e.hired_at,
        retired_at: e.retired_at,
        seat: e.seat,
        parent: e.parent.clone(),
        depth: e.depth,
        confidence: e.confidence,
        context_used: e.context_used,
        context_window: e.context_window,
    }
}

fn team_dto(k: &TeamKey, t: &Team) -> TeamDto {
    TeamDto {
        team_id: team_id_str(k),
        title: t.title.clone(),
        product: t.product,
        building: building(t.product).to_string(),
        host_badge: host_badge(&k.host),
        folder: folder(t),
        started: t.started,
        last_activity: t.last_activity,
        phase: t.phase,
        desk_slot: t.desk_slot,
        employees: t.employees.iter().map(employee_dto).collect(),
        props: t.props.clone(),
        incident: t.incident.clone(),
    }
}

fn shared(teams: &[(&TeamKey, &Team)]) -> SharedPropsDto {
    let latest = teams
        .iter()
        .map(|(_, t)| *t)
        .max_by_key(|t| t.last_activity);
    SharedPropsDto {
        light: teams
            .iter()
            .map(|(_, t)| *t)
            .filter(|t| t.props.light != Light::Off)
            .max_by_key(|t| t.last_activity)
            .map_or(Light::Off, |t| t.props.light),
        safe_since: teams
            .iter()
            .map(|(_, t)| t.props.safe_since)
            .max()
            .unwrap_or(DateTime::UNIX_EPOCH),
        trash: teams.iter().map(|(_, t)| t.props.trash).sum(),
        pizza: teams.iter().map(|(_, t)| t.props.pizza).max().unwrap_or(0),
        frame_title: latest.map(|t| t.title.clone()).unwrap_or_default(),
        kanban: latest.map(|t| t.props.kanban.clone()).unwrap_or_default(),
        incident: teams.iter().any(|(_, t)| t.incident.is_some()),
        night: teams.iter().any(|(_, t)| t.props.night),
    }
}

/// 사무실 전체 상태. 팀이 하나도 없으면 None
pub fn office_dto(
    w: &OfficeWorld,
    id: &OfficeId,
    rebuilt: bool,
    cues: Vec<Cue>,
) -> Option<OfficeDto> {
    let teams = grouped(w).remove(id)?;
    Some(OfficeDto {
        state_seq: w.state_seq.get(id).copied().unwrap_or(0),
        office_id: office_id_str(id),
        rebuilt,
        teams: teams.iter().map(|(k, t)| team_dto(k, t)).collect(),
        shared: shared(&teams),
        cues,
    })
}

/// 방 번호와 마지막으로 만든 목록. world 잠금 다음에 잡는다
struct Books {
    book: RoomBook,
    /// session_id → 방 번호. 처음 볼 때 한 번 받는다
    // ponytail: 7일 회수를 위한 last_seen 은 처음 볼 때만 갱신한다, 앱을 일주일 넘게 켜 둘 일이 보이면 주기적으로 부른다
    rooms: HashMap<String, u32>,
    last_list: Option<Vec<OfficeSummaryDto>>,
    last_list_seq: u64,
}

/// 커맨드와 발행기가 함께 쓰는 상태 (Tauri `State`)
pub struct Ipc {
    world: Arc<Mutex<OfficeWorld>>,
    books: Mutex<Books>,
}

/// 발행기가 마지막으로 보낸 번호
#[derive(Default)]
pub struct Sent {
    list: Option<u64>,
    state: Option<(OfficeId, u64)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ipc {
    pub fn new(world: Arc<Mutex<OfficeWorld>>, book: RoomBook) -> Self {
        Self {
            world,
            books: Mutex::new(Books {
                book,
                rooms: HashMap::new(),
                last_list: None,
                last_list_seq: 0,
            }),
        }
    }

    /// 새 팀에 방 번호를 준다. rooms.json 쓰기가 있어 world 잠금 밖에서 한다
    fn assign_rooms(&self, now: DateTime<Utc>) {
        let missing: Vec<(String, String)> = {
            let w = lock(&self.world);
            let b = lock(&self.books);
            w.teams
                .keys()
                .filter(|k| !b.rooms.contains_key(&k.session))
                .map(|k| (k.session.clone(), k.host.clone()))
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let mut b = lock(&self.books);
        for (session, host) in missing {
            let a = b.book.room_of(&session, &host_of(&host), now);
            if let Err(e) = a.saved {
                // ponytail: 운영 로그(4.11) 대신 stderr 한 줄, 운영 로그 티켓에서 올린다
                eprintln!("방 번호 저장 실패: {e}");
            }
            b.rooms.insert(session, a.number);
        }
    }

    /// 목록을 만들고, 내용이 바뀌었는데 world 가 번호를 안 올렸으면 올린다.
    /// 점·결재판은 시간으로도 바뀌어서(3초 디바운스 등) world 의 목록 번호만으로는 모자란다
    fn sync_list(w: &mut OfficeWorld, b: &mut Books, now: DateTime<Utc>) -> OfficeListDto {
        let offices = summaries(w, &b.rooms, now);
        if b.last_list.as_ref() != Some(&offices) {
            if w.list_seq == b.last_list_seq {
                w.list_seq += 1;
            }
            b.last_list = Some(offices.clone());
        }
        b.last_list_seq = w.list_seq;
        OfficeListDto {
            list_seq: w.list_seq,
            rebuilt: false,
            scale: w.scale,
            offices,
        }
    }

    /// 시작 때 프론트가 부른다. 스냅샷이라 애니메이션 없이 배치한다
    pub fn snapshot(&self, now: DateTime<Utc>) -> WorldDto {
        self.assign_rooms(now);
        let mut w = lock(&self.world);
        let mut b = lock(&self.books);
        let list = Self::sync_list(&mut w, &mut b, now);
        let selected = w
            .selected
            .clone()
            .and_then(|s| office_dto(&w, &s, true, Vec::new()));
        WorldDto {
            list: OfficeListDto {
                rebuilt: true,
                ..list
            },
            selected,
        }
    }

    /// 선택을 바꾸고 그 사무실 상태를 바로 돌려준다. 상태는 이미 있다 (4.5 선택 전환)
    pub fn select(&self, office_id: &str, request_id: String) -> Result<SelectResponseDto, String> {
        let id =
            parse_office_id(office_id).ok_or_else(|| "사무실 id 형식이 아닙니다".to_string())?;
        let mut w = lock(&self.world);
        if !w.select(Some(id.clone())) {
            return Err("없는 사무실입니다".to_string());
        }
        let office = office_dto(&w, &id, true, Vec::new()).ok_or("없는 사무실입니다")?;
        Ok(SelectResponseDto { office, request_id })
    }

    /// 발행 한 번. 목록 번호가 바뀌었으면 목록, 선택 사무실 번호가 바뀌었거나 연출이 있으면 상태를 낸다.
    /// 연출은 선택 사무실 것만 싣고 나머지는 버린다. 보이지 않는 사무실의 연출은 재생할 곳이 없다
    pub fn poll(
        &self,
        sent: &mut Sent,
        now: DateTime<Utc>,
    ) -> (Option<OfficeListDto>, Option<OfficeDto>) {
        self.assign_rooms(now);
        let mut w = lock(&self.world);
        let mut b = lock(&self.books);
        let outbox = std::mem::take(&mut w.outbox);
        let rebuilt = std::mem::take(&mut w.rebuilt);
        let list_rebuilt = std::mem::take(&mut w.list_rebuilt);
        let list = Self::sync_list(&mut w, &mut b, now);
        let list = (sent.list != Some(list.list_seq)).then(|| {
            sent.list = Some(list.list_seq);
            OfficeListDto {
                rebuilt: list_rebuilt,
                ..list
            }
        });
        let state = w.selected.clone().and_then(|sel| {
            let quiet = rebuilt.contains(&sel);
            // 조용한 재구성에서는 방금 시작된 세션의 출근만 낸다 (4.3)
            let cues: Vec<Cue> = outbox
                .into_iter()
                .filter(|(o, c)| *o == sel && (!quiet || matches!(c, Cue::Enter { .. })))
                .map(|(_, c)| c)
                .collect();
            let seq = w.state_seq.get(&sel).copied().unwrap_or(0);
            let at = Some((sel.clone(), seq));
            if sent.state == at && cues.is_empty() && !quiet {
                return None;
            }
            sent.state = at;
            office_dto(&w, &sel, quiet, cues)
        });
        (list, state)
    }
}

#[tauri::command]
pub fn get_world_snapshot(ipc: State<'_, Arc<Ipc>>) -> WorldDto {
    ipc.snapshot(Utc::now())
}

#[tauri::command]
pub fn select_office(
    ipc: State<'_, Arc<Ipc>>,
    office_id: String,
    request_id: String,
) -> Result<SelectResponseDto, String> {
    ipc.select(&office_id, request_id)
}

/// 200ms 마다 world 를 짧게 잠가 바뀐 것만 낸다. 직렬화와 전송은 잠금 밖이다
pub fn spawn_publisher(app: AppHandle, ipc: Arc<Ipc>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut sent = Sent::default();
        // 디버그 빌드에서만 개수를 찍는다. 내용은 실제 세션이라 찍지 않는다
        #[cfg(debug_assertions)]
        let (mut lists, mut states, mut logged) = (0u64, 0u64, Instant::now());
        loop {
            let start = Instant::now();
            let (list, state) = ipc.poll(&mut sent, Utc::now());
            if let Some(l) = list {
                if let Err(e) = app.emit("office-list", &l) {
                    eprintln!("office-list 발행 실패: {e}");
                }
                #[cfg(debug_assertions)]
                {
                    lists += 1;
                }
            }
            if let Some(s) = state {
                if let Err(e) = app.emit("office-state", &s) {
                    eprintln!("office-state 발행 실패: {e}");
                }
                #[cfg(debug_assertions)]
                {
                    states += 1;
                }
            }
            #[cfg(debug_assertions)]
            if logged.elapsed() >= StdDuration::from_secs(5) {
                println!("발행 누적: office-list {lists}, office-state {states}");
                logged = Instant::now();
            }
            thread::sleep(PUBLISH_EVERY.saturating_sub(start.elapsed()));
        }
    })
}

#[cfg(test)]
mod tests;
