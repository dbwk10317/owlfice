//! IPC 계약 (docs/CONTRACTS.md C9). 프론트 타입은 여기서 ts-rs 로 만든다 (`src/types/ipc.ts`)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::detail;
use crate::diag::{self, Diag};
use crate::discover::{CandidateResult, Host, HostKind, Product, Vendor};
use crate::office::names::RoomBook;
use crate::office::work::{Role, WorkType};
use crate::office::world::{OfficeId, OfficeWorld, Scale, TeamKey};
use crate::office::{
    Activity, Bubble, Counters, Cue, Employee, Incident, Light, Rank, Room, Team, TeamPhase,
    TeamProps, TestCounts, TestVerdict, TimelineKind, Tokens,
};
use crate::parse::{Confidence, TodoItem, ToolKind, ToolOutcome};
use crate::runtime::Runtime;
use crate::settings::{ManualPath, SettingsBook};

/// 최대 5회/초 (docs/CONTRACTS.md C9)
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
    /// 백엔드의 현재 선택. 팀이 나가거나 규모가 바뀌면 백엔드가 스스로 바꾼다
    pub selected_id: Option<String>,
    /// 그 선택을 만든 마지막 `select_office` 요청 번호. store 는 자기 마지막 요청 이상일 때만 선택을 따른다
    pub select_request: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum NightMode {
    Auto,
    On,
    Off,
}

/// `get_settings` 반환, `set_settings` 인자. 칸이 비어 있으면(null) 그 설정은 건드리지 않는다.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// 아직 안 골랐으면 null (첫 실행, 손상된 설정 파일)
    pub scale: Option<Scale>,
    /// 저장된 회사 이름. 없으면 null 이고 프론트가 기본 이름을 쓴다. 보낼 때 ""(공백뿐)는 기본으로 되돌린다
    pub company_name: Option<String>,
    // 자동 탐색에 추가한다. 빈 배열은 저장된 수동 경로를 모두 지운다.
    pub manual_paths: Option<Vec<ManualPath>>,
    pub sound: Option<bool>,
    pub night_mode: Option<NightMode>,
    pub os_notifications: Option<bool>,
    // 비면 프론트가 기본 호칭을 쓴다. 빈 문자열을 보내면 기본으로 되돌린다.
    pub boss_name: Option<String>,
    pub raw_diagnostics: Option<bool>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPathDto {
    pub host: String,
    pub vendor: String,
    pub path: Option<String>,
    pub status: DiscoveryStatus,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
pub enum DiscoveryStatus {
    Found,
    Missing,
    Offline,
    Unknown,
}

/// `get_world_snapshot` 반환. 목록 필드를 그대로 품어 store 가 목록처럼 받는다
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorldDto {
    #[serde(flatten)]
    pub list: OfficeListDto,
    pub selected: Option<OfficeDto>,
}

/// 사이드바 한 줄 (docs/CONTRACTS.md C3). 대기업은 팀 하나, 중소기업은 층 하나
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
    /// 팀 중 가장 급한 점 (docs/CONTRACTS.md C3 중소기업 층 점)
    pub dot: Dot,
    /// 허가 대기는 점이 아니라 결재판 아이콘이라(docs/CONTRACTS.md C3 범례) 점 우선순위에 넣지 않고 따로 둔다
    pub asking: bool,
    pub border: Border,
    pub delayed: bool,
    pub teams: Vec<TeamSummaryDto>,
}

/// 팀 한 줄 (docs/CONTRACTS.md C3). 대기업에서는 사무실 줄의 둘째 줄 재료
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
    pub last_activity: DateTime<Utc>,
    pub read_failed: bool,
    pub phase: TeamPhase,
    pub dot: Dot,
    /// 점 옆 결재판 (허가 대기)
    pub asking: bool,
    pub asking_count: u32,
    pub border: Border,
    /// 통신 지연 (점선 테두리, WSL 의심 상태)
    pub delayed: bool,
    /// 호스트 배지 (`WSL`). Windows 는 null
    pub host_badge: Option<String>,
    /// 오늘의 업무 (docs/CONTRACTS.md C3 사무실 한 줄)
    pub work_type: WorkType,
}

/// 상태 점 (docs/CONTRACTS.md C3 범례). 선언 순서가 급한 순서다 (장애 > 근무 > 결재·응답 대기 > 졸음 > 대기 중 > 퇴근). 사무실 점은 팀 중 최솟값
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
pub enum Dot {
    Incident,
    Working,
    /// 초록 깜빡임: 결재 대기 또는 응답 대기
    Waiting,
    Dozing,
    /// 노랑 + "대기 중" 칩: 30분 넘게 조용하지만 프로세스가 살아 있다 (S58)
    IdleAlive,
    Off,
}

/// 줄 테두리 (docs/CONTRACTS.md C3 범례). 선언 순서가 약한 순서다. 사무실 테두리는 팀 중 최댓값
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
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
    pub room_recent: Vec<RoomRecentDto>,
    pub cues: Vec<CueDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RoomRecentDto {
    pub room: Room,
    pub events: Vec<RoomEventDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RoomEventDto {
    pub team_id: String,
    pub agent_id: String,
    pub activity: Activity,
}

/// 연출 신호와 그 팀. 중소기업은 한 사무실에 팀이 여럿이라 팀 단위 연출(사장님 방문, 분갈이 등)에 필요하다
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CueDto {
    pub team_id: String,
    #[serde(flatten)]
    pub cue: Cue,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SelectResponseDto {
    #[serde(flatten)]
    pub office: OfficeDto,
    pub request_id: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TeamDto {
    pub team_id: String,
    pub title: String,
    /// 동 글자 (A~F) = 제품
    pub building: String,
    pub host_badge: Option<String>,
    /// 통신 지연 구름 (S46)
    pub delayed: bool,
    pub folder: String,
    pub started: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub phase: TeamPhase,
    pub work_type: WorkType,
    /// 0~3 팀장석, 4~5 간이 책상, null 은 카드만 (docs/CONTRACTS.md C8)
    pub desk_slot: Option<u8>,
    /// 첫 번째가 부장
    pub employees: Vec<EmployeeDto>,
    pub props: TeamProps,
    pub incident: Option<Incident>,
    /// 30분 넘게 조용하지만 프로세스가 살아 있다 (S58). 살아 있는 팀이 모두 그러면 조명이 어둡다
    pub idle_alive: bool,
    pub tests_today: TestCounts,
    pub commits_today: u32,
}

/// 직원 카드 한 장 (docs/CONTRACTS.md C3)
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EmployeeDto {
    pub agent_id: String,
    pub name: String,
    pub rank: Rank,
    /// 담당 (docs/CONTRACTS.md C3 카드)
    pub role: Role,
    pub model: Option<String>,
    pub description: Option<String>,
    pub activity: Activity,
    /// 최근 행적, 최신이 먼저
    pub recent: Vec<Activity>,
    pub counters: Counters,
    pub unique_reads: u32,
    pub pending_count: u32,
    pub unique_edits: u32,
    pub planning: bool,
    pub medal: bool,
    pub build_test_border: Border,
    pub last_say: Option<String>,
    pub hired_at: DateTime<Utc>,
    /// 있으면 퇴장한(또는 퇴장으로 짐작한) 직원
    pub retired_at: Option<DateTime<Utc>>,
    /// 있으면 퇴근한 서브 (퇴근 묶음, 근태판 퇴근 칸)
    pub departed_at: Option<DateTime<Utc>>,
    pub seat: Option<u8>,
    /// 상사 agent_id. 깊이 2 이상 카드의 "상사" 줄
    pub parent: Option<String>,
    pub depth: u8,
    pub confidence: Confidence,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    /// 마지막 말풍선
    pub bubble: Option<Bubble>,
    /// 백그라운드로 도는 명령 수 (작업표)
    pub jobs: u32,
    /// 미팅실에 있을 때만: 대표 진행 중 호출이 무엇인가
    pub meeting: Option<Meeting>,
    /// 리뷰실에 있을 때만
    pub review: Option<Review>,
}

/// 미팅실 소품 재료 (docs/CONTRACTS.md C3 거래처 미팅실). MCP 는 화면 상호·명함, 스킬은 매뉴얼
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind")]
pub enum Meeting {
    Mcp { server: String },
    Skill,
}

/// 리뷰실 소품 재료 (docs/CONTRACTS.md C3 리뷰실)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// 빨간 포스트잇 수 (`파일:줄` 지적)
    pub pens: u32,
    /// 승인 문구가 나왔다 (초록 도장)
    pub approved: bool,
}

/// 사무실 공유 소품 (docs/CONTRACTS.md C8 합성 규칙). 대기업은 팀 하나라 그 팀 값과 같다
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedPropsDto {
    /// 가장 최근 빌드·테스트 결과 = 신호등을 가장 늦게 바꾼 팀의 색
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
    /// 테스트실 결과판 = 테스트를 가장 늦게 끝낸 팀 결과
    pub test: Option<TestVerdict>,
    pub stats: BoardStatsDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BoardStatsDto {
    pub commits: u32,
    pub files_edited: u32,
    pub commands: u32,
    pub turns: u32,
}

/// 타임라인 커서 (docs/CONTRACTS.md C9): 원본 파일 세대와 그 줄의 시작 바이트. 페이지 마지막 칸의 값을 그대로 돌려준다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CursorDto {
    pub generation: u64,
    pub byte_offset: u64,
}

/// 긴 글의 앞뒤만. 잘렸으면 `tail` 이 끝부분이고, 안 잘렸으면 `head` 가 전문이고 `tail` 은 빈 글이다
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipDto {
    pub head: String,
    pub tail: String,
    /// 원문 바이트 수
    pub bytes: u64,
}

/// 타임라인 한 칸 (docs/CONTRACTS.md C9). 원본 줄을 다시 읽어 만든다
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EventDto {
    /// 다음 페이지를 열 때 `before` 로 보낸다
    pub cursor: CursorDto,
    pub agent_id: String,
    pub ts: DateTime<Utc>,
    pub kind: TimelineKind,
    /// 한 줄 요약
    pub summary: String,
    /// 결과를 본문 정규식으로 짐작했으면 Inferred
    pub confidence: Confidence,
    /// 원본의 도구 이름
    pub tool: Option<String>,
    /// 호출과 그 결과가 같은 값을 가진다
    pub call_id: Option<String>,
    /// 도구 입력 원문 (앞뒤 2KB)
    pub input: Option<ClipDto>,
    pub outcome: Option<ToolOutcome>,
    /// 결과·메시지·지시 원문 (앞뒤 2KB)
    pub body: Option<ClipDto>,
    /// 원본 jsonl 과 줄 번호 (1 기준)
    pub path: String,
    pub line_no: u64,
    /// 그 줄의 이벤트 중 몇 번째인가. 같은 줄의 칸을 `OpenTarget` 이 가른다
    pub line_idx: u32,
    /// 큰 결과가 분리된 `tool-results/` 파일. 세션 폴더 안인 것만
    pub sidecar: Option<String>,
    /// 결과 본문에서 센 테스트 합격·반려 수
    pub test: Option<TestDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TestDto {
    pub passed: u32,
    pub failed: u32,
}

/// 실적 요약 한 줄: 처음 본 순서의 항목과 나온 횟수
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CountDto {
    pub item: String,
    pub times: u32,
}

/// 직원 상세 팝업 (docs/CONTRACTS.md C9) 중 원본을 다시 읽지 않는 부분
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EmployeeDetailDto {
    pub agent_id: String,
    pub name: String,
    pub rank: Rank,
    pub role: Role,
    pub model: Option<String>,
    pub hired_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
    /// 서브만. 상사 이름
    pub boss_name: Option<String>,
    /// 서브는 받은 프롬프트 전문, 부장은 첫 사장님 메시지
    pub instruction: Option<String>,
    pub files_read: Vec<String>,
    pub files_edited: Vec<CountDto>,
    pub commands: Vec<CountDto>,
}

/// 직급별 직원 수. 퇴장한 직원도 센다
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HeadcountDto {
    pub bujang: u32,
    pub gwajang: u32,
    pub daeri: u32,
    pub sawon: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PrDto {
    pub number: u32,
    pub url: String,
}

/// 세션 상세 팝업 (docs/CONTRACTS.md C9)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetailDto {
    pub session_id: String,
    pub title: String,
    /// 동 글자 (A~F)
    pub building: String,
    pub host_label: String,
    pub work_type: WorkType,
    /// 작업 폴더 전체 경로 (기록에 적힌 그대로)
    pub cwd: String,
    pub git_branch: Option<String>,
    /// 부장 모델
    pub model: Option<String>,
    pub started: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub turns: u32,
    pub tokens: Option<Tokens>,
    pub headcount: HeadcountDto,
    pub pr: Option<PrDto>,
    /// 최근 사건, 최신부터
    pub recent: Vec<EventDto>,
    /// 원본 줄을 못 읽어 `recent` 가 비었으면 이유. 나머지 값은 메모리에서 온 것이라 그대로 있다
    pub recent_error: Option<DetailError>,
}

/// `open_path` 대상. 경로는 백엔드가 아는 것만 연다. 프론트가 경로를 넘기는 길은 두지 않는다
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum OpenTarget {
    /// 세션 원본 파일이 있는 폴더 (그 파일을 골라 둔다)
    SourceFolder {
        office_id: String,
        team_id: String,
    },
    WorkFolder {
        office_id: String,
        team_id: String,
    },
    /// 그 칸의 원본 jsonl (탐색기는 줄로 못 간다. 줄 번호는 EventDto 가 준다).
    /// `line_idx` 는 EventDto 의 것이다
    SourceLine {
        office_id: String,
        team_id: String,
        agent_id: String,
        cursor: CursorDto,
        line_idx: u32,
    },
    Sidecar {
        office_id: String,
        team_id: String,
        agent_id: String,
        cursor: CursorDto,
        line_idx: u32,
    },
    /// 그 팀의 PR 주소를 기본 브라우저로
    Pr {
        office_id: String,
        team_id: String,
    },
}

/// 상세 커맨드의 에러 코드
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DetailError {
    /// 없는 사무실·팀·직원·칸, 형식이 틀린 id
    NotFound,
    /// Reset 으로 세대가 바뀌었다. 첫 페이지부터 다시 연다
    CursorInvalid,
    /// WSL 배포판이 꺼졌거나 통신 지연 중이라 파일에 손대지 않았다 (docs/CONTRACTS.md C5)
    HostOffline,
    /// 읽기가 상한 시간을 넘겼다
    Timeout,
    /// 열 경로가 없거나 열 수 없는 모양이다
    OpenFailed,
}

/// 대기업 `team/<vendor>/<session>/<host>`, 중소기업 `vendor/<vendor>`.
/// 호스트 id 에 `:` 가 있고(`wsl:Ubuntu`) 세션 id 는 파일 이름이라 `/` 가 없어서 호스트를 끝에 둔다
pub fn office_id_str(id: &OfficeId) -> String {
    match id {
        OfficeId::Team(k) => team_id_str(k),
        OfficeId::Vendor(v) => format!("vendor/{}", vendor_str(*v)),
    }
}

/// 팀 id. 방 번호 키로도 쓴다 (같은 세션 id 가 다른 호스트·제조사에서 와도 번호를 나누지 않게)
pub(crate) fn team_id_str(k: &TeamKey) -> String {
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

/// 중소기업 층 (docs/CONTRACTS.md C2)
fn vendor_floor(v: Vendor) -> u32 {
    match v {
        Vendor::Claude => 1,
        Vendor::Codex => 2,
        Vendor::Gemini => 3,
    }
}

/// 동 (docs/CONTRACTS.md C2)
pub(crate) fn building(p: Product) -> &'static str {
    match p {
        Product::ClaudeDesktop => "A",
        Product::ClaudeCli => "B",
        Product::CodexApp => "C",
        Product::CodexCli => "D",
        Product::AntigravityApp => "E",
        Product::AntigravityCli => "F",
    }
}

fn host_badge(w: &OfficeWorld, host: &str) -> Option<String> {
    w.hosts
        .get(host)
        .filter(|h| matches!(h.kind, HostKind::Wsl { .. }))
        .map(|_| "WSL".to_string())
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
        TeamPhase::Dozing if t.idle_alive => Dot::IdleAlive,
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

fn border(light: Light) -> Border {
    match light {
        Light::Red => Border::Failed,
        Light::Yellow => Border::Running,
        Light::Off | Light::Green => Border::Plain,
    }
}

fn team_summary(w: &OfficeWorld, k: &TeamKey, t: &Team, now: DateTime<Utc>) -> TeamSummaryDto {
    let raw_asking_count = t
        .employees
        .iter()
        .filter(|e| e.retired_at.is_none() && t.asking(e, now))
        .count() as u32;
    TeamSummaryDto {
        team_id: team_id_str(k),
        boss_name: t.employees[0].name.clone(),
        title: t.title.clone(),
        folder: folder(t),
        others: active_subs(t).count() as u32,
        started: t.started,
        last_activity: t.last_activity,
        read_failed: w.read_failed_teams.contains(k),
        phase: t.phase,
        dot: team_dot(t, now),
        asking: raw_asking_count > 0,
        asking_count: if t.phase == TeamPhase::Off {
            0
        } else {
            raw_asking_count
        },
        border: border(t.props.light),
        delayed: w.suspect_hosts.contains(&k.host),
        host_badge: host_badge(w, &k.host),
        work_type: t.work_type,
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
                    let room = rooms.get(&team_id_str(k)).copied();
                    let label = match room {
                        Some(r) => format!("{b}동 {r}호"),
                        None => format!("{b}동"),
                    };
                    (Some(b.to_string()), room.map(|r| r / 100), room, label)
                }
                OfficeId::Vendor(v) => {
                    let f = vendor_floor(*v);
                    (None, Some(f), None, format!("{f}층 {v:?} 사무실"))
                }
            };
            OfficeSummaryDto {
                office_id: office_id_str(&id),
                building,
                floor,
                room,
                label,
                phase: rows.iter().map(|r| r.phase).min().unwrap_or(TeamPhase::Off),
                dot: rows.iter().map(|r| r.dot).min().unwrap_or(Dot::Off),
                asking: rows.iter().any(|r| r.asking),
                border: rows.iter().map(|r| r.border).max().unwrap_or(Border::Plain),
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

fn employee_dto(e: &Employee, now: DateTime<Utc>) -> EmployeeDto {
    EmployeeDto {
        agent_id: e.id.clone(),
        name: e.name.clone(),
        rank: e.rank,
        role: e.role,
        model: e.model.clone(),
        description: e.description.clone(),
        activity: e.activity.clone(),
        recent: e.recent.iter().cloned().collect(),
        counters: e.counters,
        unique_reads: u32::try_from(e.record.files_read.items.len()).unwrap_or(u32::MAX),
        pending_count: u32::try_from(e.pending.len()).unwrap_or(u32::MAX),
        unique_edits: u32::try_from(e.record.files_edited.items.len()).unwrap_or(u32::MAX),
        planning: e.planning,
        medal: e.medal_until.is_some_and(|until| now < until),
        build_test_border: match e.build_test_light() {
            Light::Yellow => Border::Running,
            Light::Red => Border::Failed,
            Light::Off | Light::Green => Border::Plain,
        },
        last_say: e.last_say.clone(),
        hired_at: e.hired_at,
        retired_at: e.retired_at,
        departed_at: e.departed_at,
        seat: e.seat,
        parent: e.parent.clone(),
        depth: e.depth,
        confidence: e.confidence,
        context_used: e.context_used,
        context_window: e.context_window,
        bubble: e.bubble.clone(),
        jobs: u32::try_from(e.background.len()).unwrap_or(u32::MAX),
        meeting: e
            .pending
            .last()
            .filter(|_| e.activity.room == Room::Meeting)
            .and_then(|c| match &c.tool {
                ToolKind::Mcp { server, .. } => Some(Meeting::Mcp {
                    server: server.clone(),
                }),
                // 리뷰 스킬은 리뷰실로 가서 미팅실에 있는 스킬은 리뷰가 아니다
                ToolKind::Skill { .. } => Some(Meeting::Skill),
                _ => None,
            }),
        review: (e.activity.room == Room::Review).then_some(Review {
            pens: e.review_pens,
            approved: e.approved,
        }),
    }
}

fn team_dto(w: &OfficeWorld, k: &TeamKey, t: &Team, now: DateTime<Utc>) -> TeamDto {
    TeamDto {
        team_id: team_id_str(k),
        title: t.title.clone(),
        building: building(t.product).to_string(),
        host_badge: host_badge(w, &k.host),
        delayed: w.suspect_hosts.contains(&k.host),
        folder: folder(t),
        started: t.started,
        last_activity: t.last_activity,
        phase: t.phase,
        work_type: t.work_type,
        desk_slot: t.desk_slot,
        employees: t.employees.iter().map(|e| employee_dto(e, now)).collect(),
        props: t.props.clone(),
        incident: t.incident.clone(),
        idle_alive: t.idle_alive,
        tests_today: t.tests_today(),
        commits_today: t.commits_today(),
    }
}

fn shared(teams: &[(&TeamKey, &Team)]) -> SharedPropsDto {
    let latest = teams
        .iter()
        .map(|(_, t)| *t)
        .max_by_key(|t| t.last_activity);
    let mut stats = BoardStatsDto::default();
    for (_, t) in teams {
        stats.commits = stats.commits.saturating_add(t.commits);
        stats.turns = stats.turns.saturating_add(t.turns);
        let mut files = HashSet::new();
        for e in &t.employees {
            files.extend(e.record.files_edited.items.iter().map(|(path, _)| path));
            for (_, count) in &e.record.commands.items {
                stats.commands = stats.commands.saturating_add(*count);
            }
        }
        stats.files_edited = stats
            .files_edited
            .saturating_add(u32::try_from(files.len()).unwrap_or(u32::MAX));
    }
    SharedPropsDto {
        light: teams
            .iter()
            .map(|(_, t)| &t.props)
            .filter(|p| p.light != Light::Off)
            .max_by_key(|p| p.light_at)
            .map_or(Light::Off, |p| p.light),
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
        test: teams
            .iter()
            .map(|(_, t)| &t.props)
            .filter(|p| p.test.is_some())
            .max_by_key(|p| p.test_at)
            .and_then(|p| p.test),
        stats,
    }
}

/// 사무실 전체 상태. 팀이 하나도 없으면 None
pub fn office_dto(
    w: &OfficeWorld,
    id: &OfficeId,
    rebuilt: bool,
    cues: Vec<CueDto>,
    now: DateTime<Utc>,
) -> Option<OfficeDto> {
    let teams = grouped(w).remove(id)?;
    let mut room_recent: BTreeMap<Room, Vec<RoomEventDto>> = BTreeMap::new();
    for (key, team) in &teams {
        let team_id = team_id_str(key);
        for e in &team.employees {
            for activity in &e.room_recent {
                room_recent
                    .entry(activity.room)
                    .or_default()
                    .push(RoomEventDto {
                        team_id: team_id.clone(),
                        agent_id: e.id.clone(),
                        activity: activity.clone(),
                    });
            }
        }
    }
    let room_recent = room_recent
        .into_iter()
        .map(|(room, mut events)| {
            events.sort_by(|a, b| {
                b.activity
                    .since
                    .cmp(&a.activity.since)
                    .then_with(|| a.team_id.cmp(&b.team_id))
                    .then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            events.truncate(10);
            RoomRecentDto { room, events }
        })
        .collect();
    Some(OfficeDto {
        state_seq: w.state_seq.get(id).copied().unwrap_or(0),
        office_id: office_id_str(id),
        rebuilt,
        teams: teams.iter().map(|(k, t)| team_dto(w, k, t, now)).collect(),
        shared: shared(&teams),
        room_recent,
        cues,
    })
}

/// 목록에서 번호 말고 바뀌었는지 볼 부분
type ListBody = (Vec<OfficeSummaryDto>, Option<String>, u64);

/// 방 번호, 마지막 목록, 마지막 선택 요청. world 잠금 다음에 잡거나 혼자 잡는다. 잡은 채로 파일을 쓰지 않는다
struct Books {
    book: RoomBook,
    /// 팀 id → 방 번호. 처음 볼 때 한 번 받는다
    // ponytail: 7일 회수를 위한 last_seen 은 처음 볼 때만 갱신한다, 앱을 일주일 넘게 켜 둘 일이 보이면 주기적으로 부른다
    rooms: HashMap<String, u32>,
    last_list: Option<ListBody>,
    last_list_seq: u64,
    /// 마지막으로 처리한 select_office 요청 번호
    last_select: u64,
}

/// 커맨드와 발행기가 함께 쓰는 상태 (Tauri `State`)
pub struct Ipc {
    world: Arc<Mutex<OfficeWorld>>,
    books: Mutex<Books>,
    /// 설정 적용을 한 번에 하나씩 보낸다. 파일 쓰기를 world 잠금 밖에 두면서 파일과 world 의 반영 순서를 지킨다
    settings_gate: Mutex<()>,
}

/// 발행기가 마지막으로 보낸 번호
#[derive(Default)]
pub struct Sent {
    list: Option<u64>,
    state: Option<(OfficeId, u64)>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ipc {
    pub fn new(world: Arc<Mutex<OfficeWorld>>, book: RoomBook) -> Self {
        Self {
            world,
            settings_gate: Mutex::new(()),
            books: Mutex::new(Books {
                book,
                rooms: HashMap::new(),
                last_list: None,
                last_list_seq: 0,
                last_select: 0,
            }),
        }
    }

    /// 새 팀에 방 번호를 준다. world 를 잡고 팀을 모은 뒤 놓고, books 안에서는 메모리 배정만,
    /// rooms.json 쓰기는 두 잠금 밖에서 한다. 쓰는 동안 스냅샷·발행이 books 를 기다리지 않게
    fn assign_rooms(&self, now: DateTime<Utc>) {
        let teams: Vec<(String, Host)> = {
            let w = lock(&self.world);
            w.teams
                .keys()
                .filter_map(|k| Some((team_id_str(k), w.hosts.get(&k.host)?.clone())))
                .collect()
        };
        let save = {
            let mut b = lock(&self.books);
            for (id, host) in teams {
                if b.rooms.contains_key(&id) {
                    continue;
                }
                let (number, reused) = b.book.room_of_unsaved(&id, &host, now);
                if reused {
                    crate::oplog!("방 번호 재사용: {}층이 99팀을 넘었다", number / 100);
                }
                b.rooms.insert(id, number);
            }
            b.book.unsaved()
        };
        if let Some(e) = save.and_then(|s| s.write_report()) {
            crate::oplog!("방 번호 저장 실패: {e}");
        }
    }

    /// 목록을 만들고, 내용이 바뀌었는데 world 가 번호를 안 올렸으면 올린다.
    /// 점·결재판은 시간으로도 바뀌고(3초 디바운스 등) 선택은 커맨드가 바꿔서 world 의 목록 번호만으로는 모자란다
    fn sync_list(w: &mut OfficeWorld, b: &mut Books, now: DateTime<Utc>) -> OfficeListDto {
        let body: ListBody = (
            summaries(w, &b.rooms, now),
            w.selected.as_ref().map(office_id_str),
            b.last_select,
        );
        if b.last_list.as_ref() != Some(&body) {
            if w.list_seq == b.last_list_seq {
                w.list_seq += 1;
            }
            b.last_list = Some(body.clone());
        }
        b.last_list_seq = w.list_seq;
        let (offices, selected_id, select_request) = body;
        OfficeListDto {
            list_seq: w.list_seq,
            rebuilt: false,
            scale: w.scale,
            offices,
            selected_id,
            select_request,
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
            .and_then(|s| office_dto(&w, &s, true, Vec::new(), now));
        WorldDto {
            list: OfficeListDto {
                rebuilt: true,
                ..list
            },
            selected,
        }
    }

    /// 선택을 바꾸고 그 사무실 상태를 바로 돌려준다. 상태는 이미 있다 (docs/CONTRACTS.md C6 선택 전환).
    /// 두 요청이 뒤바뀌어 오면 늦은 옛 요청을 무시한다. 안 그러면 백엔드 선택이 store 와 어긋난다
    pub fn select(&self, office_id: &str, request_id: u64) -> Result<SelectResponseDto, String> {
        let id =
            parse_office_id(office_id).ok_or_else(|| "사무실 id 형식이 아닙니다".to_string())?;
        let mut w = lock(&self.world);
        let mut b = lock(&self.books);
        if request_id < b.last_select {
            return Err("더 새 선택 요청이 이미 처리됐습니다".to_string());
        }
        if !w.select(Some(id.clone())) {
            return Err("없는 사무실입니다".to_string());
        }
        b.last_select = request_id;
        let office =
            office_dto(&w, &id, true, Vec::new(), Utc::now()).ok_or("없는 사무실입니다")?;
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
            // 조용한 재구성에서는 방금 시작된 세션의 출근만 낸다 (docs/CONTRACTS.md C4)
            let cues: Vec<CueDto> = outbox
                .into_iter()
                .filter(|(o, _, c)| *o == sel && (!quiet || matches!(c, Cue::Enter { .. })))
                .map(|(_, k, cue)| CueDto {
                    team_id: team_id_str(&k),
                    cue,
                })
                .collect();
            let seq = w.state_seq.get(&sel).copied().unwrap_or(0);
            let at = Some((sel.clone(), seq));
            if sent.state == at && cues.is_empty() && !quiet {
                return None;
            }
            sent.state = at;
            office_dto(&w, &sel, quiet, cues, now)
        });
        (list, state)
    }
}

#[tauri::command]
pub fn get_world_snapshot(ipc: State<'_, Arc<Ipc>>) -> WorldDto {
    ipc.snapshot(Utc::now())
}

/// 저장이 실패해도 이번 실행의 설정은 바뀐다. 에러는 저장 실패를 알린다.
/// 규모가 바뀌면 world 가 재배치하고(docs/CONTRACTS.md C8) 발행기가 rebuilt 로 낸다
pub fn apply_settings(
    ipc: &Ipc,
    book: &SettingsBook,
    diag: &Mutex<Diag>,
    runtime: &Runtime,
    dto: SettingsDto,
) -> Result<(), String> {
    if let Some(paths) = &dto.manual_paths {
        for path in paths {
            path.validate()?;
        }
    }
    // 입력 오류 때는 world도 바꾸지 않는다. 디스크 쓰기는 world 잠금 밖(C4)이고, 동시 요청의 순서는 gate 가 지킨다.
    let _gate = lock(&ipc.settings_gate);
    let old_paths = dto
        .manual_paths
        .as_ref()
        .map(|_| book.snapshot().manual_paths.unwrap_or_default());
    let saved = book.update(&dto);
    if let Some(raw) = dto.raw_diagnostics {
        lock(diag).set_raw(raw);
    }
    if let (Some(old), Some(new)) = (old_paths, &dto.manual_paths) {
        runtime.manual_changed(&old, new);
    }
    if let Some(scale) = dto.scale {
        lock(&ipc.world).set_scale(scale);
    }
    saved.map_err(|e| format!("설정을 저장하지 못했습니다: {e}"))
}

/// `get_settings` 의 몸통. 테스트가 State 없이 부른다
pub fn settings_dto(book: &SettingsBook) -> SettingsDto {
    book.snapshot()
}

#[tauri::command]
pub fn get_settings(settings: State<'_, Arc<SettingsBook>>) -> SettingsDto {
    settings_dto(&settings)
}

#[tauri::command]
pub fn set_settings(
    ipc: State<'_, Arc<Ipc>>,
    settings: State<'_, Arc<SettingsBook>>,
    diag: State<'_, Arc<Mutex<Diag>>>,
    runtime: State<'_, Arc<Runtime>>,
    dto: SettingsDto,
) -> Result<(), String> {
    apply_settings(&ipc, &settings, &diag, &runtime, dto)
}

#[tauri::command]
pub fn rescan(runtime: State<'_, Arc<Runtime>>) {
    runtime.request_rescan();
}

#[tauri::command]
pub fn get_discovery(runtime: State<'_, Arc<Runtime>>) -> Vec<DiscoveryPathDto> {
    lock(&runtime.candidates)
        .iter()
        .map(|candidate| DiscoveryPathDto {
            host: candidate.host.label.clone(),
            vendor: candidate.vendor.label().into(),
            path: candidate
                .root
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            status: match candidate.result {
                CandidateResult::Found => DiscoveryStatus::Found,
                CandidateResult::Missing => DiscoveryStatus::Missing,
                CandidateResult::Offline => DiscoveryStatus::Offline,
                CandidateResult::Unknown => DiscoveryStatus::Unknown,
            },
        })
        .collect()
}

#[tauri::command]
pub async fn get_diagnostics(
    ipc: State<'_, Arc<Ipc>>,
    runtime: State<'_, Arc<Runtime>>,
) -> Result<String, String> {
    let ipc = ipc.inner().clone();
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let data = runtime.diagnostics(StdDuration::from_secs(2));
        // spawn_blocking 안이라 기다려도 된다. 잠금 경합으로 익명화 정보가 빠지면 안 된다
        let projects: Vec<_> = lock(&ipc.world)
            .teams
            .values()
            .map(|team| team.cwd.clone())
            .collect();
        diag::diagnostic_text(&data, runtime.home.as_deref(), &projects)
    })
    .await
    .map_err(|e| format!("진단 정보를 모으지 못했습니다: {e}"))
}

#[tauri::command]
pub fn open_log_folder(runtime: State<'_, Arc<Runtime>>) -> Result<(), String> {
    detail::open_log_folder(&runtime.log_dir)
        .map_err(|e| format!("로그 폴더를 열지 못했습니다: {e:?}"))
}

#[tauri::command]
pub fn select_office(
    ipc: State<'_, Arc<Ipc>>,
    office_id: String,
    request_id: u64,
) -> Result<SelectResponseDto, String> {
    ipc.select(&office_id, request_id)
}

/// 200ms 마다 world 를 짧게 잠가 바뀐 것만 낸다. 직렬화와 전송은 잠금 밖이다
pub fn spawn_publisher(app: AppHandle, ipc: Arc<Ipc>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut sent = Sent::default();
        let mut notifications = crate::notification::PendingNotifications::default();
        loop {
            let start = Instant::now();
            let now = Utc::now();
            let (list, state) = ipc.poll(&mut sent, now);
            let enabled = app.state::<Arc<SettingsBook>>().os_notifications();
            let hidden = app
                .state::<crate::notification::WindowVisibility>()
                .hidden();
            let notices = notifications.poll(&lock(&ipc.world).teams, now, hidden, enabled);
            for notice in notices {
                crate::notification::show(&app, &notice);
            }
            if let Some(l) = list
                && let Err(e) = app.emit("office-list", &l)
            {
                crate::oplog_once!("office-list 발행 실패: {e}");
            }
            if let Some(s) = state
                && let Err(e) = app.emit("office-state", &s)
            {
                crate::oplog_once!("office-state 발행 실패: {e}");
            }
            thread::sleep(PUBLISH_EVERY.saturating_sub(start.elapsed()));
        }
    })
}

/// 파일을 읽는 상세 일은 tokio worker 가 아니라 막아도 되는 스레드에서 한다 (2초 상한 대기, world 잠금 대기)
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, DetailError> + Send + 'static,
) -> Result<T, DetailError> {
    // ponytail: 스레드가 죽은 것도 Timeout 으로 알린다, 에러 종류를 늘릴 때 나눈다
    tauri::async_runtime::spawn_blocking(f)
        .await
        .unwrap_or(Err(DetailError::Timeout))
}

#[tauri::command]
pub async fn get_employee_timeline(
    ipc: State<'_, Arc<Ipc>>,
    office_id: String,
    team_id: String,
    agent_id: String,
    before: Option<CursorDto>,
    limit: Option<u32>,
) -> Result<Vec<EventDto>, DetailError> {
    let ipc = ipc.inner().clone();
    blocking(move || {
        detail::employee_timeline(&ipc.world, &office_id, &team_id, &agent_id, before, limit)
    })
    .await
}

#[tauri::command]
pub async fn get_employee_detail(
    ipc: State<'_, Arc<Ipc>>,
    office_id: String,
    team_id: String,
    agent_id: String,
) -> Result<EmployeeDetailDto, DetailError> {
    let ipc = ipc.inner().clone();
    blocking(move || detail::employee_detail(&ipc.world, &office_id, &team_id, &agent_id)).await
}

#[tauri::command]
pub async fn get_session_detail(
    ipc: State<'_, Arc<Ipc>>,
    office_id: String,
    team_id: String,
    limit: Option<u32>,
) -> Result<SessionDetailDto, DetailError> {
    let ipc = ipc.inner().clone();
    blocking(move || detail::session_detail(&ipc.world, &office_id, &team_id, limit)).await
}

#[tauri::command]
pub async fn open_path(ipc: State<'_, Arc<Ipc>>, target: OpenTarget) -> Result<(), DetailError> {
    let ipc = ipc.inner().clone();
    blocking(move || detail::open_path(&ipc.world, target)).await
}

#[cfg(test)]
mod tests;
