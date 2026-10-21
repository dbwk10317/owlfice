//! 담당 (PLAN 4.7.5)과 오늘의 업무 (4.7.6). 이벤트 순서만 보고 정해서 재구성과 실행 중 적용이 같다

use std::collections::{HashMap, HashSet};
use std::path::Path;

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

/// 이 수 미만의 분류되는 파일(서로 다른 경로)을 만졌으면 수습. PLAN 에 기준이 없어 정했다
const TRAINEE_BELOW: usize = 5;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Touched {
    seen: HashSet<String>,
    counts: [u32; 5],
}

impl Touched {
    pub fn touch(&mut self, path: &str, cwd: &Path) -> Role {
        let key = norm(path);
        let rel = relative(&key, &norm(&cwd.to_string_lossy()));
        if let Some(c) = category(rel)
            && self.seen.insert(key)
        {
            self.counts[c as usize] += 1;
        }
        self.role()
    }

    fn role(&self) -> Role {
        if self.seen.len() < TRAINEE_BELOW {
            return Role::Trainee;
        }
        // ponytail: 동률은 4.7.5 나열 순서(백엔드부터)로 정한다, 다른 순서가 필요해지면 올린다
        let mut best = (0, Role::Trainee);
        for r in CATEGORIES {
            let n = self.counts[r as usize];
            if n > best.0 {
                best = (n, r);
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

fn norm(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}

/// 바깥 폴더 이름(`C:/Users/test/...`)이 분류를 정하지 않게 cwd 아래는 상대 경로로,
/// cwd 밖 절대 경로는 파일 이름만 남긴다
fn relative<'a>(key: &'a str, cwd: &str) -> &'a str {
    let cwd = cwd.trim_end_matches('/');
    if !cwd.is_empty()
        && let Some(rest) = key.strip_prefix(cwd)
        && let Some(rest) = rest.strip_prefix('/')
    {
        return rest;
    }
    let drive = key.as_bytes().get(1) == Some(&b':');
    if key.starts_with('/') || drive {
        return key.rsplit('/').next().unwrap_or(key);
    }
    key
}

fn ext_of(norm_path: &str) -> &str {
    let name = norm_path.rsplit('/').next().unwrap_or("");
    name.rsplit_once('.').map_or("", |(_, e)| e)
}

/// 테크라이터 확장자. 담당과 문서 작업 판정이 같이 쓴다
pub fn is_doc_ext(ext: &str) -> bool {
    matches!(ext, "md" | "mdx" | "rst" | "txt")
}

/// 경로·확장자 → 분류. 경로 규칙(데브옵스·QA)이 확장자보다 앞선다
fn category(p: &str) -> Option<Role> {
    let name = p.rsplit('/').next().unwrap_or("");
    let ext = ext_of(p);
    // 단어 단위로 본다. `ci` 가 `decision` 에, `api` 가 `rapid` 에 걸리면 안 된다
    let has = |words: &[&str]| {
        p.split(|c: char| !c.is_alphanumeric())
            .any(|t| words.contains(&t))
    };
    if p.split('/').any(|c| c == ".github") || has(&["ci"]) {
        return Some(Role::DevOps);
    }
    // ponytail: 파일 이름 속 test·spec(`a_test.rs`)도 QA 로 본다, 오분류가 잦으면 폴더만 보게 줄인다
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
        e if is_doc_ext(e) => Some(Role::Writer),
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

/// 표 순서대로 다섯 행. 신규 개발은 "그 외"라 키워드가 없다.
// ponytail: 부분 문자열 일치라 "prefix" 의 fix 에도 걸린다. 한글은 단어 경계가 어려워 그대로 둔다, 오판이 잦으면 영어만 단어 경계로 올린다
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
    review_first: bool,
    /// 첫 쓰기 전에 Test 가 실패했다
    fail_first: bool,
    test_failed: bool,
    test_ok: bool,
    /// 창 안의 Test 호출과 첫 쓰기 전이었나. 결과가 20번째 호출 뒤에 와도 센다
    open_tests: HashMap<String, bool>,
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
                if path.is_some_and(|p| is_doc_ext(ext_of(&norm(p)))) {
                    self.md_writes += 1;
                }
            }
            ToolKind::GitDiff | ToolKind::PrView if self.writes == 0 => self.review_first = true,
            ToolKind::Test => {
                self.open_tests
                    .insert(call_id.to_string(), self.writes == 0);
            }
            ToolKind::FileRead
            | ToolKind::FileSearch
            | ToolKind::WebSearch
            | ToolKind::WebFetch => self.reads += 1,
            _ => {}
        }
    }

    pub fn result(&mut self, call_id: &str, outcome: ToolOutcome) {
        if outcome == ToolOutcome::Started {
            return;
        }
        let Some(before_write) = self.open_tests.remove(call_id) else {
            return;
        };
        // 반려·Unknown 은 통과도 실패도 아니다
        match outcome {
            ToolOutcome::Ok => self.test_ok = true,
            ToolOutcome::Err => {
                self.test_failed = true;
                self.fail_first |= before_write;
            }
            _ => {}
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
        } else if kw[1] || self.fail_first {
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
