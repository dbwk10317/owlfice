//! 상태 기계 (PLAN 4.7). 좌석·담당·업무 유형·타임라인은 다른 티켓에서 붙인다

pub mod classify;
pub mod names;
pub mod rank;
pub mod stats;
pub mod world;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::discover::Product;
use crate::parse::{Confidence, TodoItem, ToolKind, ToolSummary};

/// 직원 직급 (PLAN 4.7.4). 메인 에이전트가 부장인 것은 부르는 쪽이 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Bujang,
    Gwajang,
    Daeri,
    Sawon,
}

/// 사무실 안 구역 (PLAN 2.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    Sit,
    Type,
    Think,
    Duck,
    Squat,
    Sleep,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// 경과 시간("· 12초째")은 넣지 않는다. 프론트가 `since` 로 붙여야 매초 상태를 다시 보내지 않는다
    pub sentence: String,
    pub room: Room,
    pub since: DateTime<Utc>,
    /// 이 명령의 평균 소요 (진행 막대)
    pub expected_secs: Option<f32>,
    pub pose: Pose,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingCall {
    pub call_id: String,
    pub tool: ToolKind,
    pub summary: ToolSummary,
    pub started: DateTime<Utc>,
}

/// 오늘 실적 (3.5 카드의 여섯 칸)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub read: u32,
    pub edit: u32,
    pub shell: u32,
    pub outside: u32,
    pub meeting: u32,
    pub errors: u32,
}

#[derive(Debug, Clone)]
pub struct Employee {
    pub id: String,
    /// 비어 있으면 world 가 채운다. 이름은 사무실 안에서 겹치지 않아야 해서 팀이 정할 수 없다 (4.7.7)
    pub name: String,
    pub rank: Rank,
    pub model: Option<String>,
    pub description: Option<String>,
    pub activity: Activity,
    /// 최근 행적 3개
    pub recent: VecDeque<Activity>,
    pub counters: Counters,
    pub last_say: Option<String>,
    pub hired_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamPhase {
    Working,
    Dozing,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    Off,
    Yellow,
    Green,
    Red,
}

#[derive(Debug, Clone)]
pub struct TeamProps {
    pub boss_visit: Option<String>,
    /// 화분 성장도 = 부장 컨텍스트 사용률 (0~1). 분갈이 직후 0
    pub plant: Option<f32>,
    pub light: Light,
    pub trash: u32,
    /// 무재해 전광판의 시작 시각
    pub safe_since: DateTime<Utc>,
    pub pizza: u32,
    pub kanban: Vec<TodoItem>,
    pub night: bool,
    pub pr: Option<(u32, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub since: DateTime<Utc>,
    pub last_error: DateTime<Utc>,
}

/// 프론트 연출 힌트 (4.8 AnimCue). 상태는 Employee·TeamProps 가 진실이고 이것은 "어떻게 거기 갔는지"다
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone)]
pub struct Team {
    pub id: String,
    pub product: Product,
    pub title: String,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
    pub started: DateTime<Utc>,
    /// 읽은 이벤트 ts 의 최댓값. mtime 이 아니다 (2.5)
    pub last_activity: DateTime<Utc>,
    pub phase: TeamPhase,
    pub alive_in_registry: bool,
    /// 첫 번째가 부장
    pub employees: Vec<Employee>,
    pub props: TeamProps,
    pub incident: Option<Incident>,
    pub stats: stats::TeamStats,
}

#[cfg(test)]
mod tests;
