//! 담당 (PLAN 4.7.5)과 오늘의 업무 (4.7.6). 이벤트 순서만 보고 정해서 재구성과 실행 중 적용이 같다

use std::collections::HashSet;

use serde::Serialize;
use ts_rs::TS;

use crate::parse::{ToolKind, ToolOutcome};

/// 담당. 선언 순서가 동률일 때의 우선순위다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum Role {
    Backend,
    Frontend,
    Qa,
    DevOps,
    Writer,
    Trainee,
}

const CATEGORIES: [Role; 5] = [
    Role::Backend,
    Role::Frontend,
    Role::Qa,
    Role::DevOps,
    Role::Writer,
];

/// 이 수 미만의 서로 다른 파일을 만졌으면 수습 (PLAN 에 기준이 없어 경로 기준으로 정했다)
const TRAINEE_BELOW: usize = 5;

/// 직원이 만진 서로 다른 파일과 분류별 수
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Touched {
    seen: HashSet<String>,
    counts: [u32; 5],
}

impl Touched {
    pub fn touch(&mut self, path: &str) -> Role {
        if self.seen.insert(path.to_string())
            && let Some(c) = category(path)
        {
            self.counts[CATEGORIES.iter().position(|r| *r == c).unwrap()] += 1;
        }
        self.role()
    }

    fn role(&self) -> Role {
        if self.seen.len() < TRAINEE_BELOW {
            return Role::Trainee;
        }
        let mut best = (0, Role::Trainee);
        for (i, &n) in self.counts.iter().enumerate() {
            if n > best.0 {
                best = (n, CATEGORIES[i]);
            }
        }
        best.1
    }
}

pub fn touches_file(tool: &ToolKind) -> bool {
    matches!(
        tool,
        ToolKind::FileRead | ToolKind::FileWrite | ToolKind::FileEdit | ToolKind::FileSearch
    )
}

/// 경로·확장자 → 분류. 경로 규칙(데브옵스·QA)이 확장자보다 앞선다
fn category(path: &str) -> Option<Role> {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    // 단어 단위로 본다. `ci` 가 `decision` 에, `api` 가 `rapid` 에 걸리면 안 된다
    let has = |words: &[&str]| {
        p.split(|c: char| !c.is_alphanumeric())
            .any(|t| words.contains(&t))
    };
    if p.split('/').any(|c| c == ".github") || has(&["ci"]) {
        return Some(Role::DevOps);
    }
    if has(&["test", "tests", "spec", "specs", "fixtures"]) {
        return Some(Role::Qa);
    }
    if name == "dockerfile" || name.starts_with("dockerfile.") {
        return Some(Role::DevOps);
    }
    match ext {
        "rs" | "go" | "py" | "java" | "kt" | "cs" | "rb" | "php" | "c" | "cpp" | "h" => {
            Some(Role::Backend)
        }
        "tsx" | "jsx" | "vue" | "svelte" | "css" | "scss" | "html" => Some(Role::Frontend),
        "yml" | "yaml" | "toml" | "sh" | "ps1" => Some(Role::DevOps),
        "md" | "mdx" | "rst" | "txt" => Some(Role::Writer),
        "ts" | "js" => Some(if p.contains("src/server") || has(&["api", "backend"]) {
            Role::Backend
        } else {
            Role::Frontend
        }),
        _ => None,
    }
}

/// 오늘의 업무
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum WorkType {
    CodeReview,
    BugFix,
    Refactor,
    Docs,
    Research,
    NewDev,
}

/// 첫 20개 호출까지만 본다. 그 뒤는 고정 (4.7.6)
const WINDOW: u32 = 20;
/// 패턴 기준은 PLAN 에 숫자가 없어 정했다
const REFACTOR_MIN_EDITS: u32 = 3;
const RESEARCH_MIN_CALLS: u32 = 3;

/// 표 순서대로 다섯 행. 신규 개발은 "그 외"라 키워드가 없다
const KEYWORDS: [&[&str]; 5] = [
    &["리뷰", "review"],
    &["버그", "오류", "에러", "fix", "안 돼"],
    &["리팩터", "정리", "refactor"],
    &["문서", "readme", "docs"],
    &["조사", "찾아", "알아봐", "가능한지"],
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkTally {
    /// 첫 사용자 메시지를 봤으면 행별 키워드 적중
    keywords: Option<[bool; 5]>,
    calls: u32,
    writes: u32,
    md_writes: u32,
    reads: u32,
    /// 쓰기 전에 GitDiff·PrView 가 나왔다
    review_first: bool,
    /// 첫 Test 결과가 실패였나
    first_test_failed: Option<bool>,
    test_failed: bool,
    test_ok: bool,
    /// 창 안의 Test 호출. 결과가 20번째 호출 뒤에 와도 센다
    open_tests: HashSet<String>,
}

impl WorkTally {
    fn frozen(&self) -> bool {
        self.calls >= WINDOW
    }

    pub fn message(&mut self, text: &str) {
        if self.keywords.is_some() || self.frozen() {
            return;
        }
        let t = text.to_lowercase();
        self.keywords = Some(KEYWORDS.map(|ks| ks.iter().any(|k| t.contains(k))));
    }

    pub fn call(&mut self, call_id: &str, tool: &ToolKind, path: Option<&str>) {
        if self.frozen() {
            return;
        }
        self.calls += 1;
        match tool {
            ToolKind::FileEdit | ToolKind::FileWrite => {
                self.writes += 1;
                let ext = path
                    .and_then(|p| p.rsplit_once('.'))
                    .map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
                if matches!(ext.as_str(), "md" | "mdx" | "rst" | "txt") {
                    self.md_writes += 1;
                }
            }
            ToolKind::GitDiff | ToolKind::PrView if self.writes == 0 => self.review_first = true,
            ToolKind::Test => {
                self.open_tests.insert(call_id.to_string());
            }
            ToolKind::FileRead
            | ToolKind::FileSearch
            | ToolKind::WebSearch
            | ToolKind::WebFetch => self.reads += 1,
            _ => {}
        }
    }

    pub fn result(&mut self, call_id: &str, outcome: ToolOutcome) {
        if matches!(outcome, ToolOutcome::Started) || !self.open_tests.remove(call_id) {
            return;
        }
        let failed = outcome == ToolOutcome::Err;
        self.first_test_failed.get_or_insert(failed);
        if failed {
            self.test_failed = true;
        } else if outcome == ToolOutcome::Ok {
            self.test_ok = true;
        }
    }

    /// 근거가 없으면(첫 메시지 전, 호출 0개) None 이라 지금 값을 유지한다
    pub fn verdict(&self) -> Option<WorkType> {
        if self.keywords.is_none() && self.calls == 0 {
            return None;
        }
        let kw = self.keywords.unwrap_or_default();
        let refactor = self.writes >= REFACTOR_MIN_EDITS
            && self.writes * 2 >= self.calls
            && self.test_ok
            && !self.test_failed;
        let docs = self.writes > 0 && self.md_writes * 2 > self.writes;
        let research =
            self.writes == 0 && self.calls >= RESEARCH_MIN_CALLS && self.reads * 2 >= self.calls;
        Some(if kw[0] || self.review_first {
            WorkType::CodeReview
        } else if kw[1] || self.first_test_failed == Some(true) {
            WorkType::BugFix
        } else if kw[2] || refactor {
            WorkType::Refactor
        } else if kw[3] || docs {
            WorkType::Docs
        } else if kw[4] || research {
            WorkType::Research
        } else {
            WorkType::NewDev
        })
    }
}
