use super::{REPEAT_EDIT_WINDOW, REPEAT_EDITS, is_review};
use crate::office::{Employee, PendingCall, Rank};
use crate::parse::{Confidence, TestResult, ToolKind};

pub const QUOTE: usize = 30;
pub const LINE: usize = 40;

pub const ARRIVAL_BOSS: &str = "출근 중";
pub const ARRIVAL: &str = "출근 중 · 자리 찾는 중";
pub const REQUEST: &str = "요구사항 접수";
pub const ASKING: &str = "결재판 들고 문 앞 대기";
pub const PLANNING: &str = "회의실에서 계획을 세우는 중";
/// ExitPlanMode 의 질문 자리 (design/HANDOFF.md 5.1절 허가 대기)
pub const PLAN: &str = "계획 결재 올립니다";
pub const INCIDENT: &str = "장애 상황실 소집, 무재해 기록 0분";
pub const INCIDENT_OVER: &str = "장애 해제";
pub const THINK: &str = "턱 괴고 고민 중인 듯 ({elapsed})";
pub const DUCK: &str = "러버덕과 상담 중인 듯 ({elapsed})";
pub const SLEEP: &str = "졸고 있는 듯";
pub const WAITING_USER: &str = "요구사항 대기 중인 듯 · 사장님 자리 비움 {elapsed}";
pub const DOZING: &str = "졸음";
pub const REPOT: &str = "분갈이 중 (서류 정리)";
pub const REPOT_SAY: &str = "분갈이";

/// 내용 속 백틱은 닮은 글자(ˋ)로 바꿔 코드 조각의 짝이 뒤집히지 않게 한다
fn code(s: &str) -> String {
    format!("`{}`", s.replace('`', "\u{02CB}"))
}

/// 첫 줄을 `n` 자로. 뒤에 더 있으면 "…"
// ponytail: char 단위라 ZWJ 이모지·NFD 자모가 경계에서 갈릴 수 있다, 신고가 오면 grapheme 단위로
pub fn cut(text: &str, n: usize) -> String {
    let trimmed = text.trim();
    let line = trimmed.lines().next().unwrap_or_default();
    let head: String = line.chars().take(n).collect();
    let head = head.trim_end();
    if head.len() < trimmed.len() {
        format!("{head}…")
    } else {
        head.to_string()
    }
}

fn rank_name(r: Rank) -> &'static str {
    match r {
        Rank::Bujang => "부장",
        Rank::Gwajang => "과장",
        Rank::Daeri => "대리",
        Rank::Sawon => "사원",
    }
}

/// 입력이 없을 때 쓰는 도구 이름
pub fn label(tool: &ToolKind) -> String {
    match tool {
        ToolKind::FileRead => "열람",
        ToolKind::FileSearch => "검색",
        ToolKind::FileWrite => "새 서류",
        ToolKind::FileEdit => "수정",
        ToolKind::Shell => "셸 명령",
        ToolKind::Script => "스크립트",
        ToolKind::Build => "빌드",
        ToolKind::Test => "테스트",
        ToolKind::GitQuery => "git 조회",
        ToolKind::GitDiff => "git diff",
        ToolKind::GitCommit => "커밋",
        ToolKind::GitPush => "push",
        ToolKind::PrView => "PR 조회",
        ToolKind::PrCreate => "PR 생성",
        ToolKind::Release => "릴리스",
        ToolKind::Spawn => "업무 지시",
        ToolKind::Ask | ToolKind::ExitPlanMode => "결재",
        ToolKind::EnterPlanMode => "계획 시작",
        ToolKind::WebSearch => "웹 검색",
        ToolKind::WebFetch => "웹 열람",
        ToolKind::Todo => "칸반 정리",
        ToolKind::Mcp { server, tool } => return format!("{server} {tool}"),
        ToolKind::Skill { name } | ToolKind::Other(name) => return name.clone(),
    }
    .to_string()
}

/// 반려·결과 미확인·완료 문장의 대상: 명령, 경로, 패턴 순. 없으면 도구 이름
pub fn subject(call: &PendingCall) -> String {
    let s = &call.summary;
    s.command
        .as_deref()
        .or(s.path.as_deref())
        .or(s.pattern.as_deref())
        .map_or_else(|| label(&call.tool), code)
}

fn command(call: &PendingCall, fallback: &str) -> String {
    call.summary
        .command
        .as_deref()
        .map_or_else(|| fallback.to_string(), code)
}

fn path(call: &PendingCall) -> String {
    call.summary
        .path
        .as_deref()
        .map_or_else(|| "서류".to_string(), code)
}

/// 평균을 알면 " (평소 n초)". 이 호출 동안 바뀌지 않아 고정 글자로 붙인다
fn usual(expected: Option<f32>) -> String {
    let Some(secs) = expected.map(|s| s.round() as u64) else {
        return String::new();
    };
    match (secs / 60, secs % 60) {
        (0, s) => format!(" (평소 {s}초)"),
        (m, 0) => format!(" (평소 {m}분)"),
        (m, s) => format!(" (평소 {m}분 {s}초)"),
    }
}

fn red(r: &(String, u32)) -> String {
    format!("빨간불 끄는 중: {} {}번째 시도", code(&r.0), r.1 + 1)
}

pub fn report(text: &str) -> String {
    format!("보고서 제출함: \"{}\"", cut(text, QUOTE))
}
pub fn review(pens: u32) -> String {
    format!("PR 리뷰 중 · 빨간펜 {pens}개")
}

/// 진행 중 호출 (대표 작업). `extra` 는 함께 도는 호출 수, `expected` 는 이 명령의 평균 소요
pub fn call(call: &PendingCall, extra: usize, e: &Employee, expected: Option<f32>) -> String {
    let s = &call.summary;
    let base = match &call.tool {
        ToolKind::FileRead => {
            let mut t = format!("{} 열람 중", path(call));
            if let Some((a, b)) = s.range {
                t += &format!(" ({a}~{b}행)");
            }
            t
        }
        ToolKind::FileSearch => match (&s.pattern, &s.path) {
            (Some(p), Some(dir)) => format!("'{}' 전사 수색 중 ({})", code(p), code(dir)),
            (Some(p), None) => format!("'{}' 전사 수색 중", code(p)),
            (None, _) => format!("{} 전사 수색 중", command(call, "서류철")),
        },
        ToolKind::FileWrite => format!("{} 새 서류 작성 중", path(call)),
        ToolKind::FileEdit => match (&e.red, &s.path) {
            (Some(r), _) => red(r),
            (None, Some(p)) => {
                let times = e.edits.get(p).map_or(&[][..], Vec::as_slice);
                let recent = times
                    .iter()
                    .filter(|&&t| t <= call.started && call.started - t <= REPEAT_EDIT_WINDOW)
                    .count();
                if recent >= REPEAT_EDITS {
                    format!("{} 리팩터링 중 (오늘 {}번째 수정)", code(p), times.len())
                } else if let Some(n) = s.lines_changed {
                    format!("{} 수정 중 ({n}줄 교체)", code(p))
                } else {
                    format!("{} 수정 중", code(p))
                }
            }
            (None, None) => "서류 수정 중".to_string(),
        },
        ToolKind::Test => match &e.red {
            Some(r) => red(r),
            None => format!(
                "{} 돌리는 중 · {{elapsed}}째{}",
                command(call, "테스트"),
                usual(expected)
            ),
        },
        ToolKind::Build => "빌드 서버 돌리는 중, 선풍기 가동".to_string(),
        ToolKind::Script => "스크립트 돌리는 중 · {elapsed}째".to_string(),
        ToolKind::Shell
        | ToolKind::GitQuery
        | ToolKind::GitPush
        | ToolKind::PrCreate
        | ToolKind::Release => format!("{} 실행 중 · {{elapsed}}째", command(call, "셸 명령")),
        ToolKind::GitDiff | ToolKind::PrView => review(e.review_pens),
        ToolKind::Skill { .. } if is_review(&call.tool) => review(e.review_pens),
        ToolKind::Skill { name } => format!("외주 매뉴얼 {} 펼쳐보는 중", code(name)),
        ToolKind::Mcp { server, tool } => {
            format!("거래처 {}와 {} 건으로 미팅 중", code(server), code(tool))
        }
        ToolKind::WebSearch | ToolKind::WebFetch => match &s.pattern {
            Some(p) => format!("'{}' 건으로 외근 중", code(p)),
            None => "외근 중".to_string(),
        },
        ToolKind::GitCommit => match &s.commit_message {
            Some(m) => format!("출고 포장 중: \"{}\"", cut(m, LINE)),
            None => format!("출고 포장 중: {}", command(call, "커밋")),
        },
        ToolKind::Spawn => "업무 지시 중".to_string(),
        ToolKind::Ask | ToolKind::ExitPlanMode => ASKING.to_string(),
        ToolKind::EnterPlanMode => "계획 시작 중".to_string(),
        ToolKind::Todo => "칸반 보드 정리 중".to_string(),
        ToolKind::Other(name) => format!("처음 보는 업무: {}", code(name)),
    };
    more(base, extra)
}

/// 함께 도는 호출이 있으면 " (외 n건)"
fn more(base: String, extra: usize) -> String {
    if extra > 0 {
        format!("{base} (외 {extra}건)")
    } else {
        base
    }
}

/// `call` 이 `{elapsed}` 자리를 둔 호출인가. `call` 의 틀과 짝이다
pub fn call_timed(call: &PendingCall, e: &Employee) -> bool {
    match call.tool {
        ToolKind::Script
        | ToolKind::Shell
        | ToolKind::GitQuery
        | ToolKind::GitPush
        | ToolKind::PrCreate
        | ToolKind::Release => true,
        ToolKind::Test => e.red.is_none(),
        _ => false,
    }
}

/// 서브 생성 (docs/CONTRACTS.md C3). 이름 대신 직급과 description, 없으면 prompt 앞부분
pub fn spawn(rank: Rank, what: &str) -> String {
    format!("{}에게 \"{}\" 지시", rank_name(rank), cut(what, QUOTE))
}

pub fn background(call: &PendingCall, jobs: usize) -> String {
    format!(
        "백그라운드: {} 돌아가는 중 (작업표 {jobs}장)",
        subject(call)
    )
}

/// 확인된 성공. Mixed 명령은 결과를 보증하지 못해 "완료"만 적는다 (docs/CONTRACTS.md C7)
pub fn done(call: &PendingCall, declares: bool, test: Option<TestResult>, pens: u32) -> String {
    let s = &call.summary;
    match &call.tool {
        ToolKind::FileRead => format!("{} 열람", path(call)),
        ToolKind::FileSearch => match &s.pattern {
            Some(p) => format!("'{}' 검색", code(p)),
            None => format!("{} 검색", command(call, "서류철")),
        },
        ToolKind::FileWrite => format!("{} 작성", path(call)),
        ToolKind::FileEdit => match s.lines_changed {
            Some(n) => format!("{} 수정 ({n}줄)", path(call)),
            None => format!("{} 수정", path(call)),
        },
        ToolKind::Test if declares => match test {
            Some(t) => format!("합격 {} / 반려 {}", t.passed, t.failed),
            None => format!("{} 통과", command(call, "테스트")),
        },
        ToolKind::Build if declares => "빌드 성공".to_string(),
        ToolKind::GitPush | ToolKind::PrCreate if declares => "출고 완료".to_string(),
        ToolKind::Release if declares => "릴리스 완료".to_string(),
        ToolKind::GitCommit => "출고 포장 완료".to_string(),
        t if is_review(t) => review(pens),
        ToolKind::WebSearch | ToolKind::WebFetch => "외근 복귀".to_string(),
        ToolKind::Mcp { server, .. } => format!("거래처 {} 미팅 완료", code(server)),
        ToolKind::Skill { name } => format!("외주 매뉴얼 {} 확인", code(name)),
        ToolKind::Spawn => "업무 지시 완료".to_string(),
        ToolKind::Ask | ToolKind::ExitPlanMode => "결재 받음".to_string(),
        ToolKind::Todo => "칸반 보드 정리".to_string(),
        _ => format!("{} 완료", subject(call)),
    }
}

/// 결과 본문에서 사고 원인 한 줄. 에러 낱말이 든 줄, 없으면 "Exit code" 아닌 첫 줄
fn cause(preview: &str) -> Option<&str> {
    let lines = || preview.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| {
            let low = l.to_lowercase();
            ["error", "fail", "panic", "fatal", "denied", "rejected"]
                .iter()
                .any(|w| low.contains(w))
        })
        .or_else(|| lines().find(|l| !l.starts_with("Exit code")))
        .or_else(|| lines().next())
}

pub fn accident(call: &PendingCall, preview: &str) -> String {
    let why = cause(preview).map(|l| code(&cut(l, LINE)));
    match &call.tool {
        ToolKind::GitPush | ToolKind::PrCreate => {
            format!("반송: {}", why.unwrap_or_else(|| "push 거부됨".to_string()))
        }
        _ => format!("사고: {}", why.unwrap_or_else(|| subject(call))),
    }
}

/// 실패한 테스트 이름 (cargo `test x ... FAILED`, pytest `FAILED x - …`). 못 찾으면 명령
pub fn failed_test(call: &PendingCall, preview: &str) -> String {
    preview
        .lines()
        .map(str::trim)
        .find_map(|l| {
            l.strip_prefix("test ")
                .and_then(|r| r.strip_suffix(" ... FAILED"))
                .or_else(|| {
                    l.strip_prefix("FAILED ")
                        .and_then(|r| r.split_whitespace().next())
                })
        })
        .map(str::to_string)
        .or_else(|| call.summary.command.clone())
        .unwrap_or_else(|| label(&call.tool))
}

/// `rejected` 문장의 머리. 상태 단어(반려)가 이것으로 문장을 알아본다
const REJECTED: &str = "반려됨: ";
pub fn rejected(call: &PendingCall) -> String {
    format!("{REJECTED}{} 안 됩니다", subject(call))
}
pub fn is_rejected(s: &str) -> bool {
    s.starts_with(REJECTED)
}
/// 고정 문구를 쓰고 추정 표시는 confidence 가 한다 (docs/CONTRACTS.md C2·C8).
pub fn unknown(call: &PendingCall) -> String {
    format!("결과 확인 불가: {}", subject(call))
}
/// 셸 2분 초과. 명령 없는 호출(편집 허가 대기 등)은 문장을 그대로 둔다
pub fn long_run(call: &PendingCall, extra: usize) -> Option<String> {
    let c = call.summary.command.as_deref()?;
    Some(more(
        format!("서버실에서 땀 흘리는 중 ({} · {{elapsed}})", code(c)),
        extra,
    ))
}
pub fn asking(q: &str) -> String {
    if q.trim().is_empty() {
        ASKING.to_string()
    } else {
        format!("{ASKING}: 「{}」", cut(q, QUOTE))
    }
}
pub fn escalated(call: &PendingCall) -> String {
    format!("{} 결재판 들고 문 앞 대기 중인 듯", subject(call))
}
pub fn retired(c: Confidence) -> String {
    match c {
        Confidence::Confirmed => "탕비실에서 쉬는 중".to_string(),
        Confidence::Inferred => "탕비실에서 게임 중인 듯".to_string(),
    }
}
pub fn departed(c: Confidence) -> String {
    match c {
        Confidence::Confirmed => "퇴근함".to_string(),
        Confidence::Inferred => "퇴근한 듯".to_string(),
    }
}
