# 0.2.0 계획: 디자인 v2 를 앱에 적용

디자인 v2(캔버스 [Owlfice 디자인 0.2.0](https://claude.ai/artifact/4PtdXQYh8xL3BQwJBCkgfV))는 이 브랜치의 `design/` 에 반영됐다. 앱(`src/`, `src-tauri/`)은 아직 0.1.0 마크업이다. 이 문서는 앱을 v2 로 옮기는 티켓 목록이다. 끝나면 지운다. 남길 결론은 `design/HANDOFF.md` 와 코드 주석으로 옮긴다 (AGENTS.md 7절).

자동 업데이트(`PLAN_update.md` T79–T82)도 0.2.0 에 포함한다 (사용자 결정 2026-10-22). 그 문서의 A·B 답은 사용자가 준다.

## 브랜치

- 통합 브랜치 `v0.2.0`, 워크트리 `D:/workspace/owlfice-wt/v0.2.0`. 티켓 브랜치는 여기서 따고 여기로 합친다. main 병합과 push 는 사용자 지시 때.
- 디자인 측은 끝났다 (아래 "디자인 측 완료"). 앱 티켓이 HANDOFF 와 어긋나는 걸 찾으면 코드가 아니라 메인이 사용자에게 알리고 HANDOFF 를 고친다 (AGENTS.md 머리말).

## 사용자가 정할 것

- **확장안 2차 범위.** 캔버스 "사무실 확장안"·"직원 확장안" 중 새 방 4개(회의실·자료실·경비실·옥상), 자세 +5, 담당 +4, 소지품 +7 은 평면도와 생성기를 크게 바꾼다 (T94). 권장: 0.2.0 은 크롬 v2 + 야근 모드 + 조명 저녁 단계 + 감정·근속 표식 + 소품 확장 1차까지. 새 방·자세·담당·소지품은 0.3.0.
- **야근 모드 자동 켜짐 기본값.** 20시 이후 자동(권장) / 수동만.

## 티켓

번호는 `PLAN_update.md` 의 T79–T82 다음부터. 등급은 AGENTS.md 3절.

### T83 tokens.css 적용 확인 (대리 · 하)

- 범위: 읽기만. `src/styles/*.css`, `src/*.ts` 에서 `design/handoff/tokens.css` 값을 덮어쓰거나(`#fff`, 고정 padding) v2 에서 이름이 바뀐 클래스를 쓰는 곳을 목록으로.
- 완료 기준: 파일·줄·현재 값·v2 값 표. `npm run check` 녹색. 코드는 고치지 않는다.

### T84 사이드바 v2 (과장 · 중)

- 범위: `src/sidebar-left.ts`(+test), `src/styles/sidebar.css`.
- 할 일: HANDOFF 4.1 표대로. 간판 타일 + 회사 이름 첫 줄(설정 `companyName`, 없으면 `phrases.app.brand`), 지표 띠 `ow-kpi`(불 켜진 사무실/층 · 근무 중 직원 · 결재 대기), 행 여백(`ow-row__main` 12/4/12/18, 점 위 4, 상세 버튼 위 8), `ow-worktype.is-idle` 대기 중 칩 + `quietFor`, `ow-host` 테두리형 WSL 배지, `is-broken` 파싱 실패 색, 바닥 상태 점, 레일 `ow-rail__item` 32 타일(허가 대기 `--ow-ask` 바탕). tokens.css 의 `.ow-rail__letter` 와 ponytail 주석을 지운다.
- 완료 기준: 지표 숫자가 세션 상태에서 계산되는 테스트(사무실 n·근무 중 n·결재 n), 기존 사이드바 테스트 통과, 1600·960·800 폭에서 HANDOFF 2.2 와 같음.

### T85 머리말 v2 (대리 · 하)

- 범위: `src/header.ts`(+test), `src/styles/app.css`.
- 할 일: HANDOFF 4.3. 세그먼트 묶음 `ow-segment`(방 이름·소리·야근 토글. 야근 토글은 T90 이 동작을 붙이기 전까지 `disabled`), "결재 n건 대기" 칩 `is-ask`, 배율 칩 `is-zoom` + 1×·2×·3× 메뉴(들어가지 않는 배율은 비활성), 무대 모서리 `ow-stage__tools`(가운데로·전체 화면 = Tauri 창 최대화 토글). 문구는 `phrases.header`.
- 완료 기준: 토글 `aria-pressed`, 배율 메뉴 테스트, `npm test` 녹색.

### T86 직원 카드 v2 (과장 · 중)

- 범위: `src/sidebar-right.ts`(+test), `src/avatar.ts`(핀 자리), `src/styles/panel.css`.
- 할 일: HANDOFF 4.5 해부·변형 표대로. `ow-avatar-wrap` + `ow-pin`, 직급 칩(부장 `is-boss`), 담당 줄 말줄임, 컨텍스트 막대 `ow-ctx__bar`(5칸 눈금 삭제, tokens.css ponytail 주석 삭제), 행적 `ow-trail__item` 타임라인(반려 `is-bad`), 실적 숫자 `<b>`, 호버 액션 `ow-card__actions`(직원 상세), 허가 대기 `is-ask` 변형(버튼 2개: 세션 상세 열기 / 세션으로 이동 = 사이드바 행 선택), 외근 `is-away`, 압축형 9/12, 팀 머리글 줄바꿈 금지, 현황판 머리 `sort` 버튼(기본 순서 ↔ 이름순).
- 재현: `fixtures/claude/*` replay 로 Cards 보드의 9가지 변형.
- 완료 기준: 변형마다 DOM 테스트 1개 (클래스·칩 문구), 기존 테스트 통과, 창 높이 640 미만 압축형.

### T87 무대 v2 (대리 · 하)

- 범위: `src/styles/app.css`, `src/office/renderer.ts` 또는 `src/main.ts` 의 무대 DOM, `src/office/canvas-tooltip.ts`(방 이름표).
- 할 일: 비네트 `ow-stage::before`, 바닥 그림자 `ow-stage__floor`, 유리 방 이름표(tokens.css `ow-roomlabel` 값), 캔버스 `drop-shadow`.
- 완료 기준: 125%·150% DPR 에서 도트 경계 선명(HANDOFF 12절), 2× 와 3× 에서 그림자가 캔버스 밖 무대에만 그려짐. **drop-shadow 필터 프레임 시간을 재서 기존 대비 +1ms 를 넘으면 필터를 빼고 바닥 그림자만 둔다** (tokens.css ponytail 주석 정리). 측정값을 커밋 메시지에.

### T88 티커 v2 (대리 · 하)

- 범위: `src/ticker.ts`(+test), `src/styles/panel.css`.
- 할 일: 항목 `ow-ticker__item`(시각·팀 태그·이름·요약), 마우스 올리면 `is-paused` + 그 항목 회색, 조용할 때 회색 칩 + "조용한 밤입니다", `prefers-reduced-motion` 에서 정지.
- 완료 기준: 호버 정지·재개 테스트, 기존 테스트 통과.

### T89 직원 상세 팝업 v2 (과장 · 중)

- 범위: `src/popup-employee.ts`(+test), `src/styles/popup.css`.
- 할 일: HANDOFF 4.6 직원 상세 행대로. 머리 칩 줄(담당·모델·방·컨텍스트 막대) + 근무 상태 칩 + `open-external` 원본 열기, 타임라인 `ow-tl` 접힘 38/펼침 `ow-tl__raw`, 실적 요약 `ow-stat` 3묶음. 팝업 공통 여백(머리 20, 본문 좌우 20, 버튼 28)은 `ow-modal`·`ow-btn` 값이라 세션 상세·설정도 같이 바뀐다 → 두 팝업의 테스트가 깨지면 기대값이 아니라 마크업을 본다.
- 완료 기준: 펼침·접힘·더 보기 테스트, 세션 상세·설정 팝업 회귀 없음.

### T90 야근 모드 (과장 · 중)

- 범위: `src/main.ts`, `src/header.ts`(토글 동작), `src/popup-settings.ts`(+test), `src/store.ts`, `src-tauri/src/settings.rs`(설정 키 `nightMode: "auto" | "on" | "off"`, 기본 `auto`) + IPC 타입 재생성(docs/BUILD.md).
- 할 일: HANDOFF 4.8. `<html data-theme="night">` 토글, `auto` 는 로컬 시계 20시–06시, 전환 `--ow-dur-theme`, 6.2절 조명과 독립. 설정 "화면과 소리" 절에 세그먼트 3개(자동·켜짐·꺼짐).
- 완료 기준: 세 값 각각 DOM 테스트, `cargo test` 설정 round-trip, 야근 모드에서 글자 대비 4.5:1 표(사이드바·카드·팝업·티커의 글자 색 × 바탕 색, 계산값) 를 PR 에 첨부.

### T91 조명 저녁 단계 (대리 · 하)

- 범위: `design/sprites/floorplan.json` `lightTints`, `design/tools/owlfice_art.py`(참조 렌더), `src/office/renderer.ts`(+test), `design/assets/office-*-evening.png`(새 참조 1장), HANDOFF 6.2절 7번 한 줄.
- 할 일: 저녁 `#F6E6C4` (17–20시), 야근은 그대로 `#F2DEAD` + 비네트 레이어(`overlay`, 책상 램프 주변만 밝음). 캔버스 "사무실 확장안" 윗줄.
- 완료 기준: 참조 렌더 픽셀 비교(6.7절) 저녁 1장 추가 통과.

### T92 감정 표식 · 근속 표식 (과장 · 중)

- 범위: `src/office/canvas-tooltip.ts` 또는 새 오버레이 모듈(감정 표식 DOM `ow-emote`), `design/tools/owlfice_art.py` + `design/sprites/owls.json`(근속 `wear/<단계>/` 레이어), `src/office/actors.ts`, HANDOFF 5절 표에 행 추가, 4.4절에 `ow-emote` 행.
- 할 일: 캔버스 "직원 확장안" 의 감정 표식 6종(! ? ♪ … ✕ ✓, 말풍선 틀, 1.2초, 말풍선이 있으면 생략)과 근속 표식 3단계(1h 넥타이 느슨·소매, 3h 커피 2잔·눈 밑, 6h 수건 + 말풍선). 계기는 모두 세션 파일에서 확인되는 사건만 (C13).
- 완료 기준: replay 로 사고·허가 대기·테스트 통과·반려·승인 각 1회에 표식이 뜨고 1.2초 뒤 사라지는 테스트, 근속 레이어가 `session age` 로 바뀌는 테스트, 참조 렌더 갱신.

### T93 소품 확장 1차 (상 · 메인이 뼈대, 그림은 대리)

- 범위: `design/tools/owlfice_art.py`, `design/sprites/office.json`·`office.png`, `src/office/props.ts`(+test), `src/office/renderer.ts`, HANDOFF 5절.
- 할 일: 캔버스 "사무실 확장안" 의 기존 방 표 중 **이미 파싱되는 신호만**: 팀장석 커피 잔(근속 1h 마다, 최대 4)·책상 램프(야근), 개발실 서류 더미(수정 파일 10개마다)·모니터 2대(병렬), 소스 서고 책장 채움(읽은 파일 수), 출고장 상자 쌓임(커밋 수)·트럭 색(PR 열림/머지), 빌드실 CI 전광판(오늘 성공/실패). 메인이 신호→소품 표를 HANDOFF 5절에 먼저 적고, 대리가 생성기와 렌더러를 나눠 맡는다.
- 완료 기준: 소품마다 replay 테스트 1개, 참조 렌더 4장 갱신·비교 통과.

### T94 확장안 2차 — 사용자 결정 뒤 (0.3.0 후보)

- 새 방 4개(평면도·통로·좌석 변경), 자세 +5(생성기 프레임 약 60), 담당 +4, 소지품 +7, 계절 장식. 캔버스 두 보드의 "새로" 꼬리표 항목.
- 결정 전에는 손대지 않는다.

### T95 접근성 · 움직임 줄이기 점검 (대리 · 하)

- 범위: `src/*.ts` 의 `aria-*`, `src/styles/*.css` 의 `prefers-reduced-motion`.
- 할 일: HANDOFF 10절 + v2 추가분 — 세그먼트 `aria-pressed`, 핀 `aria-label`(상태 단어), 카드 호버 액션 키보드 도달(`focus-within`), 티커 정지, 야근 모드 대비 표(T90 결과 재확인).
- 완료 기준: 체크리스트 12절 전 항목에 ✓ 또는 사유.

### T96 스크린샷 · 버전 · 릴리스 (메인 + 사용자)

- `docs/images/office.png`·`employee.png` 를 v2 로 재촬영(합성 데이터, 1600×960 주간 1장 + 야근 1장 추가), `README.md` 캡션. `tauri.conf.json`·`package.json` 0.2.0. 릴리스는 `PLAN_update.md` T81 절차. 끝나면 이 문서와 `PLAN_update.md` 삭제, HANDOFF 0절 캔버스 링크 확인.

## 순서

T83 → (T84 ‖ T85 ‖ T88) → T86 → T87 → T89 → T90 → T91 → T92 → T93 → T95 → `PLAN_update.md` T79 → (T80 ‖ T81) → T96 → T82. T94 는 결정 뒤.

쓰는 서브는 같은 파일에 둘을 두지 않는다 (AGENTS.md 3절). T84·T85·T88 은 파일이 겹치지 않아 동시 가능. T86 과 T89 는 `panel.css`·`popup.css` 가 다르지만 `avatar.ts` 를 같이 보므로 순서대로.

## 디자인 측 완료 (이 브랜치, 2026-10-22)

- `design/project/`: v2 19장 + `canvas.json` (캔버스 "Owlfice 디자인 0.2.0" 과 같은 파일). 0.1.0 의 `Main`·`MainSmb`·`Components` 는 v2 `Main`·`MainNight`·`Cards`/`Sidebar`/`Header`/`Tokens` 로 대체. 중소기업 주간 보드는 없다 (야근 보드가 중소기업 구조를 보여 준다).
- `design/handoff/tokens.css`: v2 토큰(`--ow-surface`, `--ow-elev-*`, `--ow-radius-tag`, 금색·외근·야근 칩 색), 컴포넌트 규칙 v2 값, `[data-theme="night"]` 블록. 앱이 아직 쓰는 0.1.0 규칙(`.ow-ctx i` 눈금, `.ow-rail__letter`)은 `ponytail` 주석으로 남겼다 → T86·T84 가 지운다.
- `design/handoff/phrases.ko.json` 키 19개(sidebar 5·header 6·card 8), `design/handoff/icons/` 4개(center·fullscreen·sort·menu).
- `design/HANDOFF.md` 0·1·2.1·3·4.1–4.6·4.8(새 절)·7·12절.
- 검증: 19장을 로컬 렌더해 넘침·개행·겹침 자동 검사 통과 (티커 흐름과 한 글자 고정 상자는 의도된 것). 1:1 눈 검사는 v2 8장만 했고, 이어받은 11장은 DOM 측정(여백·구조)으로 확인했다.
