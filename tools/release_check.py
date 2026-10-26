"""로컬 릴리스 파일·버전·README·HEAD 태그가 같은 버전을 가리키는지 검사한다."""

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

sys.dont_write_bytecode = True

from release_manifest import (
    installer_name,
    read_signature,
    release_url,
    validate_version,
)

ROOT = Path(__file__).resolve().parent.parent


def check(version, manifest_path, installer):
    validate_version(version)
    for relative in (
        "package.json",
        "src-tauri/tauri.conf.json",
        "src-tauri/Cargo.toml",
    ):
        path = ROOT / relative
        text = path.read_text(encoding="utf-8")
        data = (
            tomllib.loads(text)["package"]
            if path.suffix == ".toml"
            else json.loads(text)
        )
        if data["version"] != version:
            raise ValueError(f"{relative} 버전 불일치: {version} 필요")
    signature = read_signature(installer, version)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest["version"] != version:
        raise ValueError(f"{manifest_path} 버전 불일치: {version} 필요")
    platform = manifest["platforms"]["windows-x86_64"]
    if platform["url"] != release_url(version):
        raise ValueError(
            "latest.json URL은 공식 저장소의 해당 버전 태그 고정 주소여야 합니다"
        )
    if platform["signature"] != signature:
        raise ValueError("latest.json 서명이 설치 파일 옆 .sig 내용과 다릅니다")
    links = re.findall(
        r"https://github\.com/dbwk10317/owlfice/releases/latest/download/([^\s\"'<>)]*)",
        (ROOT / "README.md").read_text(encoding="utf-8"),
    )
    if not links or any(link != installer_name(version) for link in links):
        raise ValueError("README 설치 링크 파일명이 릴리스 버전과 다릅니다")
    for ref in ("HEAD", f"refs/tags/v{version}^{{commit}}"):
        result = subprocess.run(
            ["git", "rev-parse", "--verify", ref],
            check=False,
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if result.returncode:
            raise ValueError(f"Git 커밋을 찾을 수 없습니다: {ref}")
        if ref == "HEAD":
            head = result.stdout.strip()
        elif result.stdout.strip() != head:
            raise ValueError(f"태그 v{version}가 현재 HEAD를 가리키지 않습니다")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--installer", type=Path)
    args = parser.parse_args()
    try:
        installer = args.installer or (
            ROOT / "src-tauri/target/release/bundle/nsis" / installer_name(args.version)
        )
        check(args.version, args.manifest, installer)
    except (KeyError, TypeError) as error:
        print(
            f"release_check: 릴리스 파일의 필수 필드 형식이 잘못되었습니다: {error}",
            file=sys.stderr,
        )
        return 1
    except (OSError, UnicodeError, ValueError) as error:
        print(f"release_check: {error}", file=sys.stderr)
        return 1
    print(f"릴리스 검사 통과: {args.version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
