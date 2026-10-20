# 진행 상황 (임시. 1단계 착수분이 끝나면 지운다)

결정 (2026-10-20, 사용자): AGENT_ADAPTOR.md 검토 결과 pixtuoid 코드는 안 가져온다 (디코더가 4.6보다 적게 안다). 가장자리 사례만 PLAN 4.4.1·4.6.1·4.6.2·6부에 반영, 훅은 6부 2차 후보, Antigravity는 2차(T32). 문서는 지웠다. 유사 프로젝트 7개 조사 결과는 아래 "조사 결과" 절.
결정 (2026-10-20, 사용자): 프론트 Vite+, Python uv + ruff, Rust rustfmt·clippy 기본값. 티켓마다 브랜치·로컬 커밋까지, push·PR·병합은 지시 때. 끝난 티켓 브랜치는 t00-dev-tooling 에 로컬 병합하고 다음 티켓은 t00 에서 딴다. 미대응 동작은 말풍선(직원 행동)·티커(그 밖)로 보인다. 이름 풀 300개 이상. WSL 실측은 언제든 (끝나면 꺼 둔다).

브랜치: main = t00-dev-tooling (c150a73, 2026-10-20 origin에 푸시됨. T02·T03·T04·T05·T06·T09 병합됨, 테스트 66) ← 티켓 브랜치 (워크트리 D:\workspace\owlfice-wt\<브랜치>). 빌드 산출물은 D:\workspace\owlfice-wt\target 공유 (CARGO_TARGET_DIR).

## 다음 세션에서 할 일 (순서대로)

1. **T07·T08 공용 뼈대 (메인이 t00 에 직접)**: parse/mod.rs 에 `pub mod claude; pub mod codex;` 와 `pub trait Parser { fn parse_line(&mut self, bytes: &[u8], raw_ref: &RawRef) -> Vec<Event>; }`, 빈 claude.rs·codex.rs. 파일 하나(메인·서브 기록)마다 파서 하나, Reset 때 새로 만든다 (4.3). 그래야 T07(claude.rs)·T08(codex.rs)을 동시에 돌린다. RawRef.generation 은 u64, TailReader 는 u32 — T12 에서 맞춘다.
2. **fixture 2차 (대리)**: 2차 표본에서 anonymize.py 로 뽑는다. 원본·줄 범위:
   - Claude `~/.claude/projects/D--workspace/24d416d7-c361-4308-aa4f-e10db636fd3b.jsonl` 전체(1-87): 실패 백그라운드 알림(24·29·43), 거부(46-48), 수동 /compact(49-59), 압축 뒤 턴(71-81). 시나리오 이름 claude/bg-fail-reject-compact. attachment 가 커서 크기 확인.
   - Codex B `~/.codex/sessions/2026/10/20/rollout-2026-10-20T17-58-22-01a15888-7e11-7793-b544-3cadc109529d.jsonl` 전체(1-69): 질문 답(19-32), 거부(36-39), /compact(48-56). codex/ask-reject-compact.
   - Codex A `…/rollout-2026-10-20T17-57-58-01a15888-1e30-7e71-91a2-8517fe1b4fb9.jsonl` 전체(1-21): 중단된 턴 `turn_aborted`. codex/aborted.
   - 새 키·값은 허용 집합 밖이라 치환된다. stderr 의 "부모 키와 개수"를 보고 필요한 것만 집합에 더한다. 메인이 비밀 최종 검사.
3. T07 Claude 파서, T08 Codex 파서 (과장, 동시에). PLAN 4.6.1·4.6.2 는 2차 표본까지 반영됨 (bf7d0d7).
4. T38 미대응 로그 (T07·T08 뒤), T10 probe·replay.
5. 문구 초안 (미대응 말풍선·티커, PLAN 3.6): 사용자가 쉬운 설명을 원했다. 설명 뒤 넣을지 다시 묻기.
6. 6부에 남은 것: 직급 표를 지금 모델명(gpt-6.1-sol, gemini-4-argon-trial)으로 다시 볼지 (T09 리뷰에서 나옴).

## 조사 결과 (2026-10-20, 유사 프로젝트 7개. PLAN 반영 끝. 이 절은 다음 세션이 읽고 지운다)

본 것: pixtuoid, pixel-agents, agentsview, c9watch, zoetrope, age-of-agents, agent-virtual-office, ai-office-dollhouse (모두 MIT). 제외: ccdeck(AGPL), agentroom(라이선스 불명+cass), Ctrl(비공개), claude-village·recon·pixel-office(Claude만/라이선스 없음).

결론: **가져올 코드·의존성은 없다.** 전부 우리 4.6·4.5보다 얕다 (성패 판정 없음, 훅 의존, Windows 미지원, 절단 처리 없음). 가치는 가장자리 사례였고 PLAN에 넣었다. 이 PC 표본으로 **확인됨**으로 올린 것: meta.json `model`(별칭)·`effort`, `tool_use.caller` 객체, `origin.kind: peer`, `queued_command` 평문 = 턴 중 사람 입력, `custom-title` 턴마다 반복, `persistedOutputPath`, Codex 자식 `session_meta` ordinal 0 함정, `SubAgentActivity.id` = spawn `call_id`, `completed` 중복, `CommandExecution.command` 배열, `custom_tool_call.input` 의 `require_escalated`, `session_meta.context_window`, Desktop 에 `history.jsonl` 없음, 등록부 키 목록·`.key` 파일, 무시할 최상위 type 4종. 나머지는 (미확인)으로 6부.

결정 (2026-10-20, 사용자): OS 알림 넣는다(3.6, T30), Cowork는 2차 확정(6부), 순서 불변 테스트와 `OWLFICE_CORPUS_DIR` drift 테스트는 4.12에 넣었다.

임시 파일: SAMPLE_PROMPTS.md (표본 다 받음. fixture 2차 뒤 지움), D:\workspace\owlfice-wt\fixture-scripts\ (fixture 재생성 스크립트 build.sh·build2.sh·synth.py·verify.py. 저장소 밖. 안의 S 경로는 돌리기 전에 맞춘다).
