# 0.2.0 남은 작업

통합 기준은 `v0.2.0`이다. 사용자가 티켓별 구현·검증·커밋·통합을 승인했으나 **main 병합 직전에 멈춘다**. main 병합·정식 릴리스·배포는 실행하지 않는다. 두 계획 파일은 T82-3 뒤에만 지운다.

## 진행

T83·T92·T93·T84·T85·T86·T87·T88·T89·T90·T91·T94·T95·T96·T97·T98·T99·T100·T102·T79를 통합했다. 남은 순서는 T80·T81 → T82-1 → T101 → main 병합 전 정지다.

- T80은 `t80-update-ui`에서 설정의 업데이트 상태·설치 진행·시작 토스트를 구현한다.
- T81은 `t81-release-tools`에서 릴리스 manifest·검사 스크립트와 BUILD 절차를 구현한다.
- T79는 키 없는 빌드가 서명 단계에서 실패하고, 올바른 키로 설치 파일과 `.sig`가 생성됨을 확인했다. 서명 trusted comment에 실제 버전이 있어 `requireSignedVersion`을 켰다. 기존 의존성 버전은 바뀌지 않았다.
- T79 통합 전 메인이 Rust 490개와 프론트 검사를 직접 통과시켰다. 화면 통합의 전체 프론트 498개와 실제 화면 검수 범위는 HANDOFF 12절이 기준이다.

## 업데이트 검증 준비

검증 저장소 `dbwk10317/owlfice-updater-test`는 private으로 유지한다. 공개 상태였을 때 README만 있었고 릴리스·설치 파일·키는 없었다. 앱에 GitHub 토큰을 넣지 않는다. 인증된 CLI로 시험 자산을 내려받아 임시 로컬 서버로 설치·서명·실패 경로를 검사한다. 공개 GitHub HTTPS 전송·redirect 경로의 실전 검증은 출시 뒤 T82-3에 남긴다.

키는 저장소 밖 `%USERPROFILE%/.tauri/owlfice.key`에 암호화해 보관한다. 비밀번호는 같은 폴더의 `owlfice.password.dpapi`에서 현재 Windows 계정으로 복호화한다. 공식 배포 전 별도 백업 확인은 남아 있다.

시험 앱은 override JSON으로 이름 `Owlfice Updater QA`, 식별자 `com.owlfice.updaterqa`, currentUser 설치 모드를 쓴다. 별도 임시 설치 경로와 APPDATA를 사용하며 비승격 상태로 실행한다. NSIS의 비승격 재시작은 직접 ShellExecute 분기지만, 자동 재시작 뒤 실제 설정 유지까지 확인해야 한다. 정식 설정에는 로컬 endpoint나 HTTP 예외를 합치지 않는다.

시험 자료 위치는 `%TEMP%/owlfice-updater-qa-current.txt`가 가리킨다. `configs/`에 test.1/test.2 override, `server.cjs`에 127.0.0.1 전용 서버가 있다. `server-mode.json`으로 정상·서명 불일치·없는 파일·없는 manifest와 느린 다운로드를 선택한다. `server-requests.jsonl`의 설치 파일 GET 횟수로 중복 설치를 검사한다. 실제 자산을 받기 전이며 최종 서버는 아직 실행하지 않았다.

## 화면 검수에서 지킬 것

- 실제 Windows 앱은 임시 APPDATA와 합성 세션만 사용한다. 실제 화면 검수 자료 위치는 `%TEMP%/owlfice-visual-qa-current.txt`가 가리킨다.
- 실제 OS 배율은 150%다. DPR만 바꾸는 CDP emulation은 device-pixel-content-box와 devicePixelRatio가 어긋났으므로 그 캔버스 크기는 제품 버그 증거로 쓰지 않는다. 실제 배율을 유지한 크기 행렬과 이름표만의 배율 검사를 구분한다.
- 폭 720 미만은 제품 최소 폭 밖이다. 깨끗한 Windows VM 설치와 공개 배포 경로 확인을 수행했다고 보고하지 않는다.

임시 정리: 메인이 잘못 지정한 `D:/workspace/owlfice-wt/t91-daylight/src-tauri/t91-target` 빌드 폴더 삭제가 자동 승인 검토에서 정책상 거부됐다. 커밋에는 포함하지 않았으며 재시도하지 않는다.
