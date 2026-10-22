# 0.2.0 남은 작업

통합 기준은 `v0.2.0`, 현재 티켓 브랜치는 `t92-signal-state`다. 전체 순서는 `PLAN_0.2.0.md` 4절을 따른다. 작업이 끝나면 이 파일을 지운다.

## 현재 위치

- T83 읽기 확인 완료. 서브와 메인이 각각 `npm.cmd run check`를 실행해 포맷 89개, 린트·타입 64개 파일 검사를 통과했다. 앱 코드 변경은 없다.
- T92 설계 변경안을 `design/HANDOFF.md` 5·6.2·10절, `docs/CONTRACTS.md` C2·C8·C10, `docs/SCENARIOS.md` S63–S75에 작성했다. 사용자 검토 완료. 티켓별 커밋·검증 뒤 `v0.2.0`에 합친다. 전체 개발·출시 전 검증을 마쳐도 main 병합 직전에 멈춘다. main 병합·정식 릴리스·배포는 실행하지 않는다.
- `PLAN_update.md`의 A1 알림, 별도 저장소를 쓰는 출시 전 검증, updater만 추가하는 결정은 유지한다. 서명 공개 키·빌드 환경·검증 저장소 준비는 T79 전에 확인한다.

## T83 후속 수정 위치

줄 번호는 앱 변경 전 기준이다. 수정이 끝난 행은 지운다.

| 파일·줄 | 현재 값·구조 | v2 값·구조 | 맡을 티켓 |
|---|---|---|---|
| `src/sidebar-left.ts:654`, `src/styles/sidebar.css:128` | `ow-chip ow-row__idle`, 높이 18px·좌우 5px | `ow-worktype.is-idle`, 높이 22px·좌우 8px | T84 |
| `src/sidebar-left.ts:906`, `src/styles/sidebar.css:204` | `ow-rail__letter` 20px 타일, 퇴근 opacity 0.6 | `ow-rail__item` 32px 색 타일, 퇴근 opacity 0.5 | T84 |
| `src/styles/sidebar.css:239`, `:263` | 고정 레일 그림자, 선택 상자 `#fff` | 테마 그림자 토큰, `--ow-surface` | T84·T90 |
| `src/header.ts:122` | 정적 배율 `span` | `ow-chip is-line is-zoom` 버튼과 배율 메뉴 | T85 |
| `src/sidebar-right.ts:281`, `:308`, `:332` | 카드 전체 `button`, 컨텍스트 눈금, `ow-trail__row` | `article` + `ow-card__open`, `ow-ctx__bar`, `ow-trail__item` | T86 |
| `src/styles/panel.css:53`, `:160` | 토스트 `#fff`, 퇴근 카드 opacity 0.6 | `--ow-surface`, 상태별 배경·글자 색 | T86·T90 |
| `src/styles/panel.css:131` | 티커 `font: inherit; color: inherit` | v2 항목·시각·이름 토큰 | T88 |
| `src/popup-employee.ts:69`, `:228` | `ow-avatar--lg`, 컨텍스트 눈금 | `ow-avatar--xl` 80×88, `ow-ctx__bar` | T89 |
| `src/styles/popup.css:95`, `:157` | 왼쪽 여백 24px | 공통 좌우 20px | T89 |
| `src/popup-session.ts:215`, `src/styles/popup.css:337` | 세션 사건 행도 `ow-tl`, 후순위 CSS가 grid 지정 | 직원 상세의 v2 `ow-tl` flex 목록과 충돌하지 않게 범위 제한 | T89 |
| `dev/viewport.ts:113` | `ow-trail__text` 비교 선택자 | v2 행적 선택자 | T87 |

## 구현 전 남은 결정

- 오늘 테스트 누계: 실행 횟수 또는 개별 테스트 항목 수. 사용자에게 선택 요청함.
- 메달: 5분 유지·추가 통과 때 갱신 또는 다음 확정 테스트 실패까지 유지. 사용자에게 선택 요청함. 기간·취소 조건을 정하기 전 `medal_until` 계약을 확정하지 않는다.
- 계획 모드의 장애·퇴근 중단은 사용자 답대로 회의실 연출만 중단하고 `planning`을 유지하도록 반영했다.
- 메달의 연속 중단 조건도 최종 검토한다. 제안은 다른 업무가 끼어도 연속을 유지하되 테스트 실패·반려·불명확 결과는 연속을 끊는 것. 대안은 확정 테스트 결과만 추려 실패 때만 끊는 것인데, 중간에 결과를 모르는 테스트가 있어도 연속 통과처럼 보일 수 있다. 아직 계약에 어느 쪽도 확정하지 않았다.

## T93 지시서에 반영할 근거와 남은 확인

- `TestVerdict`는 추정 Mixed 결과도 담는다. 새 확정 누계·메달·하이파이브는 결과판이나 도장 수를 세지 않고 대응 호출의 종류·종료 상태·신뢰도를 본다 (`classify.rs`의 `on_result`).
- `ExitPlanMode`는 현재 일반 `Ask`와 같은 종류여서 결과가 어떤 질문의 답인지 구분할 계획 호출 표식이 필요하다. Codex에 동등 신호가 있다는 가정은 하지 않는다.
- 고유 파일 수는 기존 `record.files_read`·`files_edited`를 재사용한다. 성공 파일 수가 아니라 경로가 기록된 호출의 고유 경로 수이며, Codex 여러 파일 패치는 현재 첫 파일만 보존한다.
- 우선순위에 기존 `waiting_user_since` 대기와 반려 직후 유지가 빠져 있다. 장애 때문에 이 상태를 덮지 않도록 기존 동작을 보존한다.
- 기존 장애 상태는 부장만 상황실로 옮긴다. HANDOFF의 팀원 자동 소집 문구를 실제 코드에 맞춘다.
- T93 범위에 상태 구조체가 있는 `office/mod.rs`와 실제 상태 테스트가 있는 `office/tests.rs`, 새 fixture 출처를 적는 `fixtures/README.md`를 보강했다.
- 압축 사건의 기존 `Repot` cue는 부장에게만 발생한다. HANDOFF의 직원별 자료실 방문에 맞추려면 T93에서 방문할 직원의 식별자를 보존해야 한다. 기존 부장 화분 갱신을 서브 압축으로 바꾸지 않도록 방문 cue와 화분 cue를 구분한다.
- 하이파이브의 10초는 수신 시각이 아닌 사건 시각으로 비교해야 한다. 현재 `Stamp`에 시각이 없으므로 T93 지시서에서 전달 형태를 정하고, T97이 화면을 다시 열 때 지난 통과 쌍을 재생하지 않게 한다.
- 기존 Mixed 테스트 성공의 장애 해제 동작은 `office/tests.rs`에 고정돼 있다. 새 확정 누계·메달 기준을 추가하는 일과 기존 동작을 바꾸는 일을 섞지 않는다.
- 메달·누계·계획 상태는 기존 재구성 테스트의 `final_state` 비교에 들어 있지 않다. 새 필드를 직접 비교하는 검증을 포함한다.

## 다음 순서

1. 승인된 T92 커밋·통합 뒤 T93을 진행한다. 누계·메달 선택과 무관한 부분을 먼저 구현한다.
2. T93 지시서: 새 필드·호출 연결·확정 누계·재구성·날짜 경계 fixture 검증. 그 뒤 UI 티켓을 시작한다.

3. 전체 개발 및 T82-1 출시 전 업데이트 검증 완료 후 main 병합 직전에 정지한다. T82-2·T82-3과 계획 파일 삭제는 아직 완료로 표시하지 않는다.
