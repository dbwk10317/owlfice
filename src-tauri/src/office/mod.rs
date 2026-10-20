//! 상태 기계 (PLAN 4.7). 좌석·담당·업무 유형·타임라인은 다른 티켓에서 붙인다

pub mod classify;
pub mod names;
pub mod rank;
pub mod stats;
pub mod work;
pub mod world;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

use crate::discover::Product;
use crate::parse::{Confidence, TodoItem, ToolKind, ToolSummary};
use work::{Role, Touched, WorkTally, WorkType};

/// 직원 직급 (PLAN 4.7.4). 메인 에이전트가 부장인 것은 부르는 쪽이 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum Rank {
    Bujang,
    Gwajang,
    Daeri,
    Sawon,
}

/// 사무실 안 구역 (PLAN 2.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum Room {
    TeamLead,
    Dev,
    Archive,
    Build,
    Test,
    Review,
    Incident,
    Meeting,
    Shipping,
    Pantry,
    Outside,
    Door,
}

/// 상태가 정하는 자세. 어떤 스프라이트로 그릴지는 프론트가 정한다 (design/HANDOFF.md 5절)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum Pose {
    Sit,
    Type,
    Think,
    Duck,
    Squat,
    Sleep,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// 시간이 흐르며 바뀌는 문장은 경과 대신 `{elapsed}` 자리를 둔다. 프론트가 `since` 로 채워야
    /// 매초 상태를 다시 보내지 않는다 (4.8)
    pub sentence: String,
    pub room: Room,
    pub since: DateTime<Utc>,
    /// 이 명령의 평균 소요 (진행 막대)
    pub expected_secs: Option<f32>,
    pub pose: Pose,
    pub confidence: Confidence,
    /// 함께 도는 호출 수. `+n` 배지와 문장의 "(외 n건)"
    pub extra: u32,
}

/// 마지막 말풍선. 풀에서 문구를 고르는 것은 프론트다 (4.8)
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Bubble {
    /// 그 사건의 시각. 프론트는 4초 안일 때만 그린다
    pub at: DateTime<Utc>,
    pub say: BubbleSay,
}

/// 말풍선 종류와 재료. 이름이 `phrases.ko.json` 의 `bubble` 풀과 짝이다
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "kind")]
pub enum BubbleSay {
    /// 재료 글을 그대로 (검색어, 명령, 허가 대기 질문, 부장 응답)
    Say {
        text: String,
    },
    Mcp {
        server: String,
        tool: String,
    },
    NewVendor {
        server: String,
    },
    NewSkill {
        skill: String,
    },
    Rank,
    Joke,
    Incident,
    AllGreen,
    TooLong,
    ThinkingDuck,
    PlanApproval,
    Scratch,
    Compaction,
    UnknownTool,
    ScriptFallback,
    UnknownResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingCall {
    pub call_id: String,
    pub tool: ToolKind,
    pub summary: ToolSummary,
    pub started: DateTime<Utc>,
}

/// 오늘 실적 (3.5 카드의 여섯 칸)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
pub struct Counters {
    pub read: u32,
    pub edit: u32,
    pub shell: u32,
    pub outside: u32,
    pub meeting: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Employee {
    pub id: String,
    /// 비어 있으면 world 가 채운다. 이름은 사무실 안에서 겹치지 않아야 해서 팀이 정할 수 없다 (4.7.7)
    pub name: String,
    pub rank: Rank,
    /// 만진 파일로 정한 담당 (4.7.5)
    pub role: Role,
    touched: Touched,
    pub model: Option<String>,
    pub description: Option<String>,
    pub activity: Activity,
    pub recent: VecDeque<Activity>,
    pub counters: Counters,
    pub last_say: Option<String>,
    pub hired_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
    /// 개발실·임시 책상 자리 번호. world 가 사무실 단위로 준다 (4.7.9)
    pub seat: Option<u8>,
    /// 결과 안 온 호출 전부. 대표는 마지막
    pub pending: Vec<PendingCall>,
    /// Started 로 끝난 호출 (작업표)
    pub background: Vec<PendingCall>,
    pub quiet_since: Option<DateTime<Utc>>,
    /// 부장만 쓴다
    pub waiting_user_since: Option<DateTime<Utc>>,
    /// 반려 직후 4초 (2.5 우선순위 3번)
    pub rejected_at: Option<DateTime<Utc>>,
    /// 허가 대기 질문. 그 Ask 호출의 결과가 오면 지운다
    pub asking: Option<String>,
    /// 이 직원 파일의 마지막 이벤트 시각
    pub last_event: DateTime<Utc>,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub parent: Option<String>,
    pub depth: u8,
    /// 모르는 agent_id 로 먼저 만든 임시 직원이면 Inferred (4.3)
    pub confidence: Confidence,
    /// 서브 종료 추정용: 마지막 레코드가 tool_use 없는 assistant 였나 (4.7.2)
    pub last_was_text: bool,
    /// 파일별 수정 호출 시각. 반복 수정 문장(3.5, S43)이 쓴다
    // ponytail: "오늘 n번째"는 직원 수명 동안 센다, 실적 칸의 자정 리셋을 정할 때 같이 올린다
    pub edits: HashMap<String, Vec<DateTime<Utc>>>,
    /// 직전 Test 가 실패했으면 (실패한 테스트, 연속 실패 수). 다음 Test·Edit 이 "빨간불 끄는 중"
    pub red: Option<(String, u32)>,
    /// 리뷰실에 들어온 뒤 응답에서 센 `파일:줄` 지적 수 (빨간펜)
    pub review_pens: u32,
    pub bubble: Option<Bubble>,
    /// 지금 활동이 이미 행적에 들어갔나 (끝난 일)
    logged: bool,
    /// 서브가 첫 일을 받아 직급 말풍선을 냈나
    greeted: bool,
    /// 말풍선을 낸 모르는 도구 이름. 같은 도구는 처음 한 번만 (3.6)
    unknown_tools: HashSet<String>,
    /// "왜 이렇게 오래 걸려" 를 이미 낸 호출
    too_long: Option<String>,
}

/// 선언 순서가 활발한 순서다. 사무실 단계는 팀 중 최솟값
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
pub enum TeamPhase {
    Working,
    Dozing,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum Light {
    Off,
    Yellow,
    Green,
    Red,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TeamProps {
    pub boss_visit: Option<String>,
    /// 화분 성장도 = 부장 컨텍스트 사용률 (0~1). 분갈이 직후 0
    pub plant: Option<f32>,
    pub light: Light,
    /// 신호등을 마지막으로 바꾼 빌드·테스트 시각. 공유 신호등은 이것이 가장 늦은 팀 색이다 (4.7.9)
    pub light_at: DateTime<Utc>,
    pub trash: u32,
    /// 무재해 전광판의 시작 시각
    pub safe_since: DateTime<Utc>,
    pub pizza: u32,
    pub kanban: Vec<TodoItem>,
    pub night: bool,
    pub pr: Option<(u32, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Incident {
    pub since: DateTime<Utc>,
    pub last_error: DateTime<Utc>,
}

/// 프론트 연출 힌트 (4.8 AnimCue). 상태는 Employee·TeamProps 가 진실이고 이것은 "어떻게 거기 갔는지"다
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "kind")]
#[ts(rename = "AnimCue")]
pub enum Cue {
    BossVisit {
        text: String,
    },
    Clap {
        agent: String,
    },
    Enter {
        agent: String,
    },
    Rehire {
        agent: String,
    },
    Retire {
        agent: String,
    },
    Stamp {
        passed: u32,
        failed: u32,
        estimated: bool,
    },
    Truck,
    Return,
    Fireworks,
    Reject {
        agent: String,
    },
    Shake {
        agent: String,
    },
    Siren {
        on: bool,
    },
    FrameSwap {
        title: String,
    },
    Repot,
    Paper {
        agent: String,
        on: bool,
    },
}

/// 세션 하나 (4.7). 사무실 합성·좌석은 world 몫이다
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    pub id: String,
    pub product: Product,
    pub title: String,
    /// 오늘의 업무 (4.7.6)
    pub work_type: WorkType,
    work: WorkTally,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
    pub started: DateTime<Utc>,
    /// 읽은 이벤트 ts 의 최댓값. mtime 이 아니다 (2.5)
    pub last_activity: DateTime<Utc>,
    pub phase: TeamPhase,
    pub alive_in_registry: bool,
    /// 팀장석 번호. 0~3 팀장석, 4~5 간이 책상, None 은 카드만. world 가 입장 때 준다 (4.7.9)
    pub desk_slot: Option<u8>,
    /// 첫 번째가 부장
    pub employees: Vec<Employee>,
    pub props: TeamProps,
    pub incident: Option<Incident>,
    pub stats: stats::TeamStats,
    /// 파일별 직전 ts. ts 없는 레코드가 다른 파일 시각을 쓰지 않게 (4.3 "순서")
    file_ts: HashMap<PathBuf, DateTime<Utc>>,
    /// 노랑으로 켜기 전 색. 확정 결과 없이 끝나면 이 색으로 돌아간다
    light_before: Light,
    light_at_before: DateTime<Utc>,
    /// 마지막으로 퇴근에서 깨어난 때와 그때의 피자 수 (4.7.2 피자)
    awake_since: DateTime<Utc>,
    pizza_at_wake: u32,
    /// 처음 본 거래처·스킬 말풍선용 (3.6)
    seen_vendors: HashSet<String>,
    seen_skills: HashSet<String>,
}

#[cfg(test)]
mod tests;
