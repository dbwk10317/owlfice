# 0.2.0 계획: 디자인 v2 적용 + 상태 모델 확장

디자인 v2(캔버스 [Owlfice 디자인 0.2.0](https://claude.ai/artifact/4PtdXQYh8xL3BQwJBCkgfV))는 이 브랜치의 `design/` 에 반영됐다. 앱(`src/`, `src-tauri/`)은 아직 0.1.0 마크업이다. 이 문서는 앱을 v2 로 옮기고 확장안(새 방·소품·자세·담당·소지품·계절)을 넣는 티켓 목록이다. 끝나면 지운다 (T82 뒤). 남길 결론은 `design/HANDOFF.md`·`docs/CONTRACTS.md` 와 코드 주석으로 옮긴다 (AGENTS.md 7절).

이 릴리스는 "디자인 적용"이 아니라 **디자인 적용 + 상태 모델·데이터 계약 확장**이다. 확장안이 요구하는 신호 중 지금 파서·상태·DTO 에 없는 것이 있어서(1절 표), UI 보다 Rust 쪽 선행 티켓(T92·T93)이 먼저다. 자동 업데이트(`PLAN_update.md` T79–T82)도 0.2.0 에 포함한다.

## 결정된 것

- 확장안 2차(새 방 4개, 자세 +5, 담당 +3, 소지품 +7, 계절)는 0.2.0 에 넣는다 (사용자, 2026-10-22).
- 자동 업데이트는 0.2.0 에 넣고, 정식 출시 **전에** 서명된 설치본 두 개로 검증한다 (`PLAN_update.md` B 절).
- 야근 모드 자동 켜짐 기본값: **20시–06시 자동** (사용자가 달리 말하면 바꾼다. 설정에서 끌 수 있다).
- 브랜치: 통합 `v0.2.0`, 워크트리 `D:/workspace/owlfice-wt/v0.2.0`. 티켓 브랜치는 여기서 따고 여기로 합친다. main 병합·push 는 사용자 지시 때.

## 1. 신호 지원표

신호·확인/추정·보존할 상태·DTO·연출·fixture의 대응은 `design/HANDOFF.md` 5.2절로 옮겼다. T92 검토 전 미결정 항목과 현재 작업 위치는 루트 `WORK_0.2.0.md`를 본다.

## 2. 상태 우선순위와 시간표

자리·자세 우선순위와 새 방의 진입·유지·종료·중단은 `design/HANDOFF.md` 5.3·5.4절, 시간표는 6.2절에 둔다. T90·T91은 같은 시간표를 따른다. 계약은 `docs/CONTRACTS.md` C2·C8·C10, 상황 번호는 `docs/SCENARIOS.md`다.

## 3. 티켓

번호는 `PLAN_update.md` 의 T79–T82 다음부터. 등급은 AGENTS.md 3절. **범위**에 적힌 파일만 고친다. 범위 밖이 필요하면 멈추고 메인에 알린다.

### T83 tokens.css 적용 확인 (대리 · 하)

- 범위: 읽기만. `src/styles/*.css`, `src/*.ts`, `dev/*.ts` 에서 `design/handoff/tokens.css` 값을 덮어쓰거나(`#fff`, 고정 padding) v2 에서 이름이 바뀐 클래스를 쓰는 곳.
- 완료 기준: 파일·줄·현재 값·v2 값 표. `npm run check` 녹색. 코드는 고치지 않는다.

### T84 사이드바 v2 (과장 · 중)

- 범위: `src/sidebar-left.ts`(+test), `src/styles/sidebar.css`, `design/handoff/tokens.css`(`.ow-rail__letter` 와 그 ponytail 주석 삭제만), `dev/company.ts`·`dev/company.html`(샘플 마크업).
- 할 일: HANDOFF 4.1 표대로. 간판 타일 + 회사 이름 첫 줄(설정 `companyName`, 없으면 `phrases.app.brand`), 지표 띠 `ow-kpi`(불 켜진 사무실/층 · 근무 중 직원 · 결재 대기), 행 여백, `ow-worktype.is-idle` + `quietFor`, `ow-host` 테두리형, `is-broken`, 바닥 상태 점, 레일 `ow-rail__item` 32 타일.
- 완료 기준: 지표 숫자 계산 테스트(사무실 n·근무 중 n·결재 n, 중소기업 층 n), 기존 테스트 통과, 1600·960·800 폭 구조가 HANDOFF 2.2 와 같음.

### T85 머리말 v2 + 배율·가운데로 (과장 · 중)

- 범위: `src/header.ts`(+test), `src/main.ts`, `src/office/renderer.ts`(+test; 공개 API 에 `setScale(n)`·`centerCanvas()` 추가, 현재는 배율 표시만 있다), `src/styles/app.css`, `src-tauri/capabilities/default.json`(창 최대화 권한 `core:window:allow-toggle-maximize` 가 필요하면), `design/handoff/phrases.ko.json` header.
- 할 일: HANDOFF 4.3. 세그먼트 묶음(야근 토글은 T90 전까지 `disabled`), "결재 n건 대기" 칩, 배율 칩 메뉴(1×·2×·3×, 들어가지 않는 배율은 비활성, 고르면 `setScale`), 무대 모서리 `ow-stage__tools`(가운데로 = 패닝 원점 복귀, 전체 화면 = 창 최대화 토글).
- 완료 기준: 배율 메뉴·가운데로 테스트(렌더러 가짜 객체로 호출 확인), `aria-pressed`, `npm test`·`cargo test` 녹색.

### T86 직원 카드 v2 (과장 · 중)

- 범위: `src/sidebar-right.ts`(+test), `src/avatar.ts`(핀 자리), `src/styles/panel.css`, `design/handoff/tokens.css`(`.ow-ctx i` 눈금 규칙과 ponytail 주석 삭제, `.ow-card.is-idle` 은 아래 구조에 맞춰 조정), `dev/actors.ts`·`dev/life.ts`(샘플).
- **구조 결정 (먼저 한다, 메인 승인)**: 지금은 카드 전체가 `<button>` 이라 안에 버튼을 둘 수 없다 (HTML 규격). 바꾼다: 카드는 `<article class="ow-card">`, 머리줄(아바타·이름·직급)을 감싼 `<button class="ow-card__open">` 이 직원 상세를 열고 로빙 tabindex 의 대상이다. 오른쪽 위 호버 액션 `ow-card__actions` 과 허가 대기 버튼 2개는 형제 `<button>`. 키보드: ↑↓ 로 카드의 `ow-card__open` 사이 이동, Tab 으로 그 카드의 다른 버튼, Enter 로 실행. `focus-within` 이면 호버 액션 표시. HANDOFF 10절 초점 순서 문장을 이에 맞춰 고친다.
- 할 일: HANDOFF 4.5 해부·변형 표. 핀, 직급 칩, 담당 말줄임, 컨텍스트 막대, 타임라인 행적(`is-bad`), 실적 숫자, 호버 액션, `is-ask` 변형, `is-away`, 압축형 9/12, 팀 머리글 줄바꿈 금지, 현황판 머리 `sort`.
- 완료 기준: 변형마다 DOM 테스트 1개, 키보드 이동 테스트(↑↓·Tab·Enter), 기존 테스트 통과, 창 높이 640 미만 압축형. 탕비실 카드의 글자 대비 4.5:1 이상 (카드 전체 opacity 가 아니라 글자 색으로 흐리게 — tokens.css 는 이미 그렇게 바꿨다).

### T87 무대 v2 (대리 · 하)

- 범위: `src/styles/app.css`, `src/main.ts`(무대 DOM), `src/office/canvas-tooltip.ts`(방 이름표), `dev/viewport.ts`.
- 할 일: 비네트, 바닥 그림자, 유리 방 이름표, 캔버스 `drop-shadow`.
- 완료 기준: 125%·150% DPR 도트 선명(HANDOFF 12절), 2×·3× 에서 그림자가 캔버스 밖에만. **drop-shadow 프레임 시간을 재서 기존 대비 +1ms 를 넘으면 필터를 빼고 바닥 그림자만 둔다** (tokens.css ponytail 주석 정리). 측정값을 커밋 메시지에.

### T88 티커 v2 (대리 · 하)

- 범위: `src/ticker.ts`(+test), `src/styles/panel.css`.
- 할 일: 항목 알약(시각·팀 태그·이름·요약), 호버 `is-paused` + 항목 회색, 조용할 때 회색 칩, `prefers-reduced-motion` 정지.
- 완료 기준: 호버 정지·재개 테스트, 기존 테스트 통과.

### T89 직원 상세 팝업 v2 (과장 · 중)

- 범위: `src/popup-employee.ts`(+test), `src/styles/popup.css`, `src/popup-session.test.ts`·`src/popup-settings.test.ts`(공통 여백 변화로 깨지면 기대값이 아니라 마크업을 본다).
- 할 일: HANDOFF 4.6 직원 상세 행. 머리 칩 줄 + 근무 상태 칩 + 원본 열기, 타임라인 `ow-tl` 접힘/펼침, 실적 요약 `ow-stat`.
- 완료 기준: 펼침·접힘·더 보기 테스트, 세션 상세·설정 팝업 회귀 없음.

### T90 야근 모드 (과장 · 중)

- 범위: `src/main.ts`, `src/header.ts`(토글 동작), `src/popup-settings.ts`(+test), `src/store.ts`, `src-tauri/src/ipc.rs`(`SettingsDto.night_mode: Option<NightMode>` — DTO 원본은 여기다), `src-tauri/src/settings.rs`(+test, 키 `nightMode: "auto" | "on" | "off"`, 기본 `auto`), IPC 타입 재생성(docs/BUILD.md), `dev/settings.ts`(완전한 설정 객체), `design/handoff/phrases.ko.json` settings.
- 할 일: HANDOFF 4.8. `<html data-theme="night">`, `auto` 는 2절 시간표의 20–06시, 전환 `--ow-dur-theme`, 조명과 독립. 설정 "화면과 소리" 절에 세그먼트 3개.
- 완료 기준: 세 값 DOM 테스트 + 20시·06시 경계 테스트(시계 주입), `cargo test` 설정 round-trip + 옛 설정 파일(키 없음) 읽기, 야근 모드 글자 대비 표(글자 색 × 바탕 색 계산값, `is-idle` 카드 포함) PR 첨부.

### T91 조명 시간표 (대리 · 하) — T92 뒤

- 범위: `src-tauri/src/office/classify.rs`(+test; `props.night` 의 `!(6..22)` 를 20시 기준으로 — 밤 판정은 여기다), `src/office/props.ts`(+test; `windowState` 17–20 dusk / 20–06 night), `design/sprites/floorplan.json`(`lightTints` + `rule`), `design/tools/owlfice_art.py`(참조 렌더), `src/office/floorplan.ts`(`LightName` 에 `evening`), `src/office/renderer.ts`(+test; `evening` 을 프론트 시계로 따로 판정하면 Rust 의 밤 판정과 어긋나므로 **`TeamDto` 에 `daylight: "day"|"evening"|"night"` 하나를 두고 창문·조명·램프가 모두 그것을 읽는다** — 이 필드는 T91 이 추가한다, `src-tauri/src/ipc.rs` 와 IPC 타입 재생성 포함), `design/assets/office-claude-evening.png`(새 참조), HANDOFF 6.2절 7번과 5절 창문 행.
- 할 일: 2절 시간표대로. 저녁 `#F6E6C4`, 야근 20시부터, 램프 비네트 레이어(`overlay`).
- 완료 기준: 참조 렌더 비교에 저녁 1장 추가 통과, 17·20·06시 경계 테스트.

### T92 신호·상태 설계 확정 (메인 · 상)

- 범위: `design/HANDOFF.md` 5절(신호 지원표·우선순위·새 방 조건)·6.2절·10절, `docs/CONTRACTS.md` C2(새 상태 `planning`, 일회성 방문)·C8(시간표)·C10(재구성 불변식에 메달·누계 추가), `docs/SCENARIOS.md`(새 S 번호).
- 할 일: HANDOFF 5.2절의 보존 상태와 미확인 입력을 확정하고 5.3절 우선순위·6.2절 시간표를 계약에 연결한다. 각 새 방의 진입·유지·종료·중단 조건을 5절 표 형식으로 쓴다.
- 완료 기준: 사용자 검토 뒤 커밋. 이 티켓 전에는 T93 이후를 시작하지 않는다.

### T93 파서·상태·DTO 확장 (과장 · 중) — T92 뒤

- 범위: `src-tauri/src/parse/claude.rs`·`codex.rs`(`EnterPlanMode` 도구 종류), `src-tauri/src/parse/mod.rs`(`ToolKind`), `src-tauri/src/office/classify.rs`(`planning`, `pass_streak`·`medal_until`, 팀 날짜별 누계, `Cue::Stamp{agent_id}`, `approves` 낱말 경계; 기존 상태 테스트 배치는 `office/tests.rs`를 따른다), `src-tauri/src/office/mod.rs`(직원·팀 상태, `Cue`), `src-tauri/src/office/tests.rs`(상태·재구성 검증), `src-tauri/src/ipc.rs`(`EmployeeDto.unique_reads`·`unique_edits`·`medal`·`planning`, `TeamDto.tests_today`·`commits_today`), IPC 타입 재생성 → `src/types/*`, 그리고 **타입 변경으로 깨지는 프론트 사용처만**: `src/test-office.ts`(테스트 데이터 생성기), `src/office/actors.test.ts`(`Stamp` cue 생성), `dev/*.ts`(개발 시나리오의 DTO 리터럴), `src/types/*` 를 쓰는 다른 테스트. `fixtures/claude/*`(새 합성 시나리오: 계획 모드, 연속 통과 3회, 거부, 컴팩션), `fixtures/README.md`(새 fixture의 합성 여부·출처).
- 완료 기준: 새 상태·누계마다 fixture replay 테스트 1개 + **재구성 불변식 테스트** (같은 fixture 를 처음부터 다시 읽어도 `pass_streak`·`medal_until`·누계·`planning` 이 같다, C10), 날짜가 바뀌면 누계가 0 이 되는 테스트, `cargo test`·clippy·`npm run check`·`npm test` 모두 녹색. **화면 기능은 바꾸지 않는다** — 프론트 수정은 새 필드를 채우는 생성기·테스트 데이터·개발 시나리오와 타입 검사 통과에 필요한 것까지만.

### T94 소품 확장 1차 (과장 · 중) — T93 뒤

- 범위: `design/tools/owlfice_art.py`, `design/sprites/office.json`·`office.png`, `design/assets/office-*.png`(참조 렌더 재생성), `src/office/props.ts`(+test), `src/office/renderer.ts`, HANDOFF 5절.
- 할 일: 1절 표의 소품 행 — 커피 잔(입사 1h 마다, 최대 4)·책상 램프(야근), 서류 더미(고유 수정 파일 10개마다)·모니터 2대(병렬), 책장 채움(고유 읽은 파일 0·25·50·100), 상자 쌓임(오늘 커밋)·파란 트럭(PR 연결), 전광판(오늘 합격/반려 누계).
- 완료 기준: 소품마다 replay 테스트 1개 + 사무실 전환 뒤 상태만으로 같은 그림이 나오는 테스트, 참조 렌더 4장 비교 통과, 생성기 겹침·누락 검사 통과.

### T95 새 방 4개 (상 · 메인이 평면도, 과장이 구현) — T93 뒤

- 범위: `design/sprites/floorplan.json`(rooms·stands·path·layouts.large/small), `design/tools/owlfice_art.py`(회의실 화이트보드, 자료실 파쇄기, 경비실 창구·차단봉, 옥상 조각·자판기), `design/sprites/office.*`, `design/assets/office-*.png`, `src/office/floorplan.ts`(+test), `src/office/actors.ts`·`anim-queue.ts`(+test), `src/office/canvas-tooltip.ts`(방 이름표), `design/handoff/phrases.ko.json` rooms, `design/handoff/icons/room-*.svg` 4개, HANDOFF 4.4·5·6.5절.
- 방별 조건 (T92 가 확정):
  - `design/HANDOFF.md` 5.4절. 장애·퇴근은 회의실 연출만 중단하고 `planning`은 보존한다. 일반 질문 응답을 계획 종료로 쓰지 않는다.
- 순서: 메인이 격자 안에서 방 4개의 사각형·출입 지점·서 있을 자리를 정하고 줄어드는 기존 방(후보: 개발실 임시 책상 열, 거래처 미팅실 폭)을 HANDOFF 6.5절에 적는다 → 과장이 생성기·배치·이동 → 대리가 테스트·fixture.
- 완료 기준: 방마다 replay 테스트(진입·종료·중단 각 1개), 우선순위 테스트(회의실 중 장애 → 상황실), 참조 렌더 비교 통과, 1×·2× 방 이름표 비겹침, 좌석·임시 책상 수가 HANDOFF 11절과 같음.

### T96 감정 표식 · 근속 표식 (과장 · 중) — T93 뒤

- 범위: `src/office/canvas-tooltip.ts` 또는 새 `src/office/emote.ts`(+test), `src/office/actors.ts`, `design/tools/owlfice_art.py` + `design/sprites/owls.json`·`owls.png`·`preview/*.png`(근속 `wear/<단계>/` 레이어), `design/assets/office-*.png`, HANDOFF 4.4·5·6.4절.
- 할 일: 감정 표식 6종(1절 표의 확인/추정 그대로, 1.2초, 말풍선 있으면 생략), 근속 표식 3단계(`hiredAt` 뒤 1h·3h·6h).
- 완료 기준: 표식마다 replay 테스트 + **사무실 전환·창 복귀 때 지난 표식을 재생하지 않는 테스트**, 근속 레이어가 `hiredAt` 으로 바뀌는 테스트(같은 팀에 방금 들어온 서브는 0단계), 참조 렌더 갱신.

### T97 자세 +5 (과장 · 중) — T96 뒤

- 범위: `design/tools/owlfice_art.py`, `design/sprites/owls.*`·`preview/poses.png`, `src/office/actors.ts`·`anim-queue.ts`(+test), HANDOFF 6.4절 표.
- 할 일: think(기존 침묵 기반 `Pose::Think` 의 발생·종료 조건 그대로, 프레임 2장만 새로, 추정 꼬리표 유지), highfive(`Cue::Stamp{agent_id}` 둘이 10초 안, 2인), stretch(`hiredAt` 뒤 1시간마다), drink(응답 대기 중 30초마다), phone(사장님 메시지). 2절 우선순위를 지킨다.
- 완료 기준: 자세마다 replay 테스트 + 우선순위 테스트(허가 대기 중 stretch 금지, 장애 중 drink 금지), 프레임 규격 6.4절, 미리보기 갱신.

### T98 담당 +3 · 소지품 +7 (과장 · 중) — T97 뒤

- 범위: `src-tauri/src/office/work.rs`(+test; 담당 판정은 여기다), `src-tauri/src/office/mod.rs`(`Role`), `src-tauri/src/ipc.rs`(`Role` 값 추가, `EmployeeDto.items`), IPC 타입 재생성, `src/types/*`, `src/sidebar-right.ts`(`ROLE_LABEL: Record<Role, string>` — 값을 더하지 않으면 타입 오류), `src/avatar.ts`, `design/tools/owlfice_art.py`, `design/sprites/owls.*`·`office.*`(`held_*`), `design/handoff/phrases.ko.json` roles, HANDOFF 6.4절.
- 담당 규칙: **기존 파일 경로·확장자 규칙 그대로**, 범주만 더한다 — 보안(`auth/`·`security/`·`*.pem`·`*.policy`·권한 설정 파일), 데이터(`*.sql`·`*.csv`·`*.parquet`·`*.ipynb`), 디자이너(`*.css`·`*.scss`·`*.fig`·`design/`). 수습(고유 파일 5개 미만)은 이미 있다 → 신규는 3종. `subagent_type` 보조 규칙은 **넣지 않는다**: `SubagentSpawn` 사건에 그 값이 없고(파서가 보존하지 않음) 이번 범위에 파서 변경이 없다. 지시문 본문도 보지 않는다.
- 소지품: 메달(`medal`), 안전모(장애 상황실), 서류 가방(퇴근 걷기), 손전등(`off` 조명에서 재출근), 담요(sleep 10분), 헤드폰(백그라운드 명령), 두루마리(`planning`, T95 뒤).
- 완료 기준: 담당 판정 단위 테스트(범주별 경로 → 담당, 보조 규칙, 재구성 뒤 같음), 소지품마다 replay 테스트, 카드 아바타 20×22 자르기가 새 레이어에서 깨지지 않음.

### T99 계절 장식 (대리 · 하) — T95 뒤

- 범위: `design/tools/owlfice_art.py`, `design/sprites/office.json`(`season/<이름>`)·`office.png`, `design/sprites/holidays.json`(설·추석 날짜표 2026–2035, 표준 라이브러리에 음력이 없어 표로 둔다. 범위 밖 연도는 생략), `src/office/props.ts`(+test), `src/office/renderer.ts`(테스트용 `season: false` 옵션), HANDOFF 5절.
- 할 일: 창밖 계절 색조 4종, 12월 트리·양말, 설·추석 벽 등. OS 로컬 날짜. 비·눈은 넣지 않는다.
- 완료 기준: 날짜 주입 테스트(사계절 + 12월 + 설·추석 + 표 밖 연도), 참조 렌더 비교는 `season:false` 로 돌려 그대로 통과.

### T100 접근성 · 움직임 줄이기 통합 검수 (과장 · 중) — 위 전부 뒤

- 범위: `src/*.ts`, `src/office/actors.ts`·`props.ts`·`anim-queue.ts`(반복 연출 정지), `src/styles/*.css`, HANDOFF 10·12절.
- 할 일: HANDOFF 10절 + v2 추가분(세그먼트 `aria-pressed`, 핀 `aria-label`, 카드 키보드 구조 T86, 티커 정지, 야근 대비), `prefers-reduced-motion` 에서 Pixi 쪽 반복(자세 반복·소품 깜빡임·감정 표식)도 멈춘다. 12절 "외부 네트워크 요청이 하나도 없다"는 **"업데이트 확인·다운로드(Rust 플러그인, 사용자 조치 전 다운로드 없음) 외에는"** 으로 고친다 (`PLAN_update.md`).
- 완료 기준: 12절 전 항목 ✓ 또는 사유, 실측 대비 표.

### T101 스크린샷 · 버전 · README (메인 + 사용자) — 릴리스 직전

- T82-1 출시 전 검증이 통과한 뒤에 한다. `docs/images/office.png`·`employee.png` 재촬영(합성 데이터, 주간 1장 + 야근 1장 추가), `README.md` 캡션과 **설치 파일 링크 파일명을 0.2.0 으로** (`latest/download/Owlfice_0.2.0_x64-setup.exe`. 파일명이 고정이라 릴리스마다 바꿔야 한다. `PLAN_update.md` T81 절차 1번). `tauri.conf.json`·`package.json`·`src-tauri/Cargo.toml`(+`Cargo.lock`) 0.2.0 — T81 의 `release_check.py` 가 셋을 같이 본다.
- 릴리스 자체는 T82-2 (`PLAN_update.md`). 이 문서와 `PLAN_update.md` 삭제는 T82-3 뒤.

## 4. 순서

T83 → T92(메인) → T93 → (T84 ‖ T85 ‖ T88) → T86 → T87 → T89 → T90 → T91 → T94 → T95 → T96 → T97 → T98 → T99 → T100 → `PLAN_update.md` T79 → (T80 ‖ T81) → T82-1 출시 전 검증 → T101 버전·문서 확정 → T82-2 정식 출시 → T82-3 실전 확인 → 두 PLAN 삭제.

- 쓰는 서브는 같은 파일에 둘을 두지 않는다 (AGENTS.md 3절). T84·T85·T88 은 파일이 겹치지 않아 동시 가능.
- T94–T99 는 모두 `owlfice_art.py` 와 스프라이트 시트를 고치므로 하나씩. 생성기를 고칠 때마다 `python design/tools/owlfice_art.py` 가 끝까지 돌고 겹침·누락 검사를 통과해야 한다.
- IPC 타입을 바꾸는 티켓(T85·T90·T91·T93·T98)은 `OWLFICE_WRITE_TYPES=1 cargo test ts_bindings` 를 돌리고 생성 파일을 같이 커밋한다.

## 5. 디자인 측 완료 (이 브랜치, 2026-10-22)

- `design/project/`: v2 19장 + `canvas.json` (캔버스와 같은 파일). 0.1.0 의 `Main`·`MainSmb`·`Components` 는 v2 `Main`·`MainNight`·`Cards`/`Sidebar`/`Header`/`Tokens` 로 대체.
- `design/handoff/tokens.css`: v2 토큰·컴포넌트 규칙·`[data-theme="night"]` 블록. 앱이 아직 쓰는 0.1.0 규칙(`.ow-ctx i`, `.ow-rail__letter`)은 ponytail 주석 → T86·T84 가 지운다. 탕비실 카드는 opacity 대신 글자 색으로 흐리게 (대비 4.5:1).
- `design/handoff/phrases.ko.json` 키 19개, `design/handoff/icons/` 4개, `design/HANDOFF.md` 0·1·2.1·3·4·7·12절과 4.8절.
- 리뷰 1차(2026-10-22) 반영: 1·2절 표, T86 카드 구조, T92·T93 선행, 범위 파일 보강, 시간표, T99 음력 표. 리뷰 2차 반영: 우선순위를 코드와 같게(허가 대기·진행 중 도구 > 장애), 출고장·외근은 실제 활동, `hiredAt`·응답 대기 추정·think 침묵 기반, T91 에 Rust 밤 판정과 `windowState` + `daylight` 필드, T93 프론트 사용처 허용, T98 보조 규칙 삭제와 `ROLE_LABEL`, 출시 순서 T82-1 → T101 → T82-2 → T82-3. 받지 않은 것: 없음. "훅 차단 → 경비실"과 "PR 머지 색"은 신호가 없어 뺐다.
