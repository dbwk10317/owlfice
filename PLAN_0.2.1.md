# 0.2.1 계획: 직원 스프라이트 v3 (해상도 · 표정 · 강아지·고양이)

시안은 캔버스 [Owlfice 직원 스프라이트 v3](https://claude.ai/artifact/LKTWr2Pc9sTH8PBdb5dt8N) (원본 `design/project/Employee*.dc.html`). 디자인 쪽(`design/`)은 이 브랜치에 반영했고 앱(`src/`, `src-tauri/`)은 아직 16×24 올빼미 한 장을 읽는다. 이 문서는 앱을 새 시트로 옮기는 티켓 목록이다. 끝나면 지운다. 남길 결론은 `design/HANDOFF.md` 6.3·6.4 와 `docs/CONTRACTS.md` 로 옮긴다 (AGENTS.md 7절).

## 결정된 것 (사용자, 2026-10-22)

- 프레임 밀도 2(32×48)와 3(48×72) 두 벌. 월드 크기 16×24·좌석·평면도·기준점 의미는 그대로. 배율 1·2는 2× 시트, 배율 3은 `@3x` 시트. 카드 아바타는 항상 2×.
- 동물은 사무실 전체가 하나. 설정 "직원 동물" (올빼미·강아지·고양이, 기본 올빼미)로 시트를 고른다. 한 사무실에 섞지 않는다.
- 자세·fps·레이어·근속·담당·소지품·평면도는 바꾸지 않는다. 표정은 자세별 얼굴 상태(`owlfice.faces`)로만 더한다.
- 브랜치 `v0.2.1` (main 에서), 워크트리 `D:/workspace/owlfice-wt/v0.2.1`. 티켓 브랜치는 여기서 따고 여기로 합친다. main 병합·push 는 사용자 지시 때.

## 티켓

### T104 생성기 밀도·동물 일반화 (과장 · 상) — 디자인 쪽
`design/tools/owlfice_art.py`. 시트 6장, 메타(`density`·`animal`·`bossSpecies`·`faces`·`eyeBoxes`), 미리보기, 참조 렌더(1× 축소), 검사 확장. 완료: 생성기 실행이 검사를 통과하고 여섯 시트 키 집합이 같다.

### T105 렌더러 밀도·시트 선택 (과장 · 중) — T104 뒤
`src/office/sprites.ts`, `src/office/actors.ts`, `src/office/renderer.ts`. 시트 경로 = 설정 동물 + 캔버스 배율(3이면 `@3x`). 직원 스프라이트 `scale = 1/density`, `anchors.head/hand` 를 `density` 로 나눠 월드 좌표로 (headAt · held · stamp · sweat 가 한 헬퍼). 사장님 종 = `meta.bossSpecies`. `hitArea`·좌석·말풍선 기준점은 월드 단위 그대로. 배율이 바뀌면 시트를 다시 고르고 로드 중엔 이전 시트 유지. 완료: `dev/ref` 픽셀 비교 녹색, 배율 1·2·3 에서 직원이 같은 자리·같은 크기.

### T106 카드 아바타 (대리 · 하) — T105 뒤
`src/avatar.ts`. `meta.frame` 검사, 2× 시트 `idle_f_0` 위 4줄을 잘라 40×44 원본, 직원 상세 2배. 동물이 바뀌면 캐시를 비운다. 완료: 카드·직원 상세·세션 상세 아바타가 HANDOFF 6.4 와 같다.

### T107 설정 "직원 동물" (대리 · 중) — T105 와 병렬
`src-tauri/src/settings.rs` 필드 `animal: "owls"|"dogs"|"cats"` (기본 `owls`, 알 수 없는 값은 기본값), `src/popup-settings.ts` 회사 그룹 세그먼트, `design/handoff/phrases.ko.json` 문구, `docs/CONTRACTS.md` 설정 문단에 한 줄. 완료: 바꾸면 저장되고 시트 재로드·아바타 갱신, 재시작 후 유지.

### T108 검수 (메인 + 사용자) — 위 전부 뒤
실제 창에서 배율 1·2·3 각각 표정·자세 가독, 야근 모드 조명 곱하기 아래의 얼굴 대비, HANDOFF 12절 항목 재확인. 끝나면 이 문서를 지운다.

## 순서

T104 → T105 → (T106 ‖ T107) → T108 → 이 문서 삭제.
