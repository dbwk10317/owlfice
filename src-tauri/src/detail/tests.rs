use std::fs;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde_json::json;

use super::*;
use crate::discover::{Host, Product, windows_host};
use crate::ipc::{office_id_str, team_id_str};
use crate::office::Tokens;
use crate::office::classify::{SubMeta, merge, rebuild};
use crate::office::names::NameBook;
use crate::office::world::Scale;
use crate::tail::scheduler::{codex_id, sub_meta};

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn wsl_host() -> Host {
    Host {
        id: "wsl:Test".into(),
        kind: HostKind::Wsl {
            distro: "Test".into(),
        },
        label: "Test".into(),
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &dest);
        } else {
            fs::copy(&p, &dest).unwrap();
        }
    }
}

/// TailReader 처럼 줄마다 실제 시작 바이트와 1 기준 줄 번호를 단다
fn parse_file(path: &Path, vendor: Vendor, generation: u64) -> Vec<Event> {
    let mut parser = make_parser(path, vendor);
    let mut unhandled = Vec::new();
    let mut out = Vec::new();
    let bytes = fs::read(path).unwrap();
    let mut offset = 0u64;
    for (i, line) in bytes.split_inclusive(|&b| b == b'\n').enumerate() {
        let raw = RawRef {
            path: path.to_path_buf(),
            generation,
            byte_offset: offset,
            line_no: i as u64 + 1,
        };
        offset += line.len() as u64;
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        out.extend(parser.parse_line(line, &raw, &mut unhandled));
    }
    out
}

/// 임시 폴더에 옮긴 세션: 메인 파일과 `<세션>/subagents/` 의 서브 기록
struct Session {
    _dir: tempfile::TempDir,
    key: TeamKey,
    product: Product,
    main: PathBuf,
    subs: Vec<PathBuf>,
}

impl Session {
    fn fixture(rel: &str, host: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../fixtures")
                .join(rel),
            &root,
        );
        Self::at(dir, root, host)
    }

    fn at(dir: tempfile::TempDir, root: PathBuf, host: &str) -> Self {
        let main = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .unwrap();
        let stem = main.file_stem().unwrap().to_str().unwrap().to_string();
        let (vendor, product, session) = if stem.starts_with("rollout-") {
            (
                Vendor::Codex,
                Product::CodexCli,
                codex_id(&stem).to_string(),
            )
        } else {
            (Vendor::Claude, Product::ClaudeCli, stem.clone())
        };
        let mut subs: Vec<PathBuf> = fs::read_dir(root.join(&stem).join("subagents"))
            .map(|rd| {
                rd.map(|e| e.unwrap().path())
                    .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
                    .collect()
            })
            .unwrap_or_default();
        subs.sort();
        Self {
            _dir: dir,
            key: TeamKey {
                host: host.into(),
                vendor,
                session,
            },
            product,
            main,
            subs,
        }
    }

    fn files(&self, generation: u64) -> Vec<Vec<Event>> {
        std::iter::once(&self.main)
            .chain(&self.subs)
            .map(|p| parse_file(p, self.key.vendor, generation))
            .collect()
    }

    fn metas(&self) -> Vec<SubMeta> {
        self.subs.iter().filter_map(|p| sub_meta(p)).collect()
    }

    fn team(&self, generation: u64) -> Team {
        rebuild(
            &self.key.session,
            self.product,
            &self.metas(),
            self.files(generation),
            utc(),
        )
    }

    fn office(&self) -> String {
        office_id_str(&OfficeId::Team(self.key.clone()))
    }

    fn team_id(&self) -> String {
        team_id_str(&self.key)
    }
}

struct Env {
    _names: tempfile::TempDir,
    world: Mutex<OfficeWorld>,
}

fn env() -> Env {
    env_scale(Scale::Large)
}

fn env_scale(scale: Scale) -> Env {
    let names = tempfile::tempdir().unwrap();
    let mut w = OfficeWorld::new(scale, NameBook::load(names.path()), utc());
    for h in [windows_host(), wsl_host()] {
        w.hosts.insert(h.id.clone(), h);
    }
    Env {
        _names: names,
        world: Mutex::new(w),
    }
}

impl Env {
    fn put(&self, s: &Session, team: Team) {
        let now = team.last_activity;
        lock(&self.world).put_team(s.key.clone(), team, now, true);
    }

    fn timeline(
        &self,
        s: &Session,
        agent: &str,
        before: Option<CursorDto>,
        limit: Option<u32>,
    ) -> Result<Vec<EventDto>, DetailError> {
        employee_timeline(&self.world, &s.office(), &s.team_id(), agent, before, limit)
    }

    fn marks(&self, s: &Session, agent: &str) -> Vec<Mark> {
        let w = lock(&self.world);
        let t = &w.teams[&s.key];
        t.employees
            .iter()
            .find(|e| e.id == agent)
            .unwrap()
            .record
            .timeline
            .clone()
    }

    fn agents(&self, s: &Session) -> Vec<String> {
        lock(&self.world).teams[&s.key]
            .employees
            .iter()
            .map(|e| e.id.clone())
            .collect()
    }
}

const ASYNC: &str = "claude/async-subagent";
const ASYNC_SUB: &str = "a4fa9a3e07b34f715";

/// 페이지를 커서로 이으면 전체와 같고, 끝 다음은 빈 페이지다. 한 줄의 이벤트는 페이지 경계에서 갈리지 않는다
#[test]
fn timeline_pages_follow_cursor_to_the_end() {
    let s = Session::fixture(ASYNC, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let total = e.marks(&s, ASYNC_SUB).len();
    assert!(total > 30, "fixture 칸이 너무 적다: {total}");
    let all = e.timeline(&s, ASYNC_SUB, None, Some(200)).unwrap();
    assert_eq!(all.len(), total);
    // 최신부터: 한 파일이라 줄 위치가 거꾸로 간다
    assert!(all.windows(2).all(|p| p[0].line_no >= p[1].line_no));

    let first = e.timeline(&s, ASYNC_SUB, None, None).unwrap();
    assert_eq!(first.len(), total.min(50));
    assert_eq!(first[..], all[..first.len()]);

    let mut pages: Vec<Vec<EventDto>> = Vec::new();
    let mut before = None;
    loop {
        let page = e.timeline(&s, ASYNC_SUB, before, Some(7)).unwrap();
        let Some(last) = page.last() else { break };
        before = Some(last.cursor);
        pages.push(page);
    }
    assert!(pages.len() > 2);
    // 마지막 페이지만 7개보다 적을 수 있다 (줄을 안 가르려고 더 길어지는 것은 된다)
    assert!(pages[..pages.len() - 1].iter().all(|p| p.len() >= 7));
    let joined: Vec<EventDto> = pages.iter().flatten().cloned().collect();
    assert_eq!(joined, all);
    for p in pages.windows(2) {
        assert_ne!(p[0].last().unwrap().cursor, p[1][0].cursor, "줄이 갈렸다");
    }
}

/// 다시 읽은 줄에서 모든 칸이 제 이벤트를 찾는다 (같은 줄의 몇 번째·call 해시가 맞다)
#[test]
fn every_mark_finds_its_line_on_all_fixtures() {
    for rel in [
        ASYNC,
        "claude/sync-subagent",
        "claude/test-fail-fix-pass",
        "claude/parallel-reversed",
        "claude/bg-fail-reject-compact",
        "claude/compaction",
        "claude/reject",
        "codex/codex-app",
        "codex/ask-reject-compact",
        "codex/aborted",
        "claude/bg-fail-reject-compact",
        "claude/async-subagent",
    ] {
        let s = Session::fixture(rel, "windows");
        let e = env();
        e.put(&s, s.team(0));
        for agent in e.agents(&s) {
            let n = e.marks(&s, &agent).len();
            let all = e.timeline(&s, &agent, None, Some(200)).unwrap();
            assert_eq!(all.len(), n.min(200), "{rel} {agent}");
            for d in &all {
                assert!(
                    d.summary != MISSING && d.summary != UNREADABLE,
                    "{rel} {agent} {}행 {:?}",
                    d.line_no,
                    d.kind
                );
                if d.kind == TimelineKind::ToolCall {
                    assert!(d.call_id.is_some() && d.tool.is_some(), "{rel} {d:?}");
                }
            }
        }
    }
}

/// Reset 으로 세대가 바뀌면 옛 커서는 cursor_invalid 이고 첫 페이지부터 새 세대로 연다
#[test]
fn generation_change_invalidates_cursor() {
    let s = Session::fixture(ASYNC, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let old = e.timeline(&s, ASYNC_SUB, None, Some(5)).unwrap();
    let cursor = old.last().unwrap().cursor;
    assert!(e.timeline(&s, ASYNC_SUB, Some(cursor), Some(5)).is_ok());

    e.put(&s, s.team(1));
    assert_eq!(
        e.timeline(&s, ASYNC_SUB, Some(cursor), Some(5)),
        Err(DetailError::CursorInvalid)
    );
    let fresh = e.timeline(&s, ASYNC_SUB, None, Some(5)).unwrap();
    assert!(fresh.iter().all(|d| d.cursor.generation == 1));
    assert_eq!(
        fresh.iter().map(|d| &d.summary).collect::<Vec<_>>(),
        old.iter().map(|d| &d.summary).collect::<Vec<_>>()
    );
}

const SID: &str = "5e55d1a0-0000-4000-8000-00000000c1ab";

fn ts(secs: i64) -> String {
    let t: DateTime<Utc> = "2026-03-02T09:00:00Z".parse().unwrap();
    (t + Duration::seconds(secs)).to_rfc3339()
}

fn base(kind: &str, secs: i64) -> serde_json::Value {
    json!({"type": kind, "sessionId": SID, "timestamp": ts(secs), "cwd": "C:\\proj", "entrypoint": "cli"})
}

fn call_line(secs: i64, id: &str, command: &str) -> serde_json::Value {
    let mut v = base("assistant", secs);
    v["message"] = json!({"role": "assistant", "model": "claude-opus-5-5", "content": [
        {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}
    ]});
    v
}

fn result_line(secs: i64, id: &str, content: &str, persisted: Option<&str>) -> serde_json::Value {
    let mut v = base("user", secs);
    v["message"] = json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": id, "content": content, "is_error": false}
    ]});
    if let Some(p) = persisted {
        v["toolUseResult"] = json!({"stdout": "", "persistedOutputPath": p});
    }
    v
}

/// 합성 세션: 첫 사장님 메시지, 큰 결과 하나, 남의 세션·폴더 밖을 가리키는 결과 둘
fn synthetic() -> (Session, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(root.join(SID).join("tool-results")).unwrap();
    let big = format!(
        "{}{}{}",
        "A".repeat(3000),
        "가".repeat(1000),
        "Z".repeat(3000)
    );
    let mine = root.join(SID).join("tool-results").join("big.txt");
    let mut first = base("user", 0);
    first["message"] = json!({"role": "user", "content": "테스트 돌려 줘"});
    let lines = [
        first,
        call_line(1, "toolu_big", "cargo test"),
        result_line(2, "toolu_big", &big, Some(mine.to_str().unwrap())),
        call_line(3, "toolu_other", "ls"),
        result_line(
            4,
            "toolu_other",
            "x",
            Some("C:\\u\\.claude\\projects\\p\\0ther-5e55\\tool-results\\a.txt"),
        ),
        call_line(5, "toolu_up", "ls"),
        result_line(
            6,
            "toolu_up",
            "x",
            Some(&format!("C:\\p\\{SID}\\tool-results\\..")),
        ),
        call_line(7, "toolu_long", &"x".repeat(5000)),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(root.join(format!("{SID}.jsonl")), text).unwrap();
    (Session::at(dir, root, "windows"), big)
}

/// 결과는 앞뒤 2KB, 입력도 같다, 사이드카는 이 세션 폴더 안인 것만
#[test]
fn result_is_clipped_and_sidecar_stays_in_session() {
    let (s, big) = synthetic();
    let e = env();
    e.put(&s, s.team(0));
    let all = e.timeline(&s, SID, None, None).unwrap();
    let find = |id: &str, kind| {
        all.iter()
            .find(|d| d.call_id.as_deref() == Some(id) && d.kind == kind)
            .unwrap()
    };
    let result = find("toolu_big", TimelineKind::ToolResult);
    let body = result.body.as_ref().unwrap();
    assert!(!body.tail.is_empty());
    assert_eq!(body.bytes, big.len() as u64);
    assert!(body.head.len() <= 2048 && body.head.len() > 2040);
    assert!(body.tail.len() <= 2048 && body.tail.len() > 2040);
    assert!(big.starts_with(&body.head) && big.ends_with(&body.tail));
    assert_eq!(result.outcome, Some(ToolOutcome::Ok));
    let sidecar = s
        .main
        .with_extension("")
        .join("tool-results")
        .join("big.txt");
    assert_eq!(result.sidecar.as_deref(), Some(sidecar.to_str().unwrap()));
    for id in ["toolu_other", "toolu_up"] {
        let d = find(id, TimelineKind::ToolResult);
        assert_eq!(d.sidecar, None, "{id}");
        assert!(d.body.as_ref().unwrap().tail.is_empty());
    }
    let call = find("toolu_big", TimelineKind::ToolCall);
    assert_eq!(call.tool.as_deref(), Some("Bash"));
    assert!(call.input.as_ref().unwrap().head.contains("cargo test"));
    // 입력도 결과처럼 앞뒤 2KB
    let long = find("toolu_long", TimelineKind::ToolCall)
        .input
        .as_ref()
        .unwrap();
    assert_eq!(
        (long.head.len(), long.tail.len(), long.bytes > 5000),
        (2048, 2048, true)
    );
    assert!(call.summary.contains("cargo test"));
    assert_eq!(call.line_no, 2);
    assert_eq!(call.path, s.main.display().to_string());

    // 실제 발췌 fixture 의 `<persisted-output>` 본문 참조도 이 세션 폴더 아래로 다시 만든다
    let p = Session::fixture("claude/parallel-reversed", "windows");
    e.put(&p, p.team(0));
    let dir = p.main.with_extension("").join("tool-results");
    let found: Vec<String> = e
        .timeline(&p, &p.key.session, None, None)
        .unwrap()
        .into_iter()
        .filter_map(|d| d.sidecar)
        .collect();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .all(|f| Path::new(f).parent() == Some(dir.as_path()))
    );
}

/// 결과 파일 열기도 화면처럼 그 자리의 줄이 같은 기록일 때만 경로를 낸다
#[test]
fn sidecar_path_requires_same_record() {
    let (s, _) = synthetic();
    let e = env();
    e.put(&s, s.team(0));
    let all = e.timeline(&s, SID, None, None).unwrap();
    let d = all
        .iter()
        .find(|d| d.call_id.as_deref() == Some("toolu_big") && d.kind == TimelineKind::ToolResult)
        .unwrap();
    let w = match open(
        &e,
        OpenTarget::Sidecar {
            office_id: s.office(),
            team_id: s.team_id(),
            agent_id: SID.into(),
            cursor: d.cursor,
            line_idx: d.line_idx,
        },
    ) {
        Ok(Launch::Sidecar(w)) => w,
        other => panic!("{other:?}"),
    };
    let want = s
        .main
        .with_extension("")
        .join("tool-results")
        .join("big.txt");
    assert_eq!(sidecar_path(&w), Ok(want));

    let orig = fs::read_to_string(&s.main).unwrap();
    let rewrite = |from: &str, to: &str| {
        assert!(orig.contains(from), "{from}");
        fs::write(&s.main, orig.replacen(from, to, 1)).unwrap();
        sidecar_path(&w)
    };
    // 같은 오프셋에 시각이 다른 기록
    assert_eq!(rewrite(&ts(2), &ts(12)), Err(DetailError::CursorInvalid));
    // 같은 오프셋에 다른 호출의 결과
    assert_eq!(
        rewrite(
            "\"tool_use_id\":\"toolu_big\"",
            "\"tool_use_id\":\"toolu_xxx\""
        ),
        Err(DetailError::CursorInvalid)
    );
    // timestamp 가 없는 줄
    assert_eq!(
        rewrite(&format!("\"timestamp\":\"{}\"", ts(2)), "\"ts\":\"x\""),
        Err(DetailError::CursorInvalid)
    );
}

/// 받은 지시(서브는 Spawn 프롬프트, 부장은 첫 사장님 메시지)와 처음 본 순서의 실적 목록(명령은 횟수), 상사 이름
#[test]
fn employee_detail_has_instruction_and_lists() {
    let s = Session::fixture(ASYNC, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let files = s.files(0);
    let prompt = files[0]
        .iter()
        .find_map(|ev| match &ev.kind {
            EventKind::SubagentSpawn {
                child_id, prompt, ..
            } if child_id == ASYNC_SUB => Some(prompt.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!prompt.is_empty());
    let sub = employee_detail(&e.world, &s.office(), &s.team_id(), ASYNC_SUB).unwrap();
    assert_eq!(sub.instruction.as_deref(), Some(prompt.as_str()));
    let boss = employee_detail(&e.world, &s.office(), &s.team_id(), &s.key.session).unwrap();
    assert_eq!(sub.boss_name.as_deref(), Some(boss.name.as_str()));
    assert_eq!(boss.boss_name, None);
    // 이 발췌는 사장님 메시지 없이 시작한다. 부장의 지시는 메시지가 있는 fixture 로 본다
    let t = Session::fixture("claude/test-fail-fix-pass", "windows");
    e.put(&t, t.team(0));
    let first = t.files(0)[0].iter().find_map(|ev| match &ev.kind {
        EventKind::UserMessage { text } => Some(text.clone()),
        _ => None,
    });
    assert!(first.is_some());
    let boss = employee_detail(&e.world, &t.office(), &t.team_id(), &t.key.session).unwrap();
    assert_eq!(boss.instruction, first);

    let calls = || {
        files[1].iter().filter_map(|ev| match &ev.kind {
            EventKind::ToolCall { tool, summary, .. } => Some((tool, summary)),
            _ => None,
        })
    };
    let counted = |v: Vec<String>| {
        let mut out: Vec<CountDto> = Vec::new();
        for x in v {
            match out.iter_mut().find(|c| c.item == x) {
                Some(c) => c.times += 1,
                None => out.push(CountDto { item: x, times: 1 }),
            }
        }
        out
    };
    let reads: Vec<String> = counted(
        calls()
            .filter(|(t, _)| **t == ToolKind::FileRead)
            .filter_map(|(_, s)| s.path.clone())
            .collect(),
    )
    .into_iter()
    .map(|c| c.item)
    .collect();
    assert!(!reads.is_empty());
    assert_eq!(sub.files_read, reads);
    let commands = counted(
        calls()
            .filter_map(|(_, s)| s.command_norm.as_deref().map(|c| sentence::cut(c, 200)))
            .collect(),
    );
    assert!(!commands.is_empty());
    assert_eq!(sub.commands, commands);
}

/// 테스트 결과가 있는 결과 칸은 그 합격·반려 수를 싣고, 나머지 칸은 비운다
#[test]
fn result_event_carries_test_counts() {
    let s = Session::fixture("claude/test-fail-fix-pass", "windows");
    let e = env();
    e.put(&s, s.team(0));
    let tests: Vec<(String, TestDto)> = s
        .files(0)
        .into_iter()
        .flatten()
        .filter_map(|ev| match ev.kind {
            EventKind::ToolResult {
                call_id,
                test_result: Some(t),
                ..
            } => Some((
                call_id,
                TestDto {
                    passed: t.passed,
                    failed: t.failed,
                },
            )),
            _ => None,
        })
        .collect();
    assert!(
        tests.iter().any(|(_, t)| t.failed > 0),
        "실패한 테스트가 없다"
    );
    let all = e.timeline(&s, &s.key.session, None, Some(200)).unwrap();
    let mut seen = 0;
    for d in &all {
        let want = (d.kind == TimelineKind::ToolResult)
            .then(|| tests.iter().find(|(id, _)| Some(id) == d.call_id.as_ref()))
            .flatten()
            .map(|(_, t)| *t);
        assert_eq!(d.test, want, "{}행", d.line_no);
        seen += usize::from(want.is_some());
    }
    assert_eq!(seen, tests.len());
}

#[test]
fn session_detail_values() {
    for rel in [ASYNC, "claude/bg-fail-reject-compact"] {
        let s = Session::fixture(rel, "windows");
        let e = env();
        let team = s.team(0);
        let (cwd, model, branch) = (
            team.cwd.display().to_string(),
            team.employees[0].model.clone(),
            team.git_branch.clone(),
        );
        e.put(&s, team);
        let d = session_detail(&e.world, &s.office(), &s.team_id(), None).unwrap();
        let files = s.files(0);
        let turns = files[0]
            .iter()
            .filter(|ev| matches!(ev.kind, EventKind::UserMessage { .. }))
            .count();
        let (input, output) = files
            .iter()
            .flatten()
            .fold((0, 0), |acc, ev| match ev.kind {
                EventKind::TokenUsage {
                    input_total,
                    cache_read,
                    output,
                    ..
                } => (acc.0 + input_total - cache_read, acc.1 + output),
                _ => acc,
            });
        assert_eq!(d.session_id, s.key.session);
        assert!(!cwd.is_empty() && model.is_some(), "{rel}");
        assert_eq!((d.cwd, d.model, d.git_branch), (cwd, model, branch));
        assert_eq!(d.turns as usize, turns, "{rel}");
        assert!(input > 0, "{rel}");
        assert_eq!(d.tokens, Some(Tokens { input, output }), "{rel}");
        assert_eq!(d.recent_error, None);
        assert!(d.recent.iter().all(|r| r.summary != MISSING), "{rel}");
        // 최근 20개는 직원들의 칸을 (ts, seq) 로 합친 것의 머리
        let mut all: Vec<Mark> = e.agents(&s).iter().flat_map(|a| e.marks(&s, a)).collect();
        all.sort_by_key(|m| std::cmp::Reverse((m.ts, m.seq)));
        all.truncate(20);
        assert!(all.len() > 10, "{rel}");
        assert_eq!(d.recent.len(), all.len(), "{rel}");
        let got: Vec<(u64, u64)> = d
            .recent
            .iter()
            .map(|r| (r.cursor.byte_offset, r.line_no))
            .collect();
        let want: Vec<(u64, u64)> = all.iter().map(|m| (m.byte_offset, m.line_no)).collect();
        assert_eq!(got, want, "{rel}");
        if rel == ASYNC {
            let h = d.headcount;
            assert_eq!((h.bujang, h.gwajang + h.daeri + h.sawon), (1, 1));
            assert!(d.recent.iter().any(|r| r.agent_id == ASYNC_SUB));
        } else {
            assert!(turns > 1);
        }
    }
}

type Refs = Vec<(String, Vec<(PathBuf, u64, u64, u64)>)>;

fn raw_refs(t: &Team) -> Refs {
    t.employees
        .iter()
        .map(|e| {
            let refs = e
                .record
                .timeline
                .iter()
                .map(|m| {
                    (
                        t.sources[m.source as usize].clone(),
                        m.generation,
                        m.byte_offset,
                        m.line_no,
                    )
                })
                .collect();
            (e.id.clone(), refs)
        })
        .collect()
}

/// 앞부분으로 만든 팀에 나머지를 실행 중처럼 몇 줄씩 적용해도 한 번에 다시 만든 것과 같은 타임라인이다
#[test]
fn rebuild_and_live_append_give_same_timeline() {
    for rel in [ASYNC, "codex/ask-reject-compact"] {
        let s = Session::fixture(rel, "windows");
        let files = s.files(0);
        let whole = s.team(0);
        let events = merge(files.clone());
        let cut = events.len() / 2;
        let head: Vec<Vec<Event>> = files
            .iter()
            .map(|f| {
                f.iter()
                    .filter(|ev| events[..cut].contains(ev))
                    .cloned()
                    .collect()
            })
            .collect();
        let e = env();
        e.put(
            &s,
            rebuild(&s.key.session, s.product, &s.metas(), head, utc()),
        );
        for chunk in events[cut..].chunks(3) {
            let now = chunk.iter().filter_map(|ev| ev.ts).max().unwrap();
            lock(&e.world).apply(&s.key, chunk, now);
        }
        let w = lock(&e.world);
        let live = &w.teams[&s.key];
        assert!(raw_refs(&whole).iter().any(|(_, r)| r.len() > 10), "{rel}");
        assert_eq!(raw_refs(live), raw_refs(&whole), "{rel}");
        assert_eq!(
            (live.turns, live.tokens),
            (whole.turns, whole.tokens),
            "{rel}"
        );
        for (a, b) in live.employees.iter().zip(&whole.employees) {
            assert_eq!(
                (&a.record.prompt, &a.record.files_read, &a.record.commands),
                (&b.record.prompt, &b.record.files_read, &b.record.commands),
                "{rel}"
            );
        }
    }
}

fn open(e: &Env, t: OpenTarget) -> Result<Launch, DetailError> {
    resolve_open(&e.world, t).map(|p| p.0)
}

/// 모르는 사무실·팀·직원·칸과 믿을 수 없는 작업 폴더는 거부한다. 아는 대상은 백엔드가 아는 경로로만
#[test]
fn open_path_resolves_only_known_targets() {
    let s = Session::fixture(ASYNC, "windows");
    let e = env();
    let mut team = s.team(0);
    team.cwd = PathBuf::from(r"C:\proj");
    e.put(&s, team);
    let (office, team_id) = (s.office(), s.team_id());
    let source = |office: &str, team: &str| OpenTarget::SourceFolder {
        office_id: office.into(),
        team_id: team.into(),
    };
    let work = || OpenTarget::WorkFolder {
        office_id: office.clone(),
        team_id: team_id.clone(),
    };
    assert_eq!(
        open(&e, source(&office, &team_id)),
        Ok(Launch::Select(s.main.clone()))
    );
    assert_eq!(
        open(&e, work()),
        Ok(Launch::Folder(PathBuf::from(r"C:\proj")))
    );
    assert_eq!(
        open(&e, source("garbage", &team_id)),
        Err(DetailError::NotFound)
    );
    assert_eq!(
        open(&e, source(&office, "team/claude/없음/windows")),
        Err(DetailError::NotFound)
    );
    assert_eq!(
        open(&e, source(&office, r"C:\Windows")),
        Err(DetailError::NotFound)
    );
    let cursor = e.timeline(&s, ASYNC_SUB, None, Some(1)).unwrap()[0].cursor;
    let line = |agent: &str, cursor, line_idx| OpenTarget::SourceLine {
        office_id: office.clone(),
        team_id: team_id.clone(),
        agent_id: agent.into(),
        cursor,
        line_idx,
    };
    assert_eq!(
        open(&e, line(ASYNC_SUB, cursor, 0)),
        Ok(Launch::Select(s.subs[0].clone()))
    );
    assert_eq!(
        open(&e, line("없는직원", cursor, 0)),
        Err(DetailError::NotFound)
    );
    let bogus = CursorDto {
        generation: 9,
        byte_offset: 1,
    };
    assert_eq!(
        open(&e, line(ASYNC_SUB, bogus, 0)),
        Err(DetailError::CursorInvalid)
    );
    // 사이드카는 결과 칸에서만
    let message = e
        .timeline(&s, ASYNC_SUB, None, Some(200))
        .unwrap()
        .into_iter()
        .find(|d| d.kind == TimelineKind::UserMessage)
        .unwrap();
    assert_eq!(
        open(
            &e,
            OpenTarget::Sidecar {
                office_id: office.clone(),
                team_id: team_id.clone(),
                agent_id: ASYNC_SUB.into(),
                cursor: message.cursor,
                line_idx: message.line_idx,
            }
        ),
        Err(DetailError::CursorInvalid)
    );
    // 기록에 적힌 작업 폴더가 UNC·상대 경로·상위 이동이면 열지 않는다
    for bad in [
        r"\\evil\share\x",
        r"proj\x",
        r"C:\proj\..\Windows",
        r"C:proj",
        r"C:",
        r"C:.",
    ] {
        lock(&e.world).teams.get_mut(&s.key).unwrap().cwd = PathBuf::from(bad);
        assert_eq!(open(&e, work()), Err(DetailError::OpenFailed), "{bad}");
    }
}

/// PR 은 그 팀이 아는 https 주소만 연다. 다른 스킴·인자 경계를 깨는 글자는 탐색기에 닿기 전에 거절한다
#[test]
fn pr_target_opens_only_safe_https() {
    let s = Session::fixture(ASYNC, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let pr = || OpenTarget::Pr {
        office_id: s.office(),
        team_id: s.team_id(),
    };
    let set = |url: &str| {
        lock(&e.world).teams.get_mut(&s.key).unwrap().props.pr = Some((142, url.into()));
    };
    lock(&e.world).teams.get_mut(&s.key).unwrap().props.pr = None;
    assert_eq!(open(&e, pr()), Err(DetailError::NotFound));
    let long = format!("https://github.com/{}", "a".repeat(2048 - 19));
    for good in [
        "https://github.com/x/y/pull/142",
        "https://github.com/x/y%20z/pull/142",
        long.as_str(),
    ] {
        set(good);
        assert_eq!(open(&e, pr()), Ok(Launch::Url(good.into())), "{good:?}");
    }
    let too_long = format!("{long}a");
    for bad in [
        "http://github.com/x/y/pull/142",
        "file:///C:/Windows/System32/calc.exe",
        r"C:\Windows\System32\calc.exe",
        "https://",
        "HTTPS://github.com/x",
        "https://github.com/x\"y",
        "https://github.com/x /y",
        "https://github.com/x\t/y",
        "https://github.com/x\n/y",
        "https://github.com/x\u{7f}y",
        r"https://github.com/x,C:\Windows\calc.exe",
        r"https://github.com\..\x",
        "https://github.com/x\u{202e}y",
        "https://github.com/x\u{200b}y",
        "https://github.com/\u{feff}x",
        "https://깃허브.com/x",
        too_long.as_str(),
    ] {
        set(bad);
        assert_eq!(open(&e, pr()), Err(DetailError::OpenFailed), "{bad:?}");
    }
    assert_eq!(
        open(
            &e,
            OpenTarget::Pr {
                office_id: s.office(),
                team_id: "team/claude/없음/windows".into(),
            }
        ),
        Err(DetailError::NotFound)
    );
}

/// WSL 작업 폴더는 세션 파일이 있는 배포판 공유 아래로 옮긴다
#[test]
fn wsl_work_folder_maps_under_distro_share() {
    let main = Path::new(r"\\wsl.localhost\Test\home\u\.claude\projects\p\s.jsonl");
    let wsl = wsl_host().kind;
    assert_eq!(
        work_dir(Path::new("/home/u/proj"), &wsl, Some(main)),
        Some(PathBuf::from(r"\\wsl.localhost\Test\home\u\proj"))
    );
    assert_eq!(
        work_dir(Path::new("/"), &wsl, Some(main)),
        Some(PathBuf::from(r"\\wsl.localhost\Test\"))
    );
    // `C:` 조각은 PathBuf::join 이 앞 경로를 통째로 바꾼다
    for bad in [
        "home/u",
        "/home/../etc",
        r"/home\u",
        "/C:/Windows",
        "/home/C:",
        "/home/u/D:x",
    ] {
        assert_eq!(work_dir(Path::new(bad), &wsl, Some(main)), None, "{bad}");
    }
    // 세션 파일이 WSL 공유가 아니면 옮길 곳이 없다
    let elsewhere = Path::new(r"\\evil\share\s.jsonl");
    assert_eq!(work_dir(Path::new("/home/u"), &wsl, Some(elsewhere)), None);
}

/// 끝의 `\` 는 닫는 따옴표를 이스케이프하므로 루트도 열리려면 두 배여야 한다
#[test]
fn explorer_arg_keeps_closing_quote_for_roots() {
    assert_eq!(explorer_arg(r"C:\", false), r#""C:\\""#);
    assert_eq!(
        explorer_arg(r"\\wsl.localhost\Test\", false),
        r#""\\wsl.localhost\Test\\""#
    );
    assert_eq!(explorer_arg(r"C:\a b", false), r#""C:\a b""#);
    assert_eq!(
        explorer_arg(r"C:\a b\c.txt", true),
        r#"/select,"C:\a b\c.txt""#
    );
}

/// 기한이 지난 뒤에는 폴더가 멀쩡해도 탐색기를 띄우지 않는다
#[test]
fn launch_after_deadline_does_not_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let past = Instant::now() - StdDuration::from_secs(1);
    assert_eq!(
        launch(Launch::Folder(dir.path().to_path_buf()), past),
        Err(DetailError::Timeout)
    );
}

fn set_phase(e: &Env, s: &Session, phase: TeamPhase) {
    lock(&e.world).teams.get_mut(&s.key).unwrap().phase = phase;
}

/// 꺼졌거나 통신 지연인 배포판, 근무 중이 아닌 팀(핸들이 없다)은 파일을 건드리기 전에 host_offline.
/// 세션 상세는 메모리 값을 그대로 주고 `recent_error` 로 알린다
#[test]
fn wsl_files_are_touched_only_for_working_teams() {
    let s = Session::fixture(ASYNC, "wsl:Test");
    let e = env();
    e.put(&s, s.team(0));
    set_phase(&e, &s, TeamPhase::Working);
    let n = e.marks(&s, ASYNC_SUB).len();
    assert_eq!(e.timeline(&s, ASYNC_SUB, None, None).unwrap().len(), n);
    let ok = session_detail(&e.world, &s.office(), &s.team_id(), None).unwrap();
    assert_eq!((ok.recent.len(), ok.recent_error), (20, None));

    // 파일을 지워 두면 건드렸을 때 "못 읽음" 칸이 나온다. 그 전에 멈춰야 한다
    fs::remove_file(&s.subs[0]).unwrap();
    fs::remove_file(&s.main).unwrap();
    for suspect in [false, true] {
        set_phase(
            &e,
            &s,
            if suspect {
                TeamPhase::Working
            } else {
                TeamPhase::Dozing
            },
        );
        if suspect {
            lock(&e.world).suspect_hosts.insert("wsl:Test".into());
        }
        assert_eq!(
            e.timeline(&s, ASYNC_SUB, None, None),
            Err(DetailError::HostOffline)
        );
        let d = session_detail(&e.world, &s.office(), &s.team_id(), None).unwrap();
        assert!(d.recent.is_empty());
        assert_eq!(d.recent_error, Some(DetailError::HostOffline));
        assert_eq!((d.tokens, d.turns), (ok.tokens, ok.turns));
        for t in [
            OpenTarget::SourceFolder {
                office_id: s.office(),
                team_id: s.team_id(),
            },
            OpenTarget::WorkFolder {
                office_id: s.office(),
                team_id: s.team_id(),
            },
        ] {
            assert_eq!(open(&e, t), Err(DetailError::HostOffline));
        }
        // 메모리에 있는 직원 상세는 호스트와 상관없다
        assert!(employee_detail(&e.world, &s.office(), &s.team_id(), ASYNC_SUB).is_ok());
    }

    set_phase(&e, &s, TeamPhase::Working);
    lock(&e.world).suspect_hosts.clear();
    let page = e.timeline(&s, ASYNC_SUB, None, None).unwrap();
    assert!(page.iter().all(|d| d.summary == UNREADABLE));
}

/// 상한을 넘긴 읽기는 그 WSL 호스트를 의심으로 돌리고, 그 스레드가 끝날 때까지 새 읽기를 막는다
#[test]
fn stalled_read_blocks_new_reads_until_it_ends() {
    let host = Host {
        id: "wsl:Stuck".into(),
        kind: HostKind::Wsl {
            distro: "Stuck".into(),
        },
        label: "Stuck".into(),
    };
    let s = Session::fixture(ASYNC, &host.id);
    let e = env();
    lock(&e.world).hosts.insert(host.id.clone(), host);
    e.put(&s, s.team(0));
    set_phase(&e, &s, TeamPhase::Working);
    assert!(e.timeline(&s, ASYNC_SUB, None, None).is_ok());

    let (release, gate) = mpsc::channel::<()>();
    let r = bounded(&e.world, Some("wsl:Stuck".into()), move |_| {
        let _ = gate.recv();
        Ok(())
    });
    assert_eq!(r, Err(DetailError::Timeout));
    assert!(lock(&e.world).suspect_hosts.contains("wsl:Stuck"));
    // Probe 가 의심을 풀어도(켜져 있으니까) 걸린 스레드가 살아 있는 동안은 막힌다
    lock(&e.world).set_suspect("wsl:Stuck", false);
    assert_eq!(
        e.timeline(&s, ASYNC_SUB, None, None),
        Err(DetailError::HostOffline)
    );
    assert_eq!(
        open(
            &e,
            OpenTarget::SourceFolder {
                office_id: s.office(),
                team_id: s.team_id()
            }
        ),
        Err(DetailError::HostOffline)
    );

    drop(release);
    let end = Instant::now() + StdDuration::from_secs(5);
    while hung("wsl:Stuck") {
        assert!(Instant::now() < end, "걸린 스레드가 끝나지 않았다");
        thread::sleep(StdDuration::from_millis(10));
    }
    assert!(e.timeline(&s, ASYNC_SUB, None, None).is_ok());
}

/// 비교용 복제가 기록을 쥐지 않으니 이벤트를 적용해도 타임라인 전체가 복사되지 않는다 (같은 Arc 에 쓴다)
#[test]
fn apply_does_not_copy_the_record() {
    let s = Session::fixture(ASYNC, "windows");
    let events = merge(s.files(0));
    let cut = events.len() / 2;
    let boss = s.key.session.clone();
    let head: Vec<Vec<Event>> = s
        .files(0)
        .iter()
        .map(|f| {
            f.iter()
                .filter(|ev| events[..cut].contains(ev))
                .cloned()
                .collect()
        })
        .collect();
    let e = env();
    e.put(
        &s,
        rebuild(&s.key.session, s.product, &s.metas(), head, utc()),
    );
    let ptr = || Arc::as_ptr(&lock(&e.world).teams[&s.key].employees[0].record);
    let before = ptr();
    let len = e.marks(&s, &boss).len();
    let mut applied = 0;
    for ev in events[cut..].iter().filter(|ev| ev.agent_id == boss) {
        let now = ev.ts.unwrap_or_default();
        lock(&e.world).apply(&s.key, std::slice::from_ref(ev), now);
        assert_eq!(ptr(), before, "기록이 복사되었다");
        applied += 1;
    }
    assert!(applied > 3 && e.marks(&s, &boss).len() > len);
}

/// 파일이 바뀌어 같은 자리에 다른 줄이 있으면(ts 가 다르다) 다른 줄을 보여 주지 않는다
#[test]
fn changed_file_is_unreadable_not_another_line() {
    let (s, _) = synthetic();
    let e = env();
    e.put(&s, s.team(0));
    let fresh = e.timeline(&s, SID, None, None).unwrap();
    assert!(fresh.iter().all(|d| d.summary != UNREADABLE));
    // 같은 줄 길이로 시각만 밀린 파일 (재구성 전)
    let text = fs::read_to_string(&s.main)
        .unwrap()
        .replace("T09:00:0", "T09:01:0");
    fs::write(&s.main, text).unwrap();
    let stale = e.timeline(&s, SID, None, None).unwrap();
    assert_eq!(stale.len(), fresh.len());
    assert!(stale.iter().all(|d| d.summary == UNREADABLE), "{stale:?}");
}

/// 한 줄에 호출·결과가 여럿이면 줄 안 순번이 칸을 가른다 (사이드카는 그 칸의 결과를 연다)
#[test]
fn line_idx_tells_apart_entries_of_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(&root).unwrap();
    let mut calls = base("assistant", 1);
    calls["message"] = json!({"role": "assistant", "model": "claude-opus-5-5", "content": [
        {"type": "tool_use", "id": "toolu_a", "name": "Bash", "input": {"command": "ls a"}},
        {"type": "tool_use", "id": "toolu_b", "name": "Bash", "input": {"command": "ls b"}},
    ]});
    let big = |name: &str| {
        format!(
            "<persisted-output>\nOutput saved to C:\\u\\p\\{SID}\\tool-results\\{name}\n</persisted-output>"
        )
    };
    let mut results = base("user", 2);
    results["message"] = json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_a", "content": big("a.txt"), "is_error": false},
        {"type": "tool_result", "tool_use_id": "toolu_b", "content": big("b.txt"), "is_error": false},
    ]});
    let text: String = [calls, results].iter().map(|l| format!("{l}\n")).collect();
    fs::write(root.join(format!("{SID}.jsonl")), text).unwrap();
    let s = Session::at(dir, root, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let all = e.timeline(&s, SID, None, None).unwrap();
    let pick = |id: &str, kind| {
        all.iter()
            .find(|d| d.call_id.as_deref() == Some(id) && d.kind == kind)
            .unwrap()
    };
    let (a, b) = (
        pick("toolu_a", TimelineKind::ToolResult),
        pick("toolu_b", TimelineKind::ToolResult),
    );
    assert_eq!(a.cursor, b.cursor);
    assert_eq!((a.line_idx, b.line_idx), (0, 1));
    assert_eq!(pick("toolu_b", TimelineKind::ToolCall).line_idx, 1);
    let sidecar = |d: &EventDto, line_idx| {
        open(
            &e,
            OpenTarget::Sidecar {
                office_id: s.office(),
                team_id: s.team_id(),
                agent_id: SID.into(),
                cursor: d.cursor,
                line_idx,
            },
        )
    };
    let call_of = |l: Result<Launch, DetailError>| match l {
        Ok(Launch::Sidecar(w)) => w.mark.call,
        other => panic!("{other:?}"),
    };
    assert_eq!(call_of(sidecar(a, a.line_idx)), call_hash("toolu_a"));
    assert_eq!(call_of(sidecar(b, b.line_idx)), call_hash("toolu_b"));
    assert_eq!(sidecar(a, 7), Err(DetailError::CursorInvalid));
    // 호출 칸은 결과가 아니다
    let call = pick("toolu_a", TimelineKind::ToolCall);
    assert_eq!(
        sidecar(call, call.line_idx),
        Err(DetailError::CursorInvalid)
    );
}

/// 중소기업은 한 사무실에 팀이 여럿이라 같은 직원 id 가 둘 있을 수 있다. 팀 id 로 가른다
#[test]
fn employee_commands_pick_the_team_by_id() {
    let a = Session::fixture(ASYNC, "windows");
    let b = Session::fixture(ASYNC, "wsl:Test");
    let e = env_scale(Scale::Small);
    e.put(&a, a.team(0));
    // 이름은 직원 id 마다 정해져 팀이 달라도 같다. 모델로 가른다
    let mut tb = b.team(0);
    for emp in &mut tb.employees {
        emp.model = Some("other-model".into());
    }
    e.put(&b, tb);
    set_phase(&e, &b, TeamPhase::Working);
    let office = office_id_str(&OfficeId::Vendor(Vendor::Claude));
    let model = |s: &Session| {
        employee_detail(&e.world, &office, &s.team_id(), ASYNC_SUB)
            .unwrap()
            .model
    };
    assert_ne!(model(&a), model(&b));
    assert_eq!(model(&b).as_deref(), Some("other-model"));
    for s in [&a, &b] {
        let page =
            employee_timeline(&e.world, &office, &s.team_id(), ASYNC_SUB, None, Some(3)).unwrap();
        assert!(
            page.iter()
                .all(|d| Path::new(&d.path).starts_with(s.main.parent().unwrap()))
        );
    }
    // 다른 사무실의 팀 id 는 이 사무실에 없다
    let other = office_id_str(&OfficeId::Vendor(Vendor::Codex));
    assert_eq!(
        employee_detail(&e.world, &other, &a.team_id(), ASYNC_SUB),
        Err(DetailError::NotFound)
    );
}

/// 최근 사건은 직원마다 끝에서 뽑아 적용 순서(seq)로 합치고, limit 은 1~100 으로 맞춘다.
/// 늦게 읽힌 줄의 적용 순서가 시각 순서와 달라도 적용 순서를 따른다
#[test]
fn session_recent_orders_by_seq_and_honors_limit() {
    let s = Session::fixture(ASYNC, "windows");
    let events = merge(s.files(0));
    let cut = events.len() / 3;
    let head: Vec<Vec<Event>> = s
        .files(0)
        .iter()
        .map(|f| {
            f.iter()
                .filter(|ev| events[..cut].contains(ev))
                .cloned()
                .collect()
        })
        .collect();
    let e = env();
    e.put(
        &s,
        rebuild(&s.key.session, s.product, &s.metas(), head, utc()),
    );
    // 서브 파일이 부장 파일보다 늦게 읽힌 것처럼 적용한다
    let (boss, subs): (Vec<&Event>, Vec<&Event>) = events[cut..]
        .iter()
        .partition(|ev| ev.agent_id == s.key.session);
    for ev in boss.into_iter().chain(subs) {
        let now = ev.ts.unwrap_or_default();
        lock(&e.world).apply(&s.key, std::slice::from_ref(ev), now);
    }
    let all: Vec<Mark> = {
        let mut v: Vec<Mark> = e.agents(&s).iter().flat_map(|a| e.marks(&s, a)).collect();
        v.sort_by_key(|m| std::cmp::Reverse(m.seq));
        v
    };
    let mut by_ts = all.clone();
    by_ts.sort_by_key(|m| std::cmp::Reverse((m.ts, m.seq)));
    assert_ne!(
        all[..20].iter().map(|m| m.seq).collect::<Vec<_>>(),
        by_ts[..20].iter().map(|m| m.seq).collect::<Vec<_>>(),
        "fixture 가 시각과 적용 순서를 어긋나게 하지 못했다"
    );
    let recent = |limit| {
        session_detail(&e.world, &s.office(), &s.team_id(), limit)
            .unwrap()
            .recent
    };
    let key = |r: &[EventDto]| {
        r.iter()
            .map(|d| (d.cursor.byte_offset, d.line_no))
            .collect::<Vec<_>>()
    };
    let want = |n: usize| {
        all[..n]
            .iter()
            .map(|m| (m.byte_offset, m.line_no))
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&recent(None)), want(20));
    assert_eq!(key(&recent(Some(5))), want(5));
    assert_eq!(recent(Some(0)).len(), 1);
    assert_eq!(key(&recent(Some(1000))), want(all.len().min(100)));
}

/// 토큰은 캐시에서 읽은 입력을 세지 않는다 (응답마다 다시 읽혀 매번 다시 세진다)
#[test]
fn tokens_leave_out_cache_reads() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(&root).unwrap();
    let reply = |secs, id: &str| {
        let mut v = base("assistant", secs);
        v["message"] = json!({"id": id, "role": "assistant", "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "ok"}],
            "usage": {"input_tokens": 10, "cache_creation_input_tokens": 20,
                      "cache_read_input_tokens": 1000, "output_tokens": 5}});
        v
    };
    let text: String = [reply(1, "msg_1"), reply(2, "msg_2")]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(root.join(format!("{SID}.jsonl")), text).unwrap();
    let s = Session::at(dir, root, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let d = session_detail(&e.world, &s.office(), &s.team_id(), None).unwrap();
    assert_eq!(
        d.tokens,
        Some(Tokens {
            input: 60,
            output: 10
        })
    );

    // Codex: input_tokens 는 cached 를 포함한다. 마지막 누적은 입력 163551 중 144384 가 캐시, 출력 768
    let c = Session::fixture("codex/ask-reject-compact", "windows");
    e.put(&c, c.team(0));
    let d = session_detail(&e.world, &c.office(), &c.team_id(), None).unwrap();
    assert_eq!(
        d.tokens,
        Some(Tokens {
            input: 163_551 - 144_384,
            output: 768
        })
    );
}

/// 호출과 결과가 다른 페이지에 있어도 결과 칸은 한 번에 읽은 것과 같다.
/// Codex 파서는 짝 호출을 본 적 없는 결과에 이벤트를 내지 않는다
#[test]
fn result_paged_apart_from_its_call_reads_the_same() {
    let mut bodies = 0;
    for rel in [
        "codex/codex-app",
        "codex/ask-reject-compact",
        "codex/aborted",
        "claude/bg-fail-reject-compact",
        "claude/async-subagent",
    ] {
        let s = Session::fixture(rel, "windows");
        let e = env();
        e.put(&s, s.team(0));
        for agent in e.agents(&s) {
            let all = e.timeline(&s, &agent, None, Some(200)).unwrap();
            let mut one_by_one = Vec::new();
            let mut before = None;
            loop {
                let page = e.timeline(&s, &agent, before, Some(1)).unwrap();
                let Some(last) = page.last() else { break };
                before = Some(last.cursor);
                one_by_one.extend(page);
            }
            assert_eq!(one_by_one, all, "{rel} {agent}");
            bodies += all
                .iter()
                .filter(|d| d.kind == TimelineKind::ToolResult && d.body.is_some())
                .count();
        }
    }
    assert!(bodies > 0, "본문 있는 결과가 없어 비교가 비었다");
}

/// 백그라운드 완료 결과는 다른 줄(Started 결과)의 파서 상태가 있어야 이벤트가 난다.
/// 한 줄씩 페이징해도 결과 칸은 "못 읽음"이 아니다
#[test]
fn background_codex_result_paged_alone_is_still_readable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(&root).unwrap();
    let ts = |s: u32| format!("2026-03-02T09:00:{s:02}Z");
    let rec = |s: u32, ty: &str, payload: serde_json::Value| json!({"timestamp": ts(s), "type": ty, "payload": payload});
    let lines = [
        rec(
            0,
            "session_meta",
            json!({"id": "fbeee44f-138a-8f82-87fd-61f01dfc0cca", "cwd": "/w",
                   "originator": "Codex Desktop", "source": "vscode"}),
        ),
        rec(
            1,
            "response_item",
            json!({"type": "custom_tool_call", "name": "exec", "call_id": "bg",
                   "input": "tools.exec_command({cmd:\"npm run dev\"})"}),
        ),
        rec(
            2,
            "response_item",
            json!({"type": "custom_tool_call_output", "call_id": "bg", "output": [
                {"type": "input_text", "text": "Script completed
"},
                {"type": "input_text", "text": "{\"session_id\":4242,\"output\":\"\"}"}]}),
        ),
        rec(
            3,
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "CommandExecution", "id": "exec-x", "process_id": "4242",
                "command": ["bash", "-lc", "x"], "status": "completed", "exit_code": 0}}),
        ),
    ];
    let name = "rollout-2026-03-02T09-00-00-fbeee44f-138a-8f82-87fd-61f01dfc0cca.jsonl";
    fs::write(
        root.join(name),
        lines
            .iter()
            .map(|l| {
                format!(
                    "{l}
"
                )
            })
            .collect::<String>(),
    )
    .unwrap();
    let s = Session::at(dir, root, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let agent = &e.agents(&s)[0];
    let all = e.timeline(&s, agent, None, Some(200)).unwrap();
    let mut results = Vec::new();
    let mut before = None;
    loop {
        let page = e.timeline(&s, agent, before, Some(1)).unwrap();
        let Some(last) = page.last() else { break };
        before = Some(last.cursor);
        results.extend(
            page.into_iter()
                .filter(|d| d.kind == TimelineKind::ToolResult),
        );
    }
    assert_eq!(results.len(), 2, "{results:?}");
    let whole: Vec<_> = all
        .into_iter()
        .filter(|d| d.kind == TimelineKind::ToolResult)
        .collect();
    assert_eq!(results, whole);
    for r in &results {
        assert_ne!(r.summary, UNREADABLE, "{r:?}");
    }
}

#[test]
fn codex_followup_spawn_paged_alone_requires_matching_raw_activity() {
    const CHILD: &str = "00000000-0000-4000-8000-0000000000bb";
    for direct in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        fs::create_dir_all(&root).unwrap();
        let rec = |s: u32, ty: &str, payload: serde_json::Value| json!({"timestamp":format!("2026-03-02T09:00:{s:02}Z"),"type":ty,"payload":payload});
        let activity = |s: u32, id: &str, kind: &str| {
            let item = json!({"type":"SubAgentActivity","id":id,"kind":kind,
                "agent_thread_id":CHILD,"agent_path":"/root/w"});
            if direct {
                rec(
                    s,
                    "event_msg",
                    json!({"type":"sub_agent_activity","id":id,"kind":kind,
                    "agent_thread_id":CHILD,"agent_path":"/root/w"}),
                )
            } else {
                rec(s, "event_msg", json!({"type":"item_completed","item":item}))
            }
        };
        let mut lines = vec![
            rec(
                0,
                "session_meta",
                json!({"id":"fbeee44f-138a-8f82-87fd-61f01dfc0cca",
                "cwd":"/w","originator":"Codex Desktop","source":"vscode"}),
            ),
            activity(1, "spawn", "started"),
            activity(2, "stop", "interrupted"),
            rec(
                3,
                "response_item",
                json!({"type":"function_call","name":"followup_task",
                "call_id":"follow","arguments":json!({"target":"/root/w","message":"m"}).to_string()}),
            ),
            activity(4, "follow", "interacted"),
        ];
        let name = "rollout-2026-03-02T09-00-00-fbeee44f-138a-8f82-87fd-61f01dfc0cca.jsonl";
        let path = root.join(name);
        let write = |rows: &[serde_json::Value]| {
            fs::write(
                &path,
                rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
            )
            .unwrap();
        };
        write(&lines);
        let s = Session::at(dir, root, "windows");
        let e = env();
        e.put(&s, s.team(0));
        let agent = &e.agents(&s)[0];
        let page = || e.timeline(&s, agent, None, Some(1)).unwrap();
        assert_eq!(page()[0].kind, TimelineKind::SubagentSpawn);
        assert_eq!(page()[0].summary, "작업 재개");

        let original = lines[4].clone();
        lines[4]["timestamp"] = json!("2026-03-02T09:00:05Z");
        write(&lines);
        assert_eq!(page()[0].summary, UNREADABLE);
        lines[4] = original.clone();
        let item = if direct {
            &mut lines[4]["payload"]
        } else {
            &mut lines[4]["payload"]["item"]
        };
        item["kind"] = json!("completed");
        write(&lines);
        assert_eq!(page()[0].summary, UNREADABLE);
        lines[4] = original.clone();
        let item = if direct {
            &mut lines[4]["payload"]
        } else {
            &mut lines[4]["payload"]["item"]
        };
        item["agent_thread_id"] = json!("invalid");
        write(&lines);
        assert_eq!(page()[0].summary, UNREADABLE);
        lines[4] = original;
        lines[4]["payload"]["type"] = json!("different");
        write(&lines);
        assert_eq!(page()[0].summary, UNREADABLE);
    }
}

/// TaskStop 결과는 TaskStop 호출 줄(다른 call)의 상태가 있어야 이벤트가 나서, 짝 줄 문맥으로도 못 읽는다.
/// 한 줄씩 페이징해도 결과 칸은 요약을 원문 대체로 채운다
#[test]
fn task_stop_result_paged_alone_falls_back_to_raw() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(&root).unwrap();
    let mut start = call_line(1, "toolu_bg", "npm run dev");
    start["message"]["content"][0]["input"]["run_in_background"] = json!(true);
    let mut started = result_line(2, "toolu_bg", "", None);
    started["toolUseResult"] = json!({"backgroundTaskId": "task01"});
    let mut stop = call_line(3, "toolu_stop", "");
    stop["message"]["content"][0]["name"] = json!("TaskStop");
    stop["message"]["content"][0]["input"] = json!({"task_id": "task01"});
    let stopped = result_line(4, "toolu_stop", "stopped", None);
    let text: String = [start, started, stop, stopped]
        .iter()
        .map(|l| {
            format!(
                "{l}
"
            )
        })
        .collect();
    fs::write(root.join(format!("{SID}.jsonl")), text).unwrap();
    let s = Session::at(dir, root, "windows");
    let e = env();
    e.put(&s, s.team(0));
    let agent = &e.agents(&s)[0];
    let mut results = Vec::new();
    let mut before = None;
    loop {
        let page = e.timeline(&s, agent, before, Some(1)).unwrap();
        let Some(last) = page.last() else { break };
        before = Some(last.cursor);
        results.extend(
            page.into_iter()
                .filter(|d| d.kind == TimelineKind::ToolResult),
        );
    }
    // 시작 결과와 TaskStop 이 닫은 원 호출의 결과
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(
        results.iter().all(|r| r.summary != UNREADABLE),
        "{results:?}"
    );
}

/// 실행 중이던 exec 의 결과는 wait 출력 줄에서 exec 의 call_id 로 난다. 그 줄 원문을 결과 본문으로 읽는다
#[test]
fn cell_wait_result_reads_its_raw_line() {
    let s = Session::fixture("codex/exec-cell-wait", "windows");
    let e = env();
    e.put(&s, s.team(0));
    let agent = &e.agents(&s)[0];
    let all = e.timeline(&s, agent, None, Some(200)).unwrap();
    let r = all
        .iter()
        .find(|d| d.kind == TimelineKind::ToolResult && d.line_no == 21)
        .expect("21번 줄 결과 칸");
    assert!(
        r.body
            .as_ref()
            .is_some_and(|b| b.head.contains("Script completed")),
        "{r:?}"
    );
    assert_ne!(r.summary, MISSING, "{r:?}");
    assert_ne!(r.summary, UNREADABLE, "{r:?}");
}
