# 앱 내 자동 업데이트 (Tauri updater) 계획

설치된 Owlfice 가 GitHub Releases 의 새 버전을 스스로 찾아 설치하게 한다. 0.2.0 에 포함한다 (사용자 결정 2026-10-22). 끝나면(T82 뒤) 이 파일은 지우고, 남길 절차는 docs/BUILD.md 릴리스 행으로, 동작은 코드 주석으로 옮긴다 (AGENTS.md 7절). 전체 순서는 `PLAN_0.2.0.md` 4절.

## 전제와 결정

- **서버·유료 인증서 없음.** 업데이트 목록(`latest.json`)과 설치 파일은 지금처럼 GitHub Releases 에 올린다. 서명은 Tauri 내장 minisign 키 쌍이며 코드 서명 인증서(SmartScreen)와 무관하다. SmartScreen 경고는 이 작업으로 사라지지 않는다.
- **업데이터는 "들어 있는 버전부터" 동작한다.** 0.1.x 에는 없으므로 0.1.x → 0.2.0 은 설치 파일을 직접 받아 덮어 깐다. 자동 업데이트가 처음 동작하는 건 0.2.0 → 다음 버전이다. 그래서 **정식 0.2.0 을 내보내기 전에** 서명된 빌드 두 개로 업데이트가 실제로 되는지 검증한다 (아래 B). 미검증 업데이터를 배포하면 고장 난 업데이터로 업데이터를 고쳐야 한다.
- **새 의존성 (사용자 허락 필요, AGENTS.md 1절):** Rust `tauri-plugin-updater` / npm `@tauri-apps/plugin-updater` 만. Windows 전용 앱이고 Windows 에서는 설치 프로그램이 앱을 스스로 종료·재시작하므로 `process` 플러그인(`relaunch()`)은 **넣지 않는다** (확인됨: https://v2.tauri.app/plugin/updater/#windows-before-exit-hook ). 뒤에 다른 OS 를 지원하면 그때 더한다.
- **CSP 는 안 바꾼다.** 확인·다운로드는 Rust 쪽(플러그인)이 하므로 webview `connect-src` 에 GitHub 를 넣을 필요가 없다 (미확인: B 검증에서 확인하고 틀리면 여기 고친다).
- **Windows 한계 (확인됨):** 설치 단계에서 앱이 자동 종료된다. NSIS 가 기존 버전을 제거하고 새로 깐 뒤 사용자 데이터는 남는다. `installMode` 는 기본값 `passive`.
- **외부 네트워크 예외:** HANDOFF 12절 "외부 네트워크 요청이 없다"의 예외는 업데이트 확인(`latest.json` GET)과 사용자가 누른 뒤의 설치 파일 다운로드 두 가지뿐이다. 둘 다 Rust 플러그인이 하고, 세션 내용은 보내지 않는다 (CONTRACTS C1). 12절 문구는 고쳐 두었다.

## 0. 사람이 하는 일 (메인이 사용자에게 부탁)

1. 서명 키 생성. 저장소 밖에 둔다.
   ```bash
   npm run tauri signer generate -- -w ~/.tauri/owlfice.key
   ```
   비밀번호를 물으면 정한다. 출력된 **공개 키 문자열**을 메인에게 준다. 개인 키 파일과 비밀번호는 **백업**한다. 잃으면 이미 설치된 버전들에 업데이트를 보낼 수 없다.
2. 빌드 머신 환경변수. `.env` 파일은 안 된다 (확인됨).
   - `TAURI_SIGNING_PRIVATE_KEY` = 키 파일 경로 또는 내용
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = 비밀번호
3. 검증용 공개 저장소 하나 (또는 이 저장소에 `updater-test` 용 별도 릴리스를 올릴 권한). B 절.

## 결정

- **A. 알림 자리 = A1.** 설정 팝업에 "업데이트" 절(현재 버전, "새 버전 확인" 버튼, 찾으면 "x.y.z 설치" 버튼). 시작 때 사무실이 뜬 뒤 한 번 조용히 확인하고, 있으면 기존 왼쪽 목록 토스트로 한 줄 ("새 버전 x.y.z · 설정에서 설치"). 간판 배지(A2)는 쓰지 않는다. `design/HANDOFF.md` 4.6절 설정 행에 "업데이트" 절 한 줄을 T80 이 더한다 (v2 디자인 반영은 끝났으므로 충돌 없음).
- **B. 검증 = 정식 출시 전, 서명된 빌드 두 개로.** 프로덕션 빌드는 TLS 를 강제하지만 `https://` GitHub 주소면 사전 검증에 문제가 없다. 원본 설정 파일을 고쳤다 되돌리지 않고 `npm run tauri build -- --config <덮어쓸 JSON 경로>` 로 endpoint·버전만 덮어쓴다 (확인됨: https://v2.tauri.app/reference/cli/#build ). 절차는 T82.

## 티켓

번호는 T77 다음. 브랜치·PR 규칙은 docs/BUILD.md 저장소 규칙. 범위에 적힌 파일만 고친다.

### T79 플러그인 연결 (대리 · 난이도 하)

- 범위: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `package.json`, `package-lock.json`, `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json`, `src-tauri/tauri.conf.json`.
- 할 일:
  - `npm run tauri add updater` (Cargo·npm·lib.rs 를 같이 손댄다. 결과를 읽고 아래와 맞춘다). `process` 는 추가하지 않는다.
  - `lib.rs` `run()` 에 `.plugin(tauri_plugin_updater::Builder::new().build())`. 기존 notification 플러그인 줄 옆.
  - `capabilities/default.json` permissions 에 `"updater:default"`.
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
    `createUpdaterArtifacts: true` 면 Windows 에서 설치 파일 옆에 `.sig` 가 나온다 (`"v1Compatible"` 은 쓰지 않는다).
- 완료 기준: `npm run tauri build` 가 `.exe` 와 `.exe.sig` 를 만든다 (환경변수 없이 빌드하면 서명 단계에서 실패하는 것이 정상. 그 오류 메시지를 보고에). `cargo test`, `npm run check` 녹색. lock 파일 변화가 플러그인 외의 의존성을 바꾸지 않았는지 `git diff --stat` 로 보고.
- 금지: 프론트 코드, 문서, 다른 플러그인 정리.

### T80 업데이트 확인 UI (과장 · 난이도 중) — T79 뒤

- 범위: `src/popup-settings.ts`(+test), `src/main.ts`, `src/store.ts`(설치 진행 상태), `src/sidebar-left.ts`(기존 토스트 경로만. 새 토스트 체계 금지), `design/handoff/phrases.ko.json` settings·toast 절, `design/HANDOFF.md` 4.6절 설정 행 한 줄.
- 할 일:
  - `check()`(`@tauri-apps/plugin-updater`)는 `SettingsDeps` 처럼 **주입**한다. 테스트는 가짜 check 로 "없음 / 있음 / 확인 실패 / 다운로드 중 / 설치 직전" 상태를 jsdom 에서 본다. 네트워크·실제 플러그인을 테스트에서 부르지 않는다.
  - 설정 팝업 "업데이트" 절: 현재 버전(`@tauri-apps/api/app` 의 `getVersion()`), "새 버전 확인" 버튼, 결과 문구, 찾으면 "x.y.z 설치" 버튼. 설치 중에는 버튼을 잠그고 `downloadAndInstall` 의 Progress 를 문구로 보여 준다. **설치 중 팝업을 닫았다 다시 열어도 설치가 중복 시작되지 않는다** (진행 상태는 팝업이 아니라 `store` 에 둔다).
  - Windows 는 설치 단계에서 앱이 종료된다. `downloadAndInstall` 이 돌아오지 않거나 종료로 끊기는 것은 정상이므로 그 경우만 오류로 보여 주지 않는다 (주석으로 이유). 그 밖의 실패(서명 불일치, 다운로드 실패, 매니페스트 없음)는 각각 문구로 보여 준다.
  - 시작 때 확인은 사무실이 뜬 뒤 한 번, 실패는 조용히 무시한다 (오프라인이 정상). 끌 수 있는 설정은 **만들지 않는다**.
  - 문구는 모두 `phrases.ko.json`. 기존 톤(~해요).
- 완료 기준: 위 상태별 테스트 + 중복 설치 방지 테스트, `npm test`·`npm run check` 녹색. `npm run tauri dev` 에서 "새 버전 확인"을 누르면: dev 빌드 버전이 최신 릴리스와 같으면 "최신입니다", 낮으면 업데이트 대상으로 뜬다 (dev 에서 설치는 누르지 않는다). 매니페스트가 없는 릴리스는 "최신"이 아니라 확인 실패로 뜬다.
- 금지: 자동 다운로드(사용자가 누르기 전에 받지 않는다), 설정 스키마(`settings.rs`) 변경, HANDOFF 밖의 새 화면.

### T81 릴리스 절차 (대리 · 난이도 하) — T79 뒤

- 범위: `tools/release_manifest.py`(새 파일), `tools/release_check.py`(새 파일), `docs/BUILD.md` 릴리스 행, `README.md`(설치 파일 링크 파일명). `build-gnu.sh` 는 손대지 않는다 (미확인: GNU 빌드에서 `.sig` 가 나오는지. 실측 뒤 틀리면 BUILD.md 폴백 절에 한 줄).
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
    `version` 은 `v` 없는 semver, `url` 은 `latest` 가 아니라 **태그 고정** 주소.
  - `tools/release_check.py <버전>` → `tauri.conf.json`·`package.json`·`Cargo.toml` 의 버전, 설치 파일명, `latest.json` 의 `version`·`url`, README 링크 파일명, 현재 HEAD 에 태그 `v<버전>` 이 있는지를 한 번에 검사하고 하나라도 다르면 실패.
  - docs/BUILD.md 릴리스 행을 이렇게 바꾼다 (한 사실 한 곳: 환경변수 이름은 여기만):
    1. README 설치 링크의 파일명을 `Owlfice_<버전>_x64-setup.exe` 로 바꾸고 버전을 올린 커밋을 만든다 (`releases/latest/download/<파일명>` 은 파일명이 고정이라 새 릴리스에 옛 파일명이 없으면 404 가 난다).
    2. 통합 브랜치를 main 에 합친 뒤 **검증한 그 커밋**에 `git tag -a v<버전> -m "<버전>"` 을 붙이고 `git push origin v<버전>`. (`gh release create` 는 태그가 없으면 기본 브랜치 끝에 태그를 만들어 바이너리와 소스가 어긋날 수 있다. 확인됨: https://cli.github.com/manual/gh_release_create )
    3. `TAURI_SIGNING_PRIVATE_KEY`(·`_PASSWORD`) 를 둔 셸에서 그 커밋을 체크아웃하고 `npm run tauri build`
    4. `python tools/release_manifest.py <버전> src-tauri/target/release/bundle/nsis/Owlfice_<버전>_x64-setup.exe > latest.json` (임시 폴더에)
    5. `python tools/release_check.py <버전>` 통과
    6. `gh release create v<버전> --verify-tag <setup.exe> <setup.exe.sig> latest.json`
    - **`latest.json` 을 빠뜨리거나 prerelease·draft 로 올리면** 설치된 앱이 새 버전을 못 본다 (`releases/latest/download/` 는 정식 최신 릴리스만 가리킨다).
- 완료 기준: 0.1.1 설치 파일과 가짜 `.sig` 로 두 스크립트를 돌려 위 형식이 나오고, 버전을 일부러 틀리면 `release_check.py` 가 실패한다. `uvx ruff check tools`, `uvx ruff format --check tools` 녹색. BUILD.md 행이 위 6단계를 담는다.
- 금지: GitHub Actions 추가 (릴리스는 로컬 빌드다).

### T82 출시 전 검증 → 정식 출시 → 실전 확인 (메인 + 사용자)

1. **출시 전 검증 (0.2.0 정식 릴리스 전).** 검증용 endpoint 와 버전을 덮어쓰는 JSON 을 임시 폴더에 둔다: `{"version":"0.2.0-test.1","plugins":{"updater":{"endpoints":["https://github.com/<검증 저장소>/releases/latest/download/latest.json"]}}}`. `npm run tauri build -- --config <경로>` 로 **0.2.0-test.1 과 0.2.0-test.2** 두 설치본을 만든다. test.2 와 그 `latest.json` 을 검증 저장소의 정식 릴리스로 올리고, test.1 을 설치해 본다.
   - 성공 경로: 시작 토스트 → 설정 "새 버전 확인" → 설치 → 앱이 종료·재시작 → 설정의 현재 버전이 test.2, 규모·회사 이름·사장님 이름·직접 지정 경로가 남아 있다.
   - 잘못된 서명: `latest.json` 의 `signature` 를 한 글자 바꿔 올리면 설치가 거부되고 문구가 뜬다.
   - 다운로드 실패: `url` 을 없는 파일로 바꾸면 실패 문구가 뜨고 앱은 계속 돈다.
   - 오프라인: 네트워크를 끄고 시작하면 조용히 넘어간다.
   - 중복 설치: 설치 중 설정 팝업을 닫고 다시 열어도 한 번만 진행된다.
   - CSP: 위 과정에서 webview 콘솔에 CSP 차단이 없으면 "CSP 는 안 바꾼다"의 (미확인) 을 뗀다.
2. 통과하면 T101 과 T81 절차로 **0.2.0 정식 릴리스**. 사용자가 0.1.1 위에 설치 파일을 직접 덮어 깐다.
3. **실전 확인.** 작은 수정으로 0.2.1 을 T81 절차로 릴리스 → 설치된 0.2.0 이 시작 토스트 또는 설정에서 0.2.1 을 찾아 설치 → 재시작 후 현재 버전 0.2.1, 사용자 데이터 유지.
4. 실패하면 AGENTS.md 6절대로 원인부터. 흔한 원인: `signature` 에 경로를 넣음, 공개 키 불일치, 릴리스가 prerelease, `pub_date` 형식, README 파일명 불일치(404).
5. 통과하면 이 문서와 `PLAN_0.2.0.md` 를 지우고 BUILD.md 의 **(미확인)** 표시를 뗀다.

## 순서

0 (키·검증 저장소) → T79 → (T80 ‖ T81, 같은 파일을 안 만지므로 동시 가능) → T82-1 출시 전 검증 → T101 → 0.2.0 릴리스 → T82-3.
