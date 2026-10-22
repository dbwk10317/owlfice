# 0.2.0 남은 작업

통합 기준은 `v0.2.0`이다. T84 기본 화면·T85·T88을 통합했고 T84 데이터 연결과 T86을 진행한다. 전체 순서는 `PLAN_0.2.0.md` 4절을 따른다. 작업이 끝나면 이 파일을 지운다.

## 현재 위치

- T83 읽기 확인 완료. 서브와 메인이 각각 `npm.cmd run check`를 실행해 포맷 89개, 린트·타입 64개 파일 검사를 통과했다. 앱 코드 변경은 없다.
- T92 설계 변경안을 `design/HANDOFF.md` 5·6.2·10절, `docs/CONTRACTS.md` C2·C8·C10, `docs/SCENARIOS.md` S63–S75에 작성했다. 사용자 검토 완료. 티켓별 커밋·검증 뒤 `v0.2.0`에 합친다. 전체 개발·출시 전 검증을 마쳐도 main 병합 직전에 멈춘다. main 병합·정식 릴리스·배포는 실행하지 않는다.
- T92 커밋 `01d1c8d`를 `v0.2.0`에 통합했다. T93의 상태·DTO·합성 fixture 구현과 메인 검증을 완료했으며, main 병합 직전에 멈춘다는 지시가 우선한다.
- `PLAN_update.md`의 A1 알림, 별도 저장소를 쓰는 출시 전 검증, updater만 추가하는 결정은 유지한다. 검증 저장소 `dbwk10317/owlfice-updater-test`는 사용자 요청으로 private으로 전환했다. 전환 전 README만 있었고 릴리스·설치 파일·키는 없었다. 앱에 GitHub 토큰을 내장하지 않는 검증 절차로 조정한다. 서명 키는 저장소 밖 `%USERPROFILE%/.tauri/owlfice.key`에 암호화해 생성했다. 비밀번호는 같은 폴더 `owlfice.password.dpapi`에서 현재 Windows 계정으로 복호화한다. 표본 서명의 올바른 비밀번호 성공·잘못된 비밀번호 거부를 확인했다. 최초 사용하지 않은 비암호화 키는 교체했다. 공식 배포 전 별도 백업 확인은 남아 있다.
- 추가 요청 T102: 설정에 실제 앱 로그 위치와 탐색기 열기를 표시한다. 운영 로그와 미대응 로그는 이미 같은 폴더에 있고 열기 명령도 있으므로 기존 기능을 재사용한다. T90 뒤에 진행한다.

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

## 확정한 구현 기준

- 사용자가 추천안을 선택했다. 누계 단위·메달 수명·연속 중단 규칙은 HANDOFF 5.2절로 옮겼다.
- 계획 모드의 장애·퇴근 중단은 사용자 답대로 회의실 연출만 중단하고 `planning`을 유지하도록 반영했다.
- 사용자 답에 따라 결재 대기 숫자는 대기하는 직원 수다. T93의 공통 데이터 선행으로 `TeamSummaryDto.askingCount`를 추가하고 T85 머리말도 같은 값을 합산한다.

## T93에서 후속 티켓으로 넘길 것

- `Stamp`는 `agent`·`at`·`confirmedPass`를 보존한다. T97은 확정 통과만 사용하고 화면 복귀 시 지난 통과를 재생하지 않는다.
- 직원 압축 방문은 `Compaction`, 기존 부장 화분은 `Repot`이다. T95는 둘을 구별한다.
- T93은 계획·메달·날짜 누계·고유 경로·결재 인원 데이터를 준비했다. 새 방과 소품을 화면에 적용하는 것은 후속 티켓이다.
- Codex 계획 모드 대응 신호와 여러 파일 패치의 모든 경로는 추가하지 않았다. 영구 기준은 HANDOFF 5.2절이다.

## 다음 순서

1. T84 후속은 lastActivity·readFailed를 연결한다. T86은 직원 카드 구조·핀·컨텍스트·정렬을 구현한다. 실제 Windows 앱을 임시 APPDATA와 합성 세션으로 검수한다. 기본 화면을 본 것으로 전체 디자인 검수를 완료 표시하지 않는다.
2. `PLAN_0.2.0.md` 4절의 순서를 이어 간다. T102 로그 위치는 T90 뒤다.
3. 전체 개발 및 T82-1 출시 전 업데이트 검증 완료 후 main 병합 직전에 정지한다. T82-2·T82-3과 계획 파일 삭제는 아직 완료로 표시하지 않는다.

## 실제 화면 검수에서 이어갈 것

- 사용자 확정: 카드의 세션 이동은 기존 원본 폴더 열기를 쓰고 버튼 이름도 그에 맞춘다. 결재판 열기는 세션 상세다.
- 코드 기준으로 직원 카드 수치는 세션 누계다. HANDOFF의 “오늘” 표기는 누계로 고쳤다. 자동 확인 바닥 점은 중지 감지가 아니라 활성 표시다.
- 실제 Windows 앱에서 1600/1280/960/800/최소 크기, 높이 640 경계, 주야간, 팝업·키보드·로그 열기·배율·대비를 확인한다. 720 미만은 제품 최소 폭 때문에 직접 창 축소로 도달하지 않으므로 별도 검증 구성에서 다룬다.
