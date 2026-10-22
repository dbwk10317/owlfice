# 앱 내 자동 업데이트 (Tauri updater) 계획

설치된 Owlfice 가 GitHub Releases 의 새 버전을 스스로 찾아 설치하게 한다. 끝나면 이 파일은 지우고, 남길 절차는 docs/BUILD.md 릴리스 행으로, 동작은 코드 주석으로 옮긴다 (AGENTS.md 7절).

## 전제와 결정

- **서버·유료 인증서 없음.** 업데이트 목록(`latest.json`)과 설치 파일은 지금처럼 GitHub Releases 에 올린다. 서명은 Tauri 내장 minisign 키 쌍이며 코드 서명 인증서(SmartScreen)와 무관하다. SmartScreen 경고는 이 작업으로 사라지지 않는다.
- **업데이터는 "들어 있는 버전부터" 동작한다.** 0.1.x 에는 없으므로 0.1.x → 다음 버전은 여전히 설치 파일을 직접 받아 덮어 깐다. 자동 업데이트가 처음 동작하는 건 업데이터가 든 버전에서 그다음 버전으로 갈 때다. 그래서 **0.2.0 에 포함해 내보내는 것을 권한다** (v0.2.0 워크트리 `D:/workspace/owlfice-wt/v0.2.0` 에서 디자인 작업 중. 이 계획의 티켓은 그 브랜치 위에서, 또는 main 에 먼저 합치고 v0.2.0 이 받는다. 어느 쪽인지는 사용자가 정한다).
- **새 의존성 (사용자 허락 필요, AGENTS.md 1절):** Rust `tauri-plugin-updater`, `tauri-plugin-process` / npm `@tauri-apps/plugin-updater`, `@tauri-apps/plugin-process`. 둘 다 Tauri 공식 플러그인이다 (확인됨: https://v2.tauri.app/plugin/updater/ , https://v2.tauri.app/plugin/process/ ).
- **CSP 는 안 바꾼다.** 확인·다운로드는 Rust 쪽(플러그인)이 하므로 webview `connect-src` 에 GitHub 를 넣을 필요가 없다 (미확인: 실제 빌드에서 확인하고 틀리면 여기 고친다).
- **Windows 한계 (확인됨):** 설치 단계에서 앱이 자동 종료된다. 설치 프로그램(NSIS)이 기존 버전을 제거하고 새로 깐 뒤 사용자 데이터는 남는다. `installMode` 는 기본값 `passive` (작은 진행 창, 사용자 입력 없음).

## 0. 사람이 하는 일 (메인이 사용자에게 부탁)

1. 서명 키 생성. 저장소 밖에 둔다.
   ```bash
   npm run tauri signer generate -- -w ~/.tauri/owlfice.key
   ```
   비밀번호를 물으면 정한다 (빈 값도 되지만 정하는 쪽을 권한다). 출력된 **공개 키 문자열**을 메인에게 준다. 개인 키 파일(`~/.tauri/owlfice.key`)과 비밀번호는 **백업**한다. 잃으면 이미 설치된 버전들에 업데이트를 보낼 수 없고, 사용자가 다시 설치 파일을 직접 받아야 한다.
2. 빌드 머신 환경변수. `.env` 파일은 안 된다 (확인됨).
   - `TAURI_SIGNING_PRIVATE_KEY` = 키 파일 경로 또는 내용
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = 비밀번호 (정했으면)
3. 아래 "해석이 갈리는 것" 두 가지에 답한다.

## 해석이 갈리는 것 (코드 치기 전에 사용자 답을 받는다)

- **A. 알림 자리.** 새 버전을 찾았을 때 사용자가 어디서 아는가.
  - A1 (권장, 최소): 설정 팝업에 "업데이트" 절 추가 (현재 버전, "새 버전 확인" 버튼, 찾으면 "x.y.z 설치하고 다시 시작" 버튼). 시작 때 한 번 조용히 확인하고, 있으면 기존 왼쪽 목록 토스트로 한 줄 ("새 버전 x.y.z · 설정에서 설치").
  - A2: 간판(`ow-brand`) 에 점·배지로 표시. design/HANDOFF.md 간판 규격 수정이 필요해 디자이너 몫이 생긴다.
  - 어느 쪽이든 design/HANDOFF.md 209행(설정 팝업 구성)에 "업데이트" 절을 한 줄 추가한다. v0.2.0 디자인 작업과 같은 파일을 만지므로 순서를 맞춘다.
- **B. 검증 방법.** 프로덕션 빌드는 TLS 를 강제하므로 로컬 http 서버로는 못 본다.
  - B1 (권장): 0.2.0 을 업데이터 포함으로 실제 릴리스 → 작은 수정으로 0.2.1 릴리스 → 설치된 0.2.0 이 0.2.1 로 올라가는지 본다. 첫 실전이 곧 검증이다.
  - B2: endpoint 를 임시로 테스트 저장소의 `raw.githubusercontent.com` 주소로 바꾼 빌드 두 개로 먼저 확인. 수고가 더 들고 설정 되돌리기를 잊을 위험이 있다.

## 티켓

번호는 T77 다음부터 임시로 붙였다. 메인이 실제 배정 때 바꾼다. 브랜치·PR 규칙은 docs/BUILD.md 저장소 규칙.

### T79 플러그인 연결 (대리 · 난이도 하)

- 범위: `src-tauri/Cargo.toml`, `package.json`, `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json`, `src-tauri/tauri.conf.json`.
- 할 일:
  - 의존성 추가 (`npm run tauri add updater`, `npm run tauri add process` 가 Cargo·npm·lib.rs 를 같이 손댄다. 결과를 읽고 아래와 맞춘다).
  - `lib.rs` `run()` 에 `.plugin(tauri_plugin_updater::Builder::new().build())`, `.plugin(tauri_plugin_process::init())`. 기존 notification 플러그인 줄 옆.
  - `capabilities/default.json` permissions 에 `"updater:default"`, `"process:allow-restart"` (exit 는 안 쓰므로 `process:default` 대신 restart 만).
  - `tauri.conf.json`:
    ```json
    "bundle": { "createUpdaterArtifacts": true },
    "plugins": {
      "updater": {
        "pubkey": "<0 단계에서 받은 공개 키 문자열. 파일 경로 불가>",
        "endpoints": ["https://github.com/dbwk10317/owlfice/releases/latest/download/latest.json"]
      }
    }
    ```
    `createUpdaterArtifacts: true` 면 Windows 에서 설치 파일 `Owlfice_<버전>_x64-setup.exe` 옆에 `.sig` 가 나온다 (`"v1Compatible"` 은 쓰지 않는다. v3 에서 없어진다).
- 완료 기준: `npm run tauri build` 가 `.exe` 와 `.exe.sig` 를 만든다 (환경변수 없이 빌드하면 서명 단계에서 실패하는 것이 정상. 그 오류 메시지를 보고에 적는다). `cargo test`, `npm run check` 녹색.
- 금지: 프론트 코드, 문서, 다른 플러그인 정리.

### T80 업데이트 확인 UI (과장 · 난이도 중) — T79 뒤, A 답 뒤

- 범위: `src/popup-settings.ts`(+test), `src/main.ts`, `design/handoff/phrases.ko.json` settings 절, A1 이면 토스트를 내는 자리(`src/sidebar-left.ts` 의 기존 토스트 경로를 쓴다. 새 토스트 체계 금지).
- 할 일:
  - `check()`(`@tauri-apps/plugin-updater`)와 `relaunch()`(`@tauri-apps/plugin-process`)는 `SettingsDeps` 처럼 **주입**한다. 테스트는 가짜 check 로 "없음 / 있음 / 실패 / 다운로드 중 / 설치 직전" 상태를 jsdom 에서 본다. 네트워크·실제 플러그인을 테스트에서 부르지 않는다.
  - 설정 팝업 "업데이트" 절: 현재 버전(`@tauri-apps/api/app` 의 `getVersion()`), "새 버전 확인" 버튼, 결과 문구, 찾으면 "설치하고 다시 시작" 버튼. 설치 중에는 버튼을 잠그고 `downloadAndInstall` 의 Progress 를 문구로 보여 준다. 끝나면 `relaunch()`. Windows 는 설치 단계에서 앱이 먼저 꺼질 수 있으므로 relaunch 실패를 오류로 보여 주지 않는다 (주석으로 이유를 남긴다).
  - 시작 때 확인은 사무실이 뜬 뒤 한 번, 실패는 조용히 무시한다 (오프라인이 정상 상황이다). 사용자가 끌 수 있는 설정은 **만들지 않는다** (필요해지면 그때).
  - 문구는 모두 `phrases.ko.json` 에 둔다. 기존 톤(~해요)을 따른다.
- 완료 기준: 위 상태별 테스트 통과, `npm test`·`npm run check` 녹색, `npm run tauri dev` 에서 "새 버전 확인" 을 누르면 현재 릴리스와 비교한 결과 문구가 뜬다 (dev 는 버전이 같거나 낮으니 "최신입니다" 가 정상).
- 금지: 자동 다운로드(사용자가 누르기 전에 받지 않는다), 설정 스키마(`settings.rs`) 변경, HANDOFF 밖의 새 화면.

### T81 릴리스 절차 (대리 · 난이도 하) — T79 뒤

- 범위: `tools/release_manifest.py`(새 파일), `docs/BUILD.md` 릴리스 행, `build-gnu.sh` 는 서명 환경변수가 있으면 그대로 동작하므로 손대지 않는다 (미확인: GNU 빌드에서 `.sig` 가 나오는지. 실측 뒤 틀리면 BUILD.md 폴백 절에 한 줄).
- 할 일:
  - `tools/release_manifest.py <버전> <setup.exe 경로>` → 옆의 `.sig` 를 읽어 `latest.json` 을 표준 출력 또는 지정 경로에 쓴다. 표준 라이브러리만. 형식:
    ```json
    {
      "version": "0.2.1",
      "notes": "",
      "pub_date": "2026-10-22T00:00:00Z",
      "platforms": {
        "windows-x86_64": {
          "signature": "<.sig 파일 내용 그대로>",
          "url": "https://github.com/dbwk10317/owlfice/releases/download/v0.2.1/Owlfice_0.2.1_x64-setup.exe"
        }
      }
    }
    ```
    `version` 은 `v` 없는 semver, `url` 은 `latest` 가 아니라 **태그 고정** 주소 (다음 릴리스가 올라가도 이 파일이 가리키는 파일은 안 바뀌게). `uvx ruff check tools`, `uvx ruff format --check tools` 녹색.
  - docs/BUILD.md 릴리스 행을 이렇게 바꾼다 (한 사실 한 곳: 환경변수 이름은 여기만):
    1. `TAURI_SIGNING_PRIVATE_KEY`(·`_PASSWORD`) 를 둔 셸에서 `npm run tauri build`
    2. `python tools/release_manifest.py <버전> src-tauri/target/release/bundle/nsis/Owlfice_<버전>_x64-setup.exe > latest.json` (임시 폴더에)
    3. `gh release create v<버전> <setup.exe> <setup.exe.sig> latest.json`
    - **`latest.json` 을 빠뜨리거나 prerelease·draft 로 올리면** 설치된 앱이 새 버전을 못 본다 (`releases/latest/download/` 는 정식 최신 릴리스만 가리킨다). 이 한 줄을 BUILD.md 에 남긴다.
  - `tauri.conf.json`·`package.json` 버전 올리기는 지금 절차 그대로.
- 완료 기준: 0.1.1 설치 파일과 가짜 `.sig` 로 스크립트를 돌려 위 형식이 나온다. BUILD.md 행이 위 3단계를 담는다.
- 금지: GitHub Actions 추가 (릴리스는 로컬 빌드다), README 링크 변경 (`releases/latest/download/Owlfice_<버전>_x64-setup.exe` 는 그대로 쓴다).

### T82 실전 검증 (메인 + 사용자) — B 답대로

- B1: 0.2.0 릴리스(T79~T81 포함) → 설치 → 0.2.1 릴리스 → 설치된 0.2.0 에서 시작 때 토스트 또는 설정의 "새 버전 확인" → 설치 → 재시작 후 설정의 현재 버전이 0.2.1. 사용자 데이터(규모·이름·직접 지정 경로)가 남아 있는지 본다.
- 실패하면 AGENTS.md 6절대로 원인부터. 흔한 원인: `latest.json` 의 `signature` 에 경로를 넣음, 공개 키 불일치, 릴리스가 prerelease, `pub_date` 형식.
- 통과하면 이 문서를 지우고 BUILD.md 의 **(미확인)** 표시를 뗀다.

## 순서

0 (키) → T79 → (T80 ‖ T81, 같은 파일을 안 만지므로 동시 가능) → 0.2.0 릴리스 → T82.
