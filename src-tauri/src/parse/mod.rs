//! 제품별 레코드를 공통 Event로 (PLAN 4.6)

pub mod claude;
pub mod codex;

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::discover::Product;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Event {
    /// 메타 레코드(custom-title 등)에는 없다. None이면 상태 기계가 직전 이벤트 시각을 쓴다
    pub ts: Option<DateTime<Utc>>,
    pub session_id: String,
    /// 메인은 session_id와 같음, 서브는 고유 id
    pub agent_id: String,
    pub parent_agent: Option<String>,
    pub kind: EventKind,
    /// 팝업 "원본 열기"·타임라인 페이징용
    pub raw_ref: RawRef,
}

/// `generation` 은 파일 세대 (4.5)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RawRef {
    pub path: PathBuf,
    pub generation: u64,
    pub byte_offset: u64,
    pub line_no: u64,
}

/// 파일 하나(메인 또는 서브 기록)마다 하나 둔다. 파일이 Reset되면 새로 만든다 (4.3)
pub trait Parser {
    /// TailReader의 한 줄을 이벤트로. 연출로 못 바꾼 모양은 `unhandled` 에 구조만 넣는다
    fn parse_line(
        &mut self,
        bytes: &[u8],
        raw_ref: &RawRef,
        unhandled: &mut Vec<Unhandled>,
    ) -> Vec<Event>;
}

/// 원본 레코드를 보는 파서만 아는 미대응 재료 (4.11). 원문 값은 담지 않는다.
/// 이벤트에서 알 수 있는 kind(`command`·`mcp_server`·`skill` 등)와 집계·저장은 diag 몫이다
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Unhandled {
    /// 모르는 최상위 `type`, Codex `payload.type`. `keys` 는 최상위 키 이름
    RecordType {
        type_name: String,
        keys: Vec<String>,
        renamed_turn: bool,
    },
    SystemSubtype {
        subtype: String,
    },
    /// ToolKind::Other 로 떨어진 도구 (내부 배관 제외)
    ToolName {
        name: String,
        input_keys: Vec<String>,
    },
    /// 성공·실패를 판별하지 못한 결과. `result_keys` 는 결과의 최상위 키 이름
    OutcomeUnknown {
        tool: String,
        result_keys: Vec<String>,
    },
    /// 모르는 Claude `entrypoint`·Codex `originator` 값
    ProductValue {
        value: String,
    },
}

/// 확인됨은 레코드가 직접 말한 것, 추정은 본문 정규식 등으로 짐작한 것 (PLAN 2.5)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Confidence {
    Confirmed,
    Inferred,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum EventKind {
    UserMessage {
        text: String,
    },
    /// 본문이 거의 비어 있어 preview를 두지 않는다. 연출 신호가 아니다 (4.7.2)
    Thinking,
    AssistantText {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool: ToolKind,
        summary: ToolSummary,
    },
    ToolResult {
        call_id: String,
        outcome: ToolOutcome,
        outcome_confidence: Confidence,
        preview: String,
        test_result: Option<TestResult>,
    },
    /// 자식 ID가 확인된 시점에 낸다
    SubagentSpawn {
        child_id: String,
        prompt: String,
        model: Option<String>,
        description: Option<String>,
        /// Codex 서브의 reasoning_effort. Claude 서브는 meta.json 사전 등록이 맡아 None
        effort: Option<String>,
        depth: u8,
    },
    /// 완료 기록에서 낸다 (4.6.1~4.6.3). 못 봤으면 추정 (4.7.2)
    SubagentDone {
        child_id: String,
    },
    /// 확인됨: AskUserQuestion / ExitPlanMode / Codex request_user_input_async / Antigravity ask_question
    PermissionRequest {
        question: String,
        /// Claude ExitPlanMode
        plan: bool,
    },
    ContextCompaction {
        pre_tokens: Option<u64>,
        post_tokens: Option<u64>,
    },
    TitleChanged {
        title: String,
    },
    PrLinked {
        number: u32,
        url: String,
    },
    TodoChanged {
        items: Vec<TodoItem>,
    },
    TokenUsage {
        /// input + cache_creation + cache_read
        input_total: u64,
        output: u64,
        context_window: Option<u64>,
    },
    SessionMeta {
        cwd: PathBuf,
        model: Option<String>,
        title: Option<String>,
        product: Product,
        git_branch: Option<String>,
    },
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ToolOutcome {
    /// 구조 필드(is_error=false, exit_code=0)로 확인된 성공
    Ok,
    /// 구조 필드로 확인된 실패
    Err,
    /// 사장님 반려
    Rejected,
    /// 백그라운드 작업이 시작됐다는 중간 응답. 호출은 끝나지 않았다
    Started,
    /// 판별 불가. 본문 정규식으로 짐작한 값은 outcome_confidence = Inferred 로 Ok/Err를 주되 연출에는 쓰지 않는다
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ToolKind {
    FileRead,
    FileSearch,
    FileWrite,
    FileEdit,
    Shell,
    Script,
    Build,
    Test,
    GitQuery,
    GitDiff,
    GitCommit,
    GitPush,
    PrView,
    PrCreate,
    Release,
    Spawn,
    Ask,
    Mcp { server: String, tool: String },
    Skill { name: String },
    WebSearch,
    WebFetch,
    Todo,
    Other(String),
}

/// 문장 생성에 필요한 최소 정보
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolSummary {
    /// 상대 경로
    pub path: Option<String>,
    pub range: Option<(u32, u32)>,
    pub pattern: Option<String>,
    /// 표시용 40자 절단
    pub command: Option<String>,
    /// 평균 소요 키: 대표 세그먼트의 첫 토큰 + 서브커맨드 (`cargo build`, `npm test`, `git push`)
    pub command_key: Option<String>,
    /// 장애 판정 키: 공백 정규화한 원문 전체
    pub command_norm: Option<String>,
    pub compound: Compound,
    /// Claude `run_in_background: true`, Codex 백그라운드 플래그
    pub background: bool,
    /// max(old_string 줄 수, new_string 줄 수)
    pub lines_changed: Option<u32>,
    /// Codex `sandbox_permissions: "require_escalated"` 로 승인 창을 요청한 호출
    pub escalated: bool,
    /// `git commit -m` 의 메시지 (커밋 문장용)
    pub commit_message: Option<String>,
}

/// 합격·반려 숫자. 종료 코드가 아니라 결과 본문에서 온다
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TestResult {
    pub passed: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

/// 파이프(pipefail 꺼짐)·`||`·`;`·개행·단독 `&`·분석 불가 구문은 개별 명령의 성공을 보증하지 못해 Mixed (4.6.4)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Compound {
    Single,
    /// `&&` 만
    AndOnly,
    Mixed,
}

/// `classify_command` 결과. ToolSummary와 ToolKind를 채우는 재료
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub kind: ToolKind,
    pub compound: Compound,
    pub command_key: String,
    pub command_norm: String,
    pub command: String,
    /// FileRead 대표일 때
    pub path: Option<String>,
    /// GitCommit 대표일 때 `-m` 값
    pub commit_message: Option<String>,
    /// 단독 `&` 로 띄운 명령이 있다. 셸이 즉시 0을 돌려주므로 호출 결과는 Unknown으로 본다
    pub async_unknown: bool,
}

struct Scan {
    segs: Vec<Vec<String>>,
    has_and: bool,
    mixed: bool,
    async_bg: bool,
}

/// 따옴표 안과 `\` 뒤의 구분자는 자르지 않는 간단한 상태 머신
fn scan(cmd: &str) -> Scan {
    let chars: Vec<char> = cmd.chars().collect();
    let mut s = Scan {
        segs: Vec::new(),
        has_and: false,
        mixed: false,
        async_bg: false,
    };
    let mut seg: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has_tok = false;
    let mut quote: Option<char> = None;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;

    fn flush(seg: &mut Vec<String>, cur: &mut String, has_tok: &mut bool) {
        if *has_tok {
            seg.push(std::mem::take(cur));
            *has_tok = false;
        }
    }
    fn end(segs: &mut Vec<Vec<String>>, seg: &mut Vec<String>) {
        if !seg.is_empty() {
            segs.push(std::mem::take(seg));
        }
    }

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && c == '\\' && matches!(next, Some('"' | '\\' | '$' | '`')) {
                cur.push(next.unwrap_or(c));
                i += 1;
            } else {
                if q == '"' && (c == '`' || (c == '$' && next == Some('('))) {
                    s.mixed = true;
                }
                cur.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                has_tok = true;
            }
            '\\' => match next {
                Some('\n') => i += 1, // 줄 잇기
                // Windows 경로(`C:\x`)의 역슬래시는 살린다
                Some(n) if "|&;\"' \\()<>$`".contains(n) => {
                    cur.push(n);
                    has_tok = true;
                    i += 1;
                }
                _ => {
                    cur.push('\\');
                    has_tok = true;
                }
            },
            // 단어 첫머리 `#` 은 줄 끝까지 주석이라 실행되지 않는다. 개행은 남겨 기존 분기가 처리한다
            '#' if !has_tok => {
                while i + 1 < chars.len() && chars[i + 1] != '\n' {
                    i += 1;
                }
            }
            ' ' | '\t' | '\r' => flush(&mut seg, &mut cur, &mut has_tok),
            '\n' | ';' => {
                flush(&mut seg, &mut cur, &mut has_tok);
                end(&mut s.segs, &mut seg);
                s.mixed = true;
                if c == '\n' && !heredocs.is_empty() {
                    i = skip_heredoc_bodies(&chars, i + 1, &mut heredocs);
                    continue;
                }
            }
            '|' => {
                flush(&mut seg, &mut cur, &mut has_tok);
                end(&mut s.segs, &mut seg);
                s.mixed = true;
                if matches!(next, Some('|' | '&')) {
                    i += 1;
                }
            }
            '&' => {
                if (has_tok && cur.ends_with(['>', '<'])) || next == Some('>') {
                    // 리다이렉트 `2>&1`, `&>`
                    cur.push(c);
                    has_tok = true;
                } else if next != Some('&')
                    && ((seg.is_empty() && !has_tok)
                        || (has_tok && cur.ends_with(['{', '(']))
                        || (!has_tok && seg.last().is_some_and(|t| t.ends_with(['{', '(']))))
                {
                    // 세그먼트 첫 토큰이나 `{`·`(` 바로 뒤의 `&` 는 PowerShell 호출 연산자. 비동기가 아니다
                } else if next == Some('&') {
                    flush(&mut seg, &mut cur, &mut has_tok);
                    end(&mut s.segs, &mut seg);
                    s.has_and = true;
                    i += 1;
                } else {
                    flush(&mut seg, &mut cur, &mut has_tok);
                    end(&mut s.segs, &mut seg);
                    s.mixed = true;
                    s.async_bg = true;
                }
            }
            '(' | ')' | '`' => {
                s.mixed = true;
                cur.push(c);
                has_tok = true;
            }
            '<' if next == Some('<') => {
                s.mixed = true; // heredoc, here-string
                match (chars.get(i + 2) != Some(&'<'))
                    .then(|| heredoc_delim(&chars, i + 2))
                    .flatten()
                {
                    Some((delim, strip_tabs, after)) => {
                        // 본문은 다음 개행 뒤에서 건너뛴다
                        flush(&mut seg, &mut cur, &mut has_tok);
                        heredocs.push((delim, strip_tabs));
                        i = after;
                        continue;
                    }
                    None => {
                        cur.push(c);
                        has_tok = true;
                    }
                }
            }
            _ => {
                cur.push(c);
                has_tok = true;
            }
        }
        i += 1;
    }
    flush(&mut seg, &mut cur, &mut has_tok);
    end(&mut s.segs, &mut seg);
    // 닫히지 않은 따옴표, 제어 구문 키워드, 중괄호 묶음은 분석하지 않는다
    if quote.is_some()
        || s.segs.iter().any(|t| {
            matches!(
                t[0].as_str(),
                "for" | "while" | "until" | "if" | "case" | "{" | "}"
            )
        })
    {
        s.mixed = true;
    }
    s
}

/// `<<` 뒤(`from`)의 구분 단어. (단어, `<<-` 여부, 단어 다음 위치)
fn heredoc_delim(chars: &[char], from: usize) -> Option<(String, bool, usize)> {
    let mut j = from;
    let strip_tabs = chars.get(j) == Some(&'-');
    if strip_tabs {
        j += 1;
    }
    while matches!(chars.get(j), Some(' ' | '\t')) {
        j += 1;
    }
    let mut word = String::new();
    let q = chars.get(j).copied().filter(|c| matches!(c, '\'' | '"'));
    if q.is_some() {
        j += 1;
    }
    while let Some(&c) = chars.get(j) {
        let stop = match q {
            Some(q) => c == q,
            None => c.is_whitespace() || ";|&<>()".contains(c),
        };
        if stop {
            break;
        }
        word.push(c);
        j += 1;
    }
    if q.is_some() && j < chars.len() {
        j += 1; // 닫는 따옴표
    }
    (!word.is_empty()).then_some((word, strip_tabs, j))
}

/// 개행 다음(`i`)부터 각 구분 단어만 있는 줄까지 건너뛰고, 그다음 위치를 돌려준다
fn skip_heredoc_bodies(chars: &[char], mut i: usize, pending: &mut Vec<(String, bool)>) -> usize {
    for (delim, strip_tabs) in pending.drain(..) {
        while i < chars.len() {
            let end = chars[i..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |p| i + p);
            let line: String = chars[i..end].iter().collect();
            i = (end + 1).min(chars.len());
            let line = line.trim_end_matches('\r');
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if line == delim {
                break;
            }
        }
    }
    i
}

struct SegClass {
    kind: ToolKind,
    key: String,
    path: Option<String>,
    msg: Option<String>,
}

/// 앞의 `FOO=1` 대입과 실행 파일의 경로·`.exe` 는 벗기고 본다. `npx`·`python -m` 같은
/// 실행기 뒤의 명령은 따라가지 않는다 (표에 없으면 Shell)
fn classify_segment(toks: &[String]) -> SegClass {
    let start = toks
        .iter()
        .position(|t| !is_env_assign(t))
        .unwrap_or(toks.len());
    let toks = &toks[start..];
    let Some(first) = toks.first() else {
        return SegClass {
            kind: ToolKind::Shell,
            key: String::new(),
            path: None,
            msg: None,
        };
    };
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let prog = match base.len().checked_sub(4) {
        Some(n) if base.is_char_boundary(n) && base[n..].eq_ignore_ascii_case(".exe") => &base[..n],
        _ => base,
    };
    let mut args: Vec<&str> = toks[1..].iter().map(String::as_str).collect();
    // ponytail: 그 밖의 앞 옵션(`git --no-pager` 등)은 서브커맨드로 읽혀 Shell 이 된다, 오탐이 보이면 올린다
    let skip = match prog {
        "git" => {
            let mut n = 0;
            while matches!(args.get(n), Some(&"-C" | &"-c")) && n + 1 < args.len() {
                n += 2;
            }
            n
        }
        "cargo" => usize::from(args.first().is_some_and(|a| a.starts_with('+'))),
        _ => 0,
    };
    args.drain(..skip);
    let sub = args.first().copied().unwrap_or("");
    let sub2 = args.get(1).copied().unwrap_or("");
    let after_sub = args.get(1..).unwrap_or(&[]);

    let mut path = None;
    let mut msg = None;
    let kind = match prog {
        "cargo" => match sub {
            "test" => ToolKind::Test,
            "build" | "check" => ToolKind::Build,
            _ => ToolKind::Shell,
        },
        "pytest" | "vitest" | "jest" | "phpunit" => ToolKind::Test,
        "npm" => match sub {
            "test" => ToolKind::Test,
            "run" if sub2.starts_with("test") => ToolKind::Test,
            "run" if sub2 == "build" => ToolKind::Build,
            _ => ToolKind::Shell,
        },
        "go" | "dotnet" => match (prog, sub) {
            (_, "test") => ToolKind::Test,
            ("go", "build") => ToolKind::Build,
            _ => ToolKind::Shell,
        },
        "make" | "tsc" | "gradle" | "mvn" => ToolKind::Build,
        "git" => match sub {
            "status" | "log" | "show" => ToolKind::GitQuery,
            "branch" if after_sub.iter().all(|a| matches!(*a, "-a" | "--list")) => {
                ToolKind::GitQuery
            }
            "tag"
                if after_sub.is_empty()
                    || after_sub.iter().any(|a| matches!(*a, "-l" | "--list")) =>
            {
                ToolKind::GitQuery
            }
            "tag" if after_sub.iter().any(|a| matches!(*a, "-d" | "--delete")) => ToolKind::Shell,
            "tag" => ToolKind::Release,
            "diff" => ToolKind::GitDiff,
            "commit" => {
                msg = commit_message(after_sub);
                ToolKind::GitCommit
            }
            "push" => ToolKind::GitPush,
            _ => ToolKind::Shell,
        },
        "gh" => match (sub, sub2) {
            ("release", "list" | "view") => ToolKind::GitQuery,
            ("release", "create") => ToolKind::Release,
            ("pr", "view" | "diff" | "list") => ToolKind::PrView,
            ("pr", "create") => ToolKind::PrCreate,
            _ => ToolKind::Shell,
        },
        "cat" | "head" | "tail" | "less" => {
            path = file_arg(&args, false);
            ToolKind::FileRead
        }
        "sed" if args.contains(&"-n") && !args.iter().any(|a| is_sed_in_place(a)) => {
            path = file_arg(&args, true);
            ToolKind::FileRead
        }
        "grep" | "rg" | "find" | "fd" => ToolKind::FileSearch,
        _ => ToolKind::Shell,
    };

    // 평균 소요 키. 서브커맨드가 있는 도구만 둘째 토큰을 붙인다 (`pytest a.py` 와 `pytest b.py` 는 같은 키)
    let key = if matches!(prog, "cargo" | "npm" | "git" | "gh" | "go" | "dotnet")
        && !sub.is_empty()
        && !sub.starts_with('-')
    {
        // `npm run build`, `gh pr create` 는 둘째 단어까지 붙인다
        let two = matches!((prog, sub), ("npm", "run") | ("gh", "pr" | "release"))
            && !sub2.is_empty()
            && !sub2.starts_with('-');
        if two {
            format!("{prog} {sub} {sub2}")
        } else {
            format!("{prog} {sub}")
        }
    } else if matches!(prog, "pnpm" | "yarn") && sub == "run" && !sub2.is_empty() {
        format!("{prog} run {sub2}")
    } else {
        prog.to_string()
    };
    SegClass {
        kind,
        key,
        path,
        msg,
    }
}

/// `-i`, `-i.bak`, `-ni`, `--in-place[=x]`
fn is_sed_in_place(a: &str) -> bool {
    a.starts_with("--in-place") || (a.starts_with('-') && !a.starts_with("--") && a.contains('i'))
}

fn is_env_assign(t: &str) -> bool {
    match t.split_once('=') {
        Some((k, _)) => {
            let mut c = k.chars();
            c.next()
                .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
                && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
        }
        None => false,
    }
}

/// 옵션이 아닌 마지막 인자. `head -n 5 f` 의 5, `sed -n 1,5p f` 의 스크립트는 파일이 아니다
// ponytail: 값을 따로 받는 다른 옵션(`-c`, `-s` 등)은 모른다, 오탐이 보이면 올린다
fn file_arg(args: &[&str], skip_script: bool) -> Option<String> {
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if *a == "-n" && !skip_script {
            it.next();
        } else if !a.starts_with('-') {
            rest.push(*a);
        }
    }
    if skip_script && !rest.is_empty() {
        rest.remove(0);
    }
    rest.last().map(|s| s.to_string())
}

fn commit_message(args: &[&str]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if *a == "-m" || *a == "--message" {
            return it.next().map(|s| message_value(s));
        }
        if let Some(v) = a.strip_prefix("--message=") {
            return Some(message_value(v));
        }
        // ponytail: `-amfoo` 처럼 묶음 속 `m` 뒤에 붙은 메시지는 모른다, 오탐이 보이면 올린다
        if !a.starts_with("--")
            && let Some(v) = a.strip_prefix("-m")
        {
            return Some(message_value(v));
        }
        // `-am x` 처럼 묶음이 `m` 으로 끝나면 다음 인자가 메시지
        if a.len() > 2 && a.starts_with('-') && !a.starts_with("--") && a.ends_with('m') {
            return it.next().map(|s| message_value(s));
        }
    }
    None
}

/// `-m` 값. Claude Code 의 `"$(cat <<'EOF' … EOF)"` 꼴이면 본문 첫 줄
fn message_value(v: &str) -> String {
    let first = v
        .strip_prefix("$(cat <<")
        .map(|r| r.strip_prefix('-').unwrap_or(r))
        .and_then(|r| r.split_once('\n'))
        .and_then(|(_, body)| body.lines().next());
    first.unwrap_or(v).to_string()
}

/// 대표 분류 우선순위 (4.6.4 3번). 마지막 명령 기준이 아니다
fn rank(k: &ToolKind) -> u8 {
    match k {
        ToolKind::Release => 11,
        ToolKind::PrCreate => 10,
        ToolKind::GitPush => 9,
        ToolKind::GitCommit => 8,
        ToolKind::Test => 7,
        ToolKind::Build => 6,
        ToolKind::GitDiff => 5,
        ToolKind::PrView => 4,
        ToolKind::GitQuery => 3,
        ToolKind::FileSearch => 2,
        ToolKind::FileRead => 1,
        _ => 0,
    }
}

/// 셸 명령 문자열 하나를 분류한다 (PLAN 4.6.4)
pub fn classify_command(cmd: &str) -> Classified {
    let s = scan(cmd);
    let mut best: Option<SegClass> = None;
    for seg in &s.segs {
        let c = classify_segment(seg);
        // 동률이면 앞 세그먼트가 이긴다
        if best.as_ref().is_none_or(|b| rank(&c.kind) > rank(&b.kind)) {
            best = Some(c);
        }
    }
    let best = best.unwrap_or(SegClass {
        kind: ToolKind::Shell,
        key: String::new(),
        path: None,
        msg: None,
    });
    let compound = if s.mixed {
        Compound::Mixed
    } else if s.has_and {
        Compound::AndOnly
    } else {
        Compound::Single
    };
    Classified {
        kind: best.kind,
        compound,
        command_key: best.key,
        command_norm: cmd.split_whitespace().collect::<Vec<_>>().join(" "),
        command: cmd.chars().take(40).collect(),
        path: best.path,
        commit_message: best.msg,
        async_unknown: s.async_bg,
    }
}

/// `word` 바로 앞의 숫자 (`3 passed` 의 3)
pub(crate) fn count_before(line: &str, word: &str) -> Option<u32> {
    line.match_indices(word).find_map(|(i, _)| {
        let head = line[..i].trim_end();
        let start = head
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()?
            .0;
        head[start..].parse().ok()
    })
}

/// 도장 숫자만 뽑는다. 성공·실패 판정에는 쓰지 않는다 (4.6.1)
pub(crate) fn parse_test_result(text: &str) -> Option<TestResult> {
    let pick = |l: &str| {
        count_before(l, "passed").map(|passed| TestResult {
            passed,
            failed: count_before(l, "failed").unwrap_or(0),
        })
    };
    let cargo: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("test result:"))
        .collect();
    if cargo.is_empty() {
        // pytest `1 failed, 3 passed in …`, jest `Tests: 1 failed, 3 passed, 4 total`
        return text.lines().rev().find_map(pick);
    }
    // cargo 는 크레이트·doc 테스트마다 한 줄씩 낸다
    cargo
        .into_iter()
        .filter_map(pick)
        .reduce(|a, b| TestResult {
            passed: a.passed.saturating_add(b.passed),
            failed: a.failed.saturating_add(b.failed),
        })
}

/// 4.6.1 매핑표의 `mcp__` 행. 도구 이름의 단어가 외근(웹)인지 거래처(Mcp)인지 정한다
pub(crate) fn mcp_kind(name: &str) -> ToolKind {
    let rest = name.strip_prefix("mcp__").unwrap_or(name);
    let (server, tool) = rest.split_once("__").unwrap_or((rest, ""));
    let lower = tool.to_ascii_lowercase();
    if lower.contains("search") {
        return ToolKind::WebSearch;
    }
    if ["fetch", "scrape", "crawl"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return ToolKind::WebFetch;
    }
    // Claude Desktop 커넥터는 서버 id가 UUID라 상호로 못 쓴다. JS 식별자라 `-` 가 `_` 로 바뀌어 있을 수 있다
    let uuid = server.len() == 36
        && server.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-' || c == '_',
            _ => c.is_ascii_hexdigit(),
        });
    let server = if uuid {
        tool.split('_').next().unwrap_or(tool)
    } else {
        server
    };
    ToolKind::Mcp {
        server: server.to_string(),
        tool: tool.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::ToolKind::*;
    use super::*;

    #[test]
    fn classifies_whole_table() {
        let cases: &[(&str, ToolKind)] = &[
            ("cargo test", Test),
            ("pytest a.py", Test),
            ("npm test", Test),
            ("npm run test", Test),
            ("npm run test:unit", Test),
            ("vitest", Test),
            ("jest", Test),
            ("go test ./...", Test),
            ("dotnet test", Test),
            ("phpunit", Test),
            ("cargo build", Build),
            ("cargo check", Build),
            ("npm run build", Build),
            ("make", Build),
            ("go build", Build),
            ("tsc", Build),
            ("gradle", Build),
            ("mvn", Build),
            ("git status", GitQuery),
            ("git log", GitQuery),
            ("git show", GitQuery),
            ("git branch", GitQuery),
            ("git branch -a", GitQuery),
            ("git branch --list", GitQuery),
            ("git tag", GitQuery),
            ("git tag -l", GitQuery),
            ("gh release list", GitQuery),
            ("gh release view v1", GitQuery),
            ("git diff", GitDiff),
            ("git commit -m x", GitCommit),
            ("git push", GitPush),
            ("gh pr view 3", PrView),
            ("gh pr diff 3", PrView),
            ("gh pr list", PrView),
            ("gh pr create", PrCreate),
            ("git tag v1", Release),
            ("git tag -a v1 -m x", Release),
            ("gh release create v1", Release),
            ("cat a", FileRead),
            ("head a", FileRead),
            ("tail a", FileRead),
            ("sed -n 1,5p a", FileRead),
            ("less a", FileRead),
            ("grep x a", FileSearch),
            ("rg x", FileSearch),
            ("find .", FileSearch),
            ("fd x", FileSearch),
            ("git branch feat", Shell),
            ("git checkout x", Shell),
            ("sed s/a/b/ f", Shell),
            ("ls", Shell),
        ];
        for (cmd, want) in cases {
            assert_eq!(&classify_command(cmd).kind, want, "{cmd}");
        }
    }

    #[test]
    fn strips_only_env_assign_and_path_for_unlisted_shapes() {
        let cases: &[(&str, ToolKind)] = &[
            ("FOO=1 cargo test", Test),
            ("/usr/bin/git push", GitPush),
            ("C:\\tools\\git.exe push", GitPush),
            ("npx vitest", Shell),
            ("python -m pytest", Shell),
            ("Get-ChildItem", Shell),
        ];
        for (cmd, want) in cases {
            assert_eq!(&classify_command(cmd).kind, want, "{cmd}");
        }
        assert_eq!(
            classify_command("FOO=1 cargo test").command_key,
            "cargo test"
        );
        // PowerShell의 `;` 도 구분자
        assert_eq!(classify_command("cd x; cargo test").kind, Test);
    }

    #[test]
    fn extracts_file_path_and_commit_message() {
        let p = |c: &str| classify_command(c).path;
        assert_eq!(p("cat a.txt"), Some("a.txt".into()));
        assert_eq!(p("head -n 5 a.txt"), Some("a.txt".into()));
        assert_eq!(p("tail -20 a.txt"), Some("a.txt".into()));
        assert_eq!(p("sed -n 1,5p src/a.rs"), Some("src/a.rs".into()));
        assert_eq!(p("head -n 5"), None);
        let m = |c: &str| classify_command(c).commit_message;
        assert_eq!(
            m("git commit -m \"fix: a && b\""),
            Some("fix: a && b".into())
        );
        assert_eq!(m("git commit -mx"), Some("x".into()));
        assert_eq!(m("git commit --message=y"), Some("y".into()));
        assert_eq!(m("git commit"), None);
    }

    #[test]
    fn keeps_separators_inside_quotes_and_after_backslash() {
        for cmd in [
            "git commit -m \"a && b\"",
            "git commit -m 'a | b; c'",
            "git commit -m a\\&\\&b",
            "git commit -m \"a\\\" && b\"",
        ] {
            let c = classify_command(cmd);
            assert_eq!(c.kind, GitCommit, "{cmd}");
            assert_eq!(c.compound, Compound::Single, "{cmd}");
        }
    }

    #[test]
    fn compound_picks_highest_priority_as_representative() {
        let k = |c: &str| classify_command(c).kind;
        assert_eq!(k("cargo test 2>&1 | tail -20"), Test);
        assert_eq!(k("cd x && npm test | head"), Test);
        assert_eq!(k("cargo build && git push"), GitPush);
        assert_eq!(k("git commit -m x && gh pr create"), PrCreate);
        assert_eq!(k("cat a; grep x a"), FileSearch);
    }

    #[test]
    fn compound_detection() {
        use Compound::*;
        let cases: &[(&str, Compound)] = &[
            ("cargo test", Single),
            ("cargo test 2>&1", Single),
            ("cargo test &> out.txt", Single),
            ("cd x && cargo build && git push", AndOnly),
            ("cargo test | tail", Mixed),
            ("git push || echo skipped", Mixed),
            ("cd x; cargo test", Mixed),
            ("cd x\ncargo test", Mixed),
            ("git push &", Mixed),
            ("(cd x && ls)", Mixed),
            ("echo $(date)", Mixed),
            ("echo \"$(date)\"", Mixed),
            ("echo `date`", Mixed),
            ("cat <<EOF\nhi\nEOF", Mixed),
            ("for f in a b; do cat $f; done", Mixed),
            ("while true; do ls; done", Mixed),
            ("if true; then ls; fi", Mixed),
            ("{ ls; }", Mixed),
            ("echo 'unclosed", Mixed),
        ];
        for (cmd, want) in cases {
            assert_eq!(classify_command(cmd).compound, *want, "{cmd}");
        }
    }

    #[test]
    fn lone_ampersand_is_unconfirmed() {
        let c = classify_command("git push &");
        assert!(c.async_unknown);
        assert_eq!(c.kind, GitPush);
        assert!(!classify_command("a && b 2>&1").async_unknown);
    }

    #[test]
    fn ampersand_after_open_brace_or_paren_is_call_operator() {
        assert!(!classify_command("{ & $x }").async_unknown);
        assert!(!classify_command("(& x)").async_unknown);
        assert!(classify_command("git push &").async_unknown);
    }

    #[test]
    fn key_is_for_average_norm_is_for_incident() {
        let a = classify_command("pytest a.py");
        let b = classify_command("pytest b.py");
        assert_eq!(a.command_key, b.command_key);
        assert_ne!(a.command_norm, b.command_norm);
        assert_eq!(
            classify_command("cargo   build  --release").command_key,
            "cargo build"
        );
        assert_eq!(
            classify_command("git push origin main").command_key,
            "git push"
        );
        assert_eq!(
            classify_command("cargo  test \n  -p   x").command_norm,
            "cargo test -p x"
        );
    }

    #[test]
    fn display_command_truncates_at_40_chars() {
        let cmd = "echo 가나다라마바사아자차카타파하".repeat(5);
        let c = classify_command(&cmd);
        assert_eq!(c.command.chars().count(), 40);
        assert!(cmd.starts_with(&c.command));
    }

    #[test]
    fn leading_ampersand_is_call_operator_not_async() {
        let c = classify_command("& 'C:/Git/git.exe' push");
        assert_eq!(c.kind, GitPush);
        assert_eq!(c.compound, Compound::Single);
        assert_eq!(c.command_key, "git push");
        assert!(!c.async_unknown);
        let c = classify_command("git push &");
        assert!(c.async_unknown);
        assert_eq!(c.compound, Compound::Mixed);
    }

    #[test]
    fn heredoc_body_is_not_classified() {
        let c = classify_command("cat > n.md <<EOF\nmake sure it works\nEOF");
        assert_ne!(c.kind, Build);
        assert_eq!(c.compound, Compound::Mixed);
        let c = classify_command("python - <<'EOF'\ngit push\nEOF");
        assert_eq!(c.kind, Shell);
        assert_eq!(c.compound, Compound::Mixed);
        let c = classify_command("cat <<-\"X\"\n\tcargo test\n\tX\ngit push");
        assert_eq!(c.kind, GitPush);
        assert_eq!(c.compound, Compound::Mixed);
    }

    #[test]
    fn commit_message_from_heredoc_and_bundled_flags() {
        let m = |c: &str| classify_command(c).commit_message;
        assert_eq!(
            m("git commit -m \"$(cat <<'EOF'\nfix: a\n\nbody\nEOF\n)\""),
            Some("fix: a".into())
        );
        assert_eq!(m("git commit -am \"x y\""), Some("x y".into()));
        let c = classify_command("git commit -m \"$(cat <<'EOF'\nfix: a\nEOF\n)\"");
        assert_eq!(c.kind, GitCommit);
        assert_eq!(c.compound, Compound::Mixed);
    }

    #[test]
    fn git_tag_variants() {
        let k = |c: &str| classify_command(c).kind;
        assert_eq!(k("git tag -l 'v*'"), GitQuery);
        assert_eq!(k("git tag v1 --list"), GitQuery);
        assert_eq!(k("git tag --list"), GitQuery);
        assert_eq!(k("git tag -d v1"), Shell);
        assert_eq!(k("git tag --delete v1"), Shell);
        assert_eq!(k("git tag v1"), Release);
    }

    #[test]
    fn two_word_command_keys() {
        let key = |c: &str| classify_command(c).command_key;
        assert_eq!(key("npm run build"), "npm run build");
        assert_eq!(key("pnpm run dev"), "pnpm run dev");
        assert_eq!(key("yarn run lint"), "yarn run lint");
        assert_eq!(key("gh pr create --title x"), "gh pr create");
        assert_eq!(key("gh release create v1"), "gh release create");
        assert_eq!(key("npm test"), "npm test");
    }

    #[test]
    fn global_options_before_subcommand() {
        let c = classify_command("git -C x push");
        assert_eq!((c.kind, c.command_key.as_str()), (GitPush, "git push"));
        let c = classify_command("git -c core.x=1 -C y commit -m z");
        assert_eq!((c.kind, c.command_key.as_str()), (GitCommit, "git commit"));
        assert_eq!(c.commit_message, Some("z".into()));
        let c = classify_command("cargo +nightly test");
        assert_eq!((c.kind, c.command_key.as_str()), (Test, "cargo test"));
        assert_eq!(classify_command("git --no-pager log").kind, Shell);
    }

    #[test]
    fn sed_in_place_is_not_a_read() {
        assert_eq!(classify_command("sed -n 1,5p a").kind, FileRead);
        assert_eq!(classify_command("sed -n -i 1,5p a").kind, Shell);
        assert_eq!(classify_command("sed -i.bak -n 1p a").kind, Shell);
        assert_eq!(classify_command("sed --in-place -n 1p a").kind, Shell);
    }

    #[test]
    fn shell_comment_is_not_executed() {
        let cases: &[(&str, ToolKind, Compound)] = &[
            (
                "echo ok # no deployment && git push",
                Shell,
                Compound::Single,
            ),
            ("git push # done", GitPush, Compound::Single),
            ("echo a#b && git push", GitPush, Compound::AndOnly),
            ("echo \"# x\" && git push", GitPush, Compound::AndOnly),
            ("echo $# && git push", GitPush, Compound::AndOnly),
            ("ls # c\ngit push", GitPush, Compound::Mixed),
            ("cat <<EOF # c\ngit push\nEOF", FileRead, Compound::Mixed),
        ];
        for (cmd, kind, compound) in cases {
            let c = classify_command(cmd);
            assert_eq!((&c.kind, &c.compound), (kind, compound), "{cmd}");
        }
    }
}
