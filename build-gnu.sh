#!/usr/bin/env bash
# GNU 폴백 릴리스 빌드: NSIS 설치 파일을 만들고, exe가 Windows 시스템 밖 DLL에 기대지 않는지 검사한다.
# MSYS2와 WSL 크로스 빌드에서 같이 돈다. 기본 경로(MSVC)는 docs/BUILD.md.
set -euo pipefail
cd "$(dirname "$0")"

target=x86_64-pc-windows-gnu
out="${CARGO_TARGET_DIR:-src-tauri/target}/$target/release"
rm -rf "$out/bundle/nsis"  # 예전 설치 파일이 남아 있으면 번들이 빠져도 아래 ls가 통과한다

npm ci
npm run tauri build -- --target "$target"

objdump=$(command -v x86_64-w64-mingw32-objdump || command -v objdump) || { echo "objdump가 없다" >&2; exit 1; }
sys32=/c/Windows/System32
[ -d /mnt/c/Windows/System32 ] && sys32=/mnt/c/Windows/System32

dlls=$("$objdump" -p "$out/owlfice.exe" | sed -n 's/^[[:space:]]*DLL Name: //p')
if ! grep -qix kernel32.dll <<<"$dlls"; then
  echo "의존 DLL 목록을 읽지 못했다 ($objdump)" >&2
  exit 1
fi

echo "의존 DLL:"
bad=""
for dll in $dlls; do
  echo "  $dll"
  case "$dll" in api-ms-win-*) continue ;; esac  # API 세트는 파일이 아니라 OS가 푼다
  # GNU 타깃은 WebView2Loader.dll을 동적으로 쓴다. 설치 파일이 exe 옆에 넣을 때만 통과
  if [ "$dll" = WebView2Loader.dll ] && grep -q 'WebView2Loader.dll' "$out/nsis/x64/installer.nsi"; then continue; fi
  [ -e "$sys32/$dll" ] || bad="$bad $dll"
done
if [ -n "$bad" ]; then
  echo "시스템 밖 DLL에 의존한다:$bad" >&2
  exit 1
fi
ls -l "$out"/bundle/nsis/*-setup.exe
