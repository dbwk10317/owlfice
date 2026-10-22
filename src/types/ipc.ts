// ipc.rs 에서 ts-rs 로 만든 파일이다. 손으로 고치지 않는다. 다시 만드는 명령은 docs/BUILD.md

/**
 * `get_world_snapshot` 반환. 목록 필드를 그대로 품어 store 가 목록처럼 받는다
 */
export type WorldDto = { selected: OfficeDto | null, listSeq: number, rebuilt: boolean, 
/**
 * 사이드바 구조(동별 목록, 층 고정 행)가 규모로 갈린다
 */
scale: Scale, offices: Array<OfficeSummaryDto>, 
/**
 * 백엔드의 현재 선택. 팀이 나가거나 규모가 바뀌면 백엔드가 스스로 바꾼다
 */
selectedId: string | null, 
/**
 * 그 선택을 만든 마지막 `select_office` 요청 번호. store 는 자기 마지막 요청 이상일 때만 선택을 따른다
 */
selectRequest: number, };

/**
 * `office-list` 페이로드
 */
export type OfficeListDto = { listSeq: number, rebuilt: boolean, 
/**
 * 사이드바 구조(동별 목록, 층 고정 행)가 규모로 갈린다
 */
scale: Scale, offices: Array<OfficeSummaryDto>, 
/**
 * 백엔드의 현재 선택. 팀이 나가거나 규모가 바뀌면 백엔드가 스스로 바꾼다
 */
selectedId: string | null, 
/**
 * 그 선택을 만든 마지막 `select_office` 요청 번호. store 는 자기 마지막 요청 이상일 때만 선택을 따른다
 */
selectRequest: number, };

/**
 * 사이드바 한 줄 (docs/CONTRACTS.md C3). 대기업은 팀 하나, 중소기업은 층 하나
 */
export type OfficeSummaryDto = { officeId: string, 
/**
 * 대기업만: 동 글자 (A~F)
 */
building: string | null, floor: number | null, 
/**
 * 대기업만. 아직 번호를 못 받았으면 null
 */
room: number | null, label: string, 
/**
 * 팀 중 가장 활발한 단계
 */
phase: TeamPhase, 
/**
 * 팀 중 가장 급한 점 (docs/CONTRACTS.md C3 중소기업 층 점)
 */
dot: Dot, 
/**
 * 허가 대기는 점이 아니라 결재판 아이콘이라(docs/CONTRACTS.md C3 범례) 점 우선순위에 넣지 않고 따로 둔다
 */
asking: boolean, border: Border, delayed: boolean, teams: Array<TeamSummaryDto>, };

/**
 * 팀 한 줄 (docs/CONTRACTS.md C3). 대기업에서는 사무실 줄의 둘째 줄 재료
 */
export type TeamSummaryDto = { teamId: string, bossName: string, title: string, 
/**
 * 작업 폴더 이름만. 전체 경로는 세션 상세가 준다
 */
folder: string, 
/**
 * "부장 외 n명": 퇴장하지 않은 서브 수
 */
others: number, 
/**
 * 활성 경과는 프론트가 이것으로 센다. 매초 다시 보내지 않으려고
 */
started: string, lastActivity: string, readFailed: boolean, phase: TeamPhase, dot: Dot, 
/**
 * 점 옆 결재판 (허가 대기)
 */
asking: boolean, askingCount: number, border: Border, 
/**
 * 통신 지연 (점선 테두리, WSL 의심 상태)
 */
delayed: boolean, 
/**
 * 호스트 배지 (`WSL`). Windows 는 null
 */
hostBadge: string | null, 
/**
 * 오늘의 업무 (docs/CONTRACTS.md C3 사무실 한 줄)
 */
workType: WorkType, };

/**
 * 상태 점 (docs/CONTRACTS.md C3 범례). 선언 순서가 급한 순서다 (장애 > 근무 > 결재·응답 대기 > 졸음 > 대기 중 > 퇴근). 사무실 점은 팀 중 최솟값
 */
export type Dot = "Incident" | "Working" | "Waiting" | "Dozing" | "IdleAlive" | "Off";

/**
 * 줄 테두리 (docs/CONTRACTS.md C3 범례). 선언 순서가 약한 순서다. 사무실 테두리는 팀 중 최댓값
 */
export type Border = "Plain" | "Failed" | "Running";

/**
 * `office-state` 페이로드. 스냅샷의 `selected`, `select_office` 응답도 같은 꼴이다
 */
export type OfficeDto = { stateSeq: number, officeId: string, rebuilt: boolean, teams: Array<TeamDto>, shared: SharedPropsDto, roomRecent: Array<RoomRecentDto>, cues: Array<CueDto>, };

export type RoomRecentDto = { room: Room, events: Array<RoomEventDto>, };

export type RoomEventDto = { teamId: string, agentId: string, activity: Activity, };

/**
 * 연출 신호와 그 팀. 중소기업은 한 사무실에 팀이 여럿이라 팀 단위 연출(사장님 방문, 분갈이 등)에 필요하다
 */
export type CueDto = { teamId: string, } & ({ "kind": "BossVisit", text: string, } | { "kind": "Clap", agent: string, } | { "kind": "Enter", agent: string, } | { "kind": "Rehire", agent: string, } | { "kind": "Retire", agent: string, } | { "kind": "Depart", agent: string, } | { "kind": "Stamp", agent: string, at: string, passed: number, failed: number, estimated: boolean, confirmedPass: boolean, } | { "kind": "Truck" } | { "kind": "Return" } | { "kind": "Fireworks" } | { "kind": "Reject", agent: string, } | { "kind": "Shake", agent: string, } | { "kind": "Siren", on: boolean, } | { "kind": "FrameSwap", title: string, } | { "kind": "Repot" } | { "kind": "Compaction", agent: string, at: string, } | { "kind": "Paper", agent: string, on: boolean, });

export type SelectResponseDto = { requestId: number, stateSeq: number, officeId: string, rebuilt: boolean, teams: Array<TeamDto>, shared: SharedPropsDto, roomRecent: Array<RoomRecentDto>, cues: Array<CueDto>, };

export type TeamDto = { teamId: string, title: string, 
/**
 * 동 글자 (A~F) = 제품
 */
building: string, hostBadge: string | null, 
/**
 * 통신 지연 구름 (S46)
 */
delayed: boolean, folder: string, started: string, lastActivity: string, phase: TeamPhase, workType: WorkType, 
/**
 * 0~3 팀장석, 4~5 간이 책상, null 은 카드만 (docs/CONTRACTS.md C8)
 */
deskSlot: number | null, 
/**
 * 첫 번째가 부장
 */
employees: Array<EmployeeDto>, props: TeamProps, incident: Incident | null, 
/**
 * 30분 넘게 조용하지만 프로세스가 살아 있다 (S58). 살아 있는 팀이 모두 그러면 조명이 어둡다
 */
idleAlive: boolean, testsToday: TestCounts, commitsToday: number, };

/**
 * 직원 카드 한 장 (docs/CONTRACTS.md C3)
 */
export type EmployeeDto = { agentId: string, name: string, rank: Rank, 
/**
 * 담당 (docs/CONTRACTS.md C3 카드)
 */
role: Role, model: string | null, description: string | null, activity: Activity, 
/**
 * 최근 행적, 최신이 먼저
 */
recent: Array<Activity>, counters: Counters, uniqueReads: number, uniqueEdits: number, planning: boolean, medal: boolean, lastSay: string | null, hiredAt: string, 
/**
 * 있으면 퇴장한(또는 퇴장으로 짐작한) 직원
 */
retiredAt: string | null, 
/**
 * 있으면 퇴근한 서브 (퇴근 묶음, 근태판 퇴근 칸)
 */
departedAt: string | null, seat: number | null, 
/**
 * 상사 agent_id. 깊이 2 이상 카드의 "상사" 줄
 */
parent: string | null, depth: number, confidence: Confidence, contextUsed: number | null, contextWindow: number | null, 
/**
 * 마지막 말풍선
 */
bubble: Bubble | null, 
/**
 * 백그라운드로 도는 명령 수 (작업표)
 */
jobs: number, 
/**
 * 미팅실에 있을 때만: 대표 진행 중 호출이 무엇인가
 */
meeting: Meeting | null, 
/**
 * 리뷰실에 있을 때만
 */
review: Review | null, };

/**
 * 미팅실 소품 재료 (docs/CONTRACTS.md C3 거래처 미팅실). MCP 는 화면 상호·명함, 스킬은 매뉴얼
 */
export type Meeting = { "kind": "Mcp", server: string, } | { "kind": "Skill" };

/**
 * 리뷰실 소품 재료 (docs/CONTRACTS.md C3 리뷰실)
 */
export type Review = { 
/**
 * 빨간 포스트잇 수 (`파일:줄` 지적)
 */
pens: number, 
/**
 * 승인 문구가 나왔다 (초록 도장)
 */
approved: boolean, };

/**
 * 사무실 공유 소품 (docs/CONTRACTS.md C8 합성 규칙). 대기업은 팀 하나라 그 팀 값과 같다
 */
export type SharedPropsDto = { 
/**
 * 가장 최근 빌드·테스트 결과 = 신호등을 가장 늦게 바꾼 팀의 색
 */
light: Light, 
/**
 * 무재해 = 팀 중 최솟값이라 가장 늦은 시작 시각
 */
safeSince: string, trash: number, pizza: number, 
/**
 * 사규 액자와 칸반은 가장 최근 활동 팀 것
 */
frameTitle: string, kanban: Array<TodoItem>, 
/**
 * 장애 상황실 = 어느 팀이든 장애
 */
incident: boolean, night: boolean, 
/**
 * 테스트실 결과판 = 테스트를 가장 늦게 끝낸 팀 결과
 */
test: TestVerdict | null, stats: BoardStatsDto, };

export type BoardStatsDto = { commits: number, filesEdited: number, commands: number, turns: number, };

export type Activity = { 
/**
 * 시간이 흐르며 바뀌는 문장은 경과 대신 `{elapsed}` 자리를 둔다. 프론트가 `since` 로 채워야
 * 매초 상태를 다시 보내지 않는다 (docs/CONTRACTS.md C9)
 */
sentence: string, room: Room, since: string, 
/**
 * 이 명령의 평균 소요 (진행 막대)
 */
expectedSecs: number | null, pose: Pose, confidence: Confidence, 
/**
 * 함께 도는 호출 수. `+n` 배지와 문장의 "(외 n건)"
 */
extra: number, 
/**
 * 문장에 `{elapsed}` 자리가 있나. 에이전트가 쓴 글 속의 같은 글자를 프론트가 채우지 않게,
 * 문장 모양이 아니라 우리가 틀로 둔 자리인지로 정한다 (docs/CONTRACTS.md C9)
 */
elapsed: boolean, };

/**
 * 마지막 말풍선. 풀에서 문구를 고르는 것은 프론트다 (docs/CONTRACTS.md C9)
 */
export type Bubble = { 
/**
 * 그 사건의 시각. 프론트는 4초 안일 때만 그린다
 */
at: string, say: BubbleSay, };

/**
 * 말풍선 종류와 재료. 이름이 `phrases.ko.json` 의 `bubble` 풀과 짝이다
 */
export type BubbleSay = { "kind": "Say", text: string, } | { "kind": "Mcp", server: string, tool: string, } | { "kind": "NewVendor", server: string, } | { "kind": "NewSkill", skill: string, } | { "kind": "Rank" } | { "kind": "Incident" } | { "kind": "AllGreen" } | { "kind": "TooLong" } | { "kind": "ThinkingDuck" } | { "kind": "PlanApproval" } | { "kind": "Scratch" } | { "kind": "Compaction" } | { "kind": "UnknownTool" } | { "kind": "ScriptFallback" } | { "kind": "UnknownResult" };

/**
 * 오늘 실적 (docs/CONTRACTS.md C3 카드의 여섯 칸)
 */
export type Counters = { read: number, edit: number, shell: number, outside: number, meeting: number, errors: number, };

export type TeamProps = { bossVisit: string | null, 
/**
 * 화분 성장도 = 부장 컨텍스트 사용률 (0~1). 분갈이 직후 0
 */
plant: number | null, light: Light, 
/**
 * 신호등을 마지막으로 바꾼 빌드·테스트 시각. 공유 신호등은 이것이 가장 늦은 팀 색이다 (docs/CONTRACTS.md C8)
 */
lightAt: string, 
/**
 * 테스트 결과를 아직 못 봤으면 null
 */
test: TestVerdict | null, 
/**
 * 결과판을 마지막으로 바꾼 테스트 시각. 공유 결과판은 이것이 가장 늦은 팀 것이다 (docs/CONTRACTS.md C8)
 */
testAt: string, trash: number, 
/**
 * 무재해 전광판의 시작 시각
 */
safeSince: string, pizza: number, kanban: Array<TodoItem>, night: boolean, pr: [number, string] | null, };

/**
 * 마지막 테스트 결과 (테스트실 결과판)
 */
export type TestVerdict = "Pass" | "Fail";

export type TestCounts = { passed: number, failed: number, };

export type Incident = { since: string, lastError: string, };

/**
 * 프론트 연출 힌트 (docs/CONTRACTS.md C9 AnimCue). 상태는 Employee·TeamProps 가 진실이고 이것은 "어떻게 거기 갔는지"다
 */
export type AnimCue = { "kind": "BossVisit", text: string, } | { "kind": "Clap", agent: string, } | { "kind": "Enter", agent: string, } | { "kind": "Rehire", agent: string, } | { "kind": "Retire", agent: string, } | { "kind": "Depart", agent: string, } | { "kind": "Stamp", agent: string, at: string, passed: number, failed: number, estimated: boolean, confirmedPass: boolean, } | { "kind": "Truck" } | { "kind": "Return" } | { "kind": "Fireworks" } | { "kind": "Reject", agent: string, } | { "kind": "Shake", agent: string, } | { "kind": "Siren", on: boolean, } | { "kind": "FrameSwap", title: string, } | { "kind": "Repot" } | { "kind": "Compaction", agent: string, at: string, } | { "kind": "Paper", agent: string, on: boolean, };

/**
 * 직원 직급 (docs/CONTRACTS.md C8). 메인 에이전트가 부장인 것은 부르는 쪽이 정한다.
 */
export type Rank = "Bujang" | "Gwajang" | "Daeri" | "Sawon";

/**
 * 담당. 선언 순서가 동률일 때의 우선순위다
 */
export type Role = "Backend" | "Frontend" | "Qa" | "DevOps" | "Writer" | "Trainee";

/**
 * 오늘의 업무
 */
export type WorkType = "CodeReview" | "BugFix" | "Refactor" | "Docs" | "Research" | "NewDev";

/**
 * 사무실 안 구역 (docs/CONTRACTS.md C2)
 */
export type Room = "TeamLead" | "Dev" | "Archive" | "Build" | "Test" | "Review" | "Incident" | "Meeting" | "Shipping" | "Pantry" | "Outside" | "Door";

/**
 * 상태가 정하는 자세. 어떤 스프라이트로 그릴지는 프론트가 정한다 (design/HANDOFF.md 5절)
 */
export type Pose = "Sit" | "Type" | "Think" | "Duck" | "Read" | "Squat" | "Sleep" | "Pace";

/**
 * 선언 순서가 활발한 순서다. 사무실 단계는 팀 중 최솟값
 */
export type TeamPhase = "Working" | "Dozing" | "Off";

export type Light = "Off" | "Yellow" | "Green" | "Red";

export type Scale = "Large" | "Small";

/**
 * `get_settings` 반환, `set_settings` 인자. 칸이 비어 있으면(null) 그 설정은 건드리지 않는다.
 */
export type SettingsDto = { 
/**
 * 아직 안 골랐으면 null (첫 실행, 손상된 설정 파일)
 */
scale: Scale | null, 
/**
 * 저장된 회사 이름. 없으면 null 이고 프론트가 기본 이름을 쓴다. 보낼 때 ""(공백뿐)는 기본으로 되돌린다
 */
companyName: string | null, manualPaths: Array<ManualPath> | null, sound: boolean | null, osNotifications: boolean | null, bossName: string | null, rawDiagnostics: boolean | null, };

export type DiscoveryPathDto = { host: string, vendor: string, path: string | null, status: DiscoveryStatus, };

export type DiscoveryStatus = "Found" | "Missing" | "Offline" | "Unknown";

export type ManualPath = { vendor: ManualVendor, root: string, };

export type ManualVendor = "Claude" | "Codex";

export type StepState = "Running" | "Done" | "Warn" | "Error";

/**
 * 문장 틀은 `phrases.ko.json` 의 loading 이 갖고 프론트가 이 재료로 채운다
 */
export type StartupSummary = { "kind": "None" } | { "kind": "Candidates", n: number, hosts: Array<string>, } | { "kind": "Rented", buildings: Array<string>, } | { "kind": "NoFolders" } | { "kind": "WslPending" } | { "kind": "Teams", n: number, } | { "kind": "Hired", n: number, bujang: number, gwajang: number, daeri: number, sawon: number, } | { "kind": "WslOff", floor: number, distro: string, } | { "kind": "WslFail", floor: number, distro: string, } | { "kind": "SettingsBroken" };

/**
 * 오래 걸리는 단계의 진짜 원인 (2초 넘으면 화면에 나온다)
 */
export type StartupDetail = { "kind": "Files", done: number, total: number, } | { "kind": "Wsl", distro: string, };

/**
 * `startup-progress` 페이로드
 */
export type StartupProgressDto = { 
/**
 * 1..7. 0단계(규모 선택)는 프론트 몫이다
 */
step: number, state: StepState, summary: StartupSummary, detail: StartupDetail | null, };

/**
 * 확인됨은 레코드가 직접 말한 것, 추정은 본문 정규식 등으로 짐작한 것 (docs/CONTRACTS.md C2)
 */
export type Confidence = "Confirmed" | "Inferred";

export type TodoItem = { content: string, status: TodoStatus, };

export type TodoStatus = "Pending" | "InProgress" | "Done";

/**
 * 타임라인 커서 (docs/CONTRACTS.md C9): 원본 파일 세대와 그 줄의 시작 바이트. 페이지 마지막 칸의 값을 그대로 돌려준다
 */
export type CursorDto = { generation: number, byteOffset: number, };

/**
 * 긴 글의 앞뒤만. 잘렸으면 `tail` 이 끝부분이고, 안 잘렸으면 `head` 가 전문이고 `tail` 은 빈 글이다
 */
export type ClipDto = { head: string, tail: string, 
/**
 * 원문 바이트 수
 */
bytes: number, };

/**
 * 타임라인 한 칸 (docs/CONTRACTS.md C9). 원본 줄을 다시 읽어 만든다
 */
export type EventDto = { 
/**
 * 다음 페이지를 열 때 `before` 로 보낸다
 */
cursor: CursorDto, agentId: string, ts: string, kind: TimelineKind, 
/**
 * 한 줄 요약
 */
summary: string, 
/**
 * 결과를 본문 정규식으로 짐작했으면 Inferred
 */
confidence: Confidence, 
/**
 * 원본의 도구 이름
 */
tool: string | null, 
/**
 * 호출과 그 결과가 같은 값을 가진다
 */
callId: string | null, 
/**
 * 도구 입력 원문 (앞뒤 2KB)
 */
input: ClipDto | null, outcome: ToolOutcome | null, 
/**
 * 결과·메시지·지시 원문 (앞뒤 2KB)
 */
body: ClipDto | null, 
/**
 * 원본 jsonl 과 줄 번호 (1 기준)
 */
path: string, lineNo: number, 
/**
 * 그 줄의 이벤트 중 몇 번째인가. 같은 줄의 칸을 `OpenTarget` 이 가른다
 */
lineIdx: number, 
/**
 * 큰 결과가 분리된 `tool-results/` 파일. 세션 폴더 안인 것만
 */
sidecar: string | null, 
/**
 * 결과 본문에서 센 테스트 합격·반려 수
 */
test: TestDto | null, };

export type TestDto = { passed: number, failed: number, };

export type ToolOutcome = "Ok" | "Err" | "Rejected" | "Started" | "Unknown";

/**
 * 팀의 토큰 사용량 합 (docs/CONTRACTS.md C9 세션 상세). 입력은 새로 보낸 몫(캐시 읽기 제외)이다
 */
export type Tokens = { input: number, output: number, };

/**
 * 직원 타임라인(docs/CONTRACTS.md C9)에 남기는 사건 종류. 생각·토큰·메타·칸반은 다른 칸(호출 입력 등)과 겹치거나 팝업에 쓸 게 없다
 */
export type TimelineKind = "UserMessage" | "AssistantText" | "ToolCall" | "ToolResult" | "SubagentSpawn" | "SubagentDone" | "PermissionRequest" | "ContextCompaction" | "TitleChanged" | "PrLinked";

/**
 * 직원 상세 팝업 (docs/CONTRACTS.md C9) 중 원본을 다시 읽지 않는 부분
 */
export type EmployeeDetailDto = { agentId: string, name: string, rank: Rank, role: Role, model: string | null, hiredAt: string, retiredAt: string | null, 
/**
 * 서브만. 상사 이름
 */
bossName: string | null, 
/**
 * 서브는 받은 프롬프트 전문, 부장은 첫 사장님 메시지
 */
instruction: string | null, filesRead: Array<string>, filesEdited: Array<CountDto>, commands: Array<CountDto>, };

/**
 * 실적 요약 한 줄: 처음 본 순서의 항목과 나온 횟수
 */
export type CountDto = { item: string, times: number, };

/**
 * 직급별 직원 수. 퇴장한 직원도 센다
 */
export type HeadcountDto = { bujang: number, gwajang: number, daeri: number, sawon: number, };

export type PrDto = { number: number, url: string, };

/**
 * 세션 상세 팝업 (docs/CONTRACTS.md C9)
 */
export type SessionDetailDto = { sessionId: string, title: string, 
/**
 * 동 글자 (A~F)
 */
building: string, hostLabel: string, workType: WorkType, 
/**
 * 작업 폴더 전체 경로 (기록에 적힌 그대로)
 */
cwd: string, gitBranch: string | null, 
/**
 * 부장 모델
 */
model: string | null, started: string, lastActivity: string, turns: number, tokens: Tokens | null, headcount: HeadcountDto, pr: PrDto | null, 
/**
 * 최근 사건, 최신부터
 */
recent: Array<EventDto>, 
/**
 * 원본 줄을 못 읽어 `recent` 가 비었으면 이유. 나머지 값은 메모리에서 온 것이라 그대로 있다
 */
recentError: DetailError | null, };

/**
 * `open_path` 대상. 경로는 백엔드가 아는 것만 연다. 프론트가 경로를 넘기는 길은 두지 않는다
 */
export type OpenTarget = { "kind": "SourceFolder", officeId: string, teamId: string, } | { "kind": "WorkFolder", officeId: string, teamId: string, } | { "kind": "SourceLine", officeId: string, teamId: string, agentId: string, cursor: CursorDto, lineIdx: number, } | { "kind": "Sidecar", officeId: string, teamId: string, agentId: string, cursor: CursorDto, lineIdx: number, } | { "kind": "Pr", officeId: string, teamId: string, };

/**
 * 상세 커맨드의 에러 코드
 */
export type DetailError = "not_found" | "cursor_invalid" | "host_offline" | "timeout" | "open_failed";
