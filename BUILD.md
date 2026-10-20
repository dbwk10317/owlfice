# 빌드

빌드·테스트 명령과 저장소 규칙. 왜 이 스택인지는 PLAN 4.1.

## 빌드 경로

| 경로 | 타깃 | 언제 쓰나 |
|---|---|---|
| **기본: Visual Studio (MSVC)** | `x86_64-pc-windows-msvc` | 보통의 Windows PC |
| 폴백 1: MSYS2 (GNU) | `x86_64-pc-windows-gnu` | Visual Studio를 쓸 수 없는 PC |
| 폴백 2: WSL 크로스 빌드 (GNU) | `x86_64-pc-windows-gnu` | 보안 소프트웨어가 Windows에서 링크한 exe를 격리하는 PC (PLAN 4.1) |

저장소 파일은 세 경로에서 같다. PC를 옮길 때는 저장소를 고치지 않고 이 문서의 절차만 바꾼다. `node_modules` 에는 설치한 OS의 네이티브 바인딩만 깔리므로, WSL과 Windows를 오가면 그쪽에서 `npm ci` 를 다시 한다.

## 기본: Visual Studio (MSVC)

개발 PC 기준 (2026-10-20 확인):

| 도구 | 버전 | 설치 |
|---|---|---|
| Visual Studio | Community 2026 18.11, MSVC 14.51, Windows SDK 10.0.26100 | "C++를 사용한 데스크톱 개발" 워크로드. Build Tools만 깔아도 된다 |
| Rust | 1.99.1 | `winget install Rustlang.Rustup`. 기본 호스트 `x86_64-pc-windows-msvc`, rustfmt·clippy 포함 |
| Node / npm | 26.8.1 / 11.19.0 | `winget install OpenJS.NodeJS` |
| Python | 3.14.7 | `winget install Python.Python.3.14 --scope user`. 앱 아이콘과 `tools/` 스크립트용 |
| WebView2 | 155 | Windows 11에 들어 있다 |
| NSIS, tauri-cli | 3.13 / 2.12.3 | 따로 깔지 않는다. NSIS는 Tauri 번들러가 첫 빌드 때 받고, tauri-cli는 `package.json` devDependency |

winget으로 깐 직후의 셸에는 새 PATH가 없다. 터미널을 새로 연다. PowerShell 실행 정책이 `npm.ps1` 을 막으면 `npm.cmd` 로 부른다.

명령은 저장소 루트에서 돌린다.

| 일 | 명령 |
|---|---|
| 의존성 설치 | `npm ci` |
| Rust 테스트 | `cargo test --manifest-path src-tauri/Cargo.toml` |
| Rust 포맷 검사 | `cargo fmt --manifest-path src-tauri/Cargo.toml --check` |
| Rust 린트 | `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` |
| 프론트 타입 검사·빌드 | `npm run build` |
| 개발 실행 | `npm run tauri dev` |
| 릴리스 | `npm run tauri build` → `src-tauri/target/release/bundle/nsis/*-setup.exe` |
| 의존 DLL 확인 | `dumpbin /dependents src-tauri\target\release\owlfice.exe` (Developer PowerShell for VS) |

의존 DLL에는 Windows 시스템 DLL만 나와야 한다. `vcruntime140.dll` 이 보이면 VC 런타임이 정적으로 링크되지 않은 것이다 (PLAN 4.1, 6부). `api-ms-win-crt-*` 는 Windows 10부터 OS에 든 UCRT라 괜찮다.

### 확인 기록 (2026-10-20)

tauri 2.12.4, tauri-build 2.7.3, 나머지는 위 표의 버전. Defender만 켜진 PC.

- 명령표가 전부 통과했다. 테스트는 아직 0개다.
- `owlfice.exe` 4.0 MB, `Owlfice_0.1.0_x64-setup.exe` 1.3 MB. Defender가 격리하지 않았다.
- 의존 DLL은 Windows 시스템 DLL과 `api-ms-win-*` 뿐이다. `vcruntime140.dll` 은 없다.
- 릴리스 exe와 `tauri dev` 모두 창 「야근불 · Owlfice」가 뜬다.
- **(미확인)** 개발 도구가 없는 Windows 11에 설치 파일을 깔아 실행하는 것 (T01 완료 기준).

## 폴백 1: MSYS2 (GNU)

| 도구 | 설치 |
|---|---|
| Rust | rustup. `rustup target add x86_64-pc-windows-gnu`, `rustup component add rustfmt clippy` |
| MSYS2 UCRT64 | `pacman -S mingw-w64-ucrt-x86_64-gcc`. `ucrt64\bin` 을 PATH에 둔다 (gcc, objdump, windres) |
| Node, NSIS, tauri-cli | 기본 경로와 같다 |

- 명령은 MSYS2 셸이나 Git Bash에서 돌린다. 기본 경로 명령표의 `cargo test`·`cargo clippy`·`tauri dev` 에 `--target x86_64-pc-windows-gnu` 를 붙인다 (`tauri dev` 는 `npm run tauri dev -- --target ...`).
- 릴리스는 `bash build-gnu.sh` 다. 설치 파일을 만들고 의존 DLL을 검사한다.
- **(미확인)** 이 경로로 앱 전체를 빌드한 적은 없다. UCRT64와 Rust의 C 런타임이 섞이는 문제는 PLAN 6부.

## 폴백 2: WSL 크로스 빌드 (GNU)

개발 PC 기준 (2026-10-20 확인):

| 도구 | 버전 | 설치 |
|---|---|---|
| 배포판 | Rocky Linux 9.8 | |
| Rust | 1.99.1 | rustup. `rustup target add x86_64-pc-windows-gnu`, `rustup component add rustfmt clippy` |
| mingw-w64 | gcc 14.2.1, binutils 2.43.1 | `dnf --enablerepo=crb install mingw64-gcc` |
| Node / npm | 26.8.1 / 12.0.2 | nvm |
| NSIS | 3.11 | 아래 |
| tauri-cli | 2.12.3 | `package.json` devDependency |

- 명령은 폴백 1과 같다 (`--target x86_64-pc-windows-gnu`, 릴리스는 `bash build-gnu.sh`). 로그인 셸(`bash -l`)에서 돌린다. 비로그인 셸이면 `~/.cargo/bin`·`~/.local/bin` 이 PATH에서 빠질 수 있다.
- 링커는 rustc 기본값 `x86_64-w64-mingw32-gcc` 를 그대로 쓴다. 저장소의 `.cargo/config.toml` 말고 따로 둘 cargo 설정은 없다.
- `npm ci` 도 WSL에서 한다. 저장소는 `/mnt/c/...` 그대로 쓰고, 산출물만 WSL 디스크에 둔다. `/mnt/c` 위의 target은 느리다.

  ```bash
  export CARGO_TARGET_DIR=$HOME/.cache/owlfice/target
  ```

- NSIS는 Rocky 저장소에 없다. 3.11 소스에서 `makensis` 만 scons로 빌드해 `~/.local/nsis/bin/` 에 두고, `Stubs` `Include` `Plugins` `Contrib` `nsisconf.nsh` 는 같은 버전의 Windows 배포본에서 `~/.local/nsis/` 로 복사한다. PATH에는 실행 파일 대신 래퍼를 둔다. 심볼릭 링크면 makensis가 데이터 폴더를 못 찾는다.

  ```sh
  # ~/.local/bin/makensis
  #!/bin/sh
  exec "$HOME/.local/nsis/bin/makensis" "$@"
  ```

- `cargo test`·`tauri dev` 가 만든 Windows exe는 WSL interop으로 Windows 프로세스로 뜬다. 레지스트리·환경변수 테스트도 WSL에서 돌린다.
- Windows 쪽에서 고친 파일은 WSL의 파일 감시에 잡히지 않는다. 프론트는 `vite.config.ts` 가 WSL에서 폴링으로 감시해 반영된다. Rust 코드를 고쳤으면 `tauri dev` 를 다시 띄운다.
- 크로스 빌드에서 나오는 경고 셋은 무시한다. Tauri의 "실험 기능" 경고, 설치 파일 서명 건너뜀(서명은 아직 안 한다), makensis `warning 5202: -OUTPUTCHARSET`(리눅스 빌드라 나온다).

### 확인 기록 (T01, 2026-10-20)

tauri 2.12.4, tauri-cli 2.12.3, 나머지는 위 표의 버전.

- `bash build-gnu.sh` 통과. `owlfice.exe` 4.4 MB, `Owlfice_0.1.0_x64-setup.exe` 1.4 MB.
- 의존 DLL은 Windows 시스템 DLL과 `WebView2Loader.dll` 뿐이다. `WebView2Loader.dll` 은 설치 파일이 exe 옆에 넣는다. `libgcc_s`·`libwinpthread`·`libstdc++` 는 없다.
- 릴리스 exe와 `tauri dev` 모두 창 「야근불 · Owlfice」가 뜬다. dev에서는 WSL의 Vite에 HMR 웹소켓이 붙고, Windows에서 고친 `index.html` 이 다시 읽힌다.
- fmt·clippy·테스트 통과. 테스트는 아직 0개다.

## 앱 아이콘

`src-tauri/icons/icon.ico` 는 `design/assets/logo.png` 를 최근접 정수배로 키워 만든다. 로고가 바뀌면 다시 만든다.

```bash
python -c "
from PIL import Image
logo = Image.open('design/assets/logo.png').convert('RGBA')
base = Image.new('RGBA', (16, 16)); base.paste(logo, (0, 1))
frames = [base.resize((s, s), Image.Resampling.NEAREST) for s in (16, 32, 48, 64, 128, 256)]
frames[-1].save('src-tauri/icons/icon.ico', sizes=[f.size for f in frames], append_images=frames[:-1])
"
```

`tauri icon` 은 부드럽게 줄여서 도트가 번지므로 쓰지 않는다.

## 저장소 규칙

- 티켓 하나 = 브랜치 하나 = PR 하나. 브랜치 이름은 `t<두 자리 번호>-<영문 요약>` (예: `t04-tail-reader`). 아트와 코드는 PR을 나눈다 (PLAN 5.5).
- main에는 사용하는 경로의 명령표 테스트·검사가 모두 통과한 것만 합친다.
