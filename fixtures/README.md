# fixtures

익명화 방법과 검사는 `tools/anonymize.py` 머리말. 경로 → 담은 시나리오, 합성 여부.

- `claude/async-subagent/302222d1-a790-f2ee-6a9b-2212d4de135d.jsonl` (+ `subagents/agent-a4fa9a3e07b34f715.{jsonl,meta.json}`) → 비동기 서브에이전트(Agent 호출, async_launched 결과, 자식 파일), 백그라운드 Bash의 `<task-notification>`, 자식 파일 안의 반려. 실제 세션 발췌
- `claude/sync-subagent/ed46b23c-eb2a-4459-1e37-cb69d8ecad93.jsonl` (+ `subagents/agent-ac0c27afe76bccd54.{jsonl,meta.json}`) → 동기 서브에이전트(자식 이벤트가 부모의 결과 레코드보다 이른 ts). 합성 (위 비동기 fixture를 바탕으로 결과 모양과 시각을 바꿈. 동기 결과의 실제 필드는 미확인)
- `claude/test-fail-fix-pass/015373c5-6b19-d6c2-c7e1-86385bee9237.jsonl` → `cargo test` 실패 → Edit → `cargo test` 통과. 합성
- `claude/rewrite-shrink/before/aac62cc7-c834-9cc6-8f65-4a0da16e2e1d.jsonl`, `claude/rewrite-shrink/aac62cc7-c834-9cc6-8f65-4a0da16e2e1d.jsonl` → 재작성(길이 감소). `before/` 를 먼저 쓰고 같은 이름으로 위 짧은 파일을 덮어쓴다. 합성 (반려 fixture를 바탕으로 앞 절반만 남김)
- `claude/parallel-reversed/bb5d9bb5-fb0b-cbf9-a342-7a82e4d82553.jsonl` → 병렬 tool_use 둘, 결과가 역순, `<persisted-output>`. 실제 세션 발췌
- `claude/compaction/2d416c44-e80b-dfec-6627-ef150260a525.jsonl` → `compact_boundary` + `isCompactSummary`. 실제 세션 발췌
- `claude/reject/13f4d27c-e1d7-fa23-0b0e-05ad838d1b68.jsonl` → 사용자 반려(Bash). 실제 세션 발췌
- `claude/internal-tool-failure/e52d1abf-7516-aeb5-e401-2f94e956374d.jsonl` → 내부 도구(`mcp__ccd_pr__bind_pr`)의 `is_error` 결과. 실제 세션 발췌
- `codex/codex-app/rollout-2026-03-02T09-00-00-a3c002ef-5a35-668d-d178-1e8241413423.jsonl` → Codex Desktop, `custom_tool_call` exec, 한 턴. 실제 세션 발췌
- `claude/bg-fail-reject-compact/dcc68dd7-8ab5-ede5-fb28-c09de28115be.jsonl` → 실패한 백그라운드 작업의 `<task-notification>`(`status: failed`, exit code 2), 사용자 반려(Bash), 수동 `/compact`(`trigger: manual`, 슬래시 명령 흔적 셋), 압축 뒤 턴. 실제 세션 발췌
- `codex/ask-reject-compact/rollout-2026-03-02T09-00-00-a9e19951-4ff7-996b-7dfa-2cfe04d16f10.jsonl` → `request_user_input_async` 질문과 `<send_user_message_question_reply>` 답, exec 거부(`Rejected(...)`), `/compact`(`compacted`, `ContextCompaction`). 실제 세션 발췌
- `codex/aborted/rollout-2026-03-02T09-00-00-fbeee44f-138a-8f82-87fd-61f01dfc0cca.jsonl` → 중단된 턴(`turn_aborted`, `reason: interrupted`). 실제 세션 발췌
- `codex/exec-cell-wait/rollout-2026-03-02T09-00-00-9394fa16-cc2e-5621-fd1e-7497c4634613.jsonl` → `exec` 결과 `Script running with cell ID <n>` 뒤 `wait` 호출(아직 실행 중 응답 한 번, 이어서 `Script completed` 응답), 두 번째 exec 의 wait 사이에 낀 `item_completed` CommandExecution. 실제 세션 발췌
- `claude/planning-state/11111111-1111-4111-8111-111111111111.jsonl` (+ `subagents/agent-plan-sub.{jsonl,meta.json}`) → 계획 진입·일반 질문과 계획 종료의 겹침·반려/실패·고유 경로·부장/서브 압축. 합성 (계획 진입→종료의 실제 형식은 미확인)
- `claude/test-streak/22222222-2222-4222-8222-222222222222.jsonl` → 연속 테스트 통과·메달 갱신·실패·반려·Mixed·테스트/커밋 중복 결과. 합성
