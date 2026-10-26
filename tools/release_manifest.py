"""설치 파일 옆의 서명으로 태그 고정 업데이터 매니페스트를 출력한다."""

import argparse
import json
import re
import sys
from datetime import UTC, datetime
from pathlib import Path
from urllib.parse import quote

DEFAULT_REPO = "dbwk10317/owlfice"
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def validate_version(version):
    match = SEMVER.fullmatch(version)
    if not match or any(
        part.isdigit() and len(part) > 1 and part.startswith("0")
        for part in (match[4] or "").split(".")
    ):
        raise ValueError(f"v 접두 없는 SemVer가 필요합니다: {version!r}")
    return version


def installer_name(version):
    return f"Owlfice_{validate_version(version)}_x64-setup.exe"


def release_url(version, repo=DEFAULT_REPO):
    owner, sep, name = repo.partition("/")
    if (
        not sep
        or not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?", owner)
        or "--" in owner
        or not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}", name)
        or name in (".", "..")
    ):
        raise ValueError(f"저장소는 owner/name 형식이어야 합니다: {repo!r}")
    filename = installer_name(version)
    return f"https://github.com/{repo}/releases/download/{quote('v' + version, safe='')}/{quote(filename, safe='')}"


def read_signature(installer, version):
    expected = installer_name(version)
    if installer.name != expected:
        raise ValueError(f"설치 파일명 불일치: {expected} 필요")
    if not installer.is_file():
        raise ValueError(f"설치 파일이 없습니다: {installer}")
    signature_path = installer.with_name(installer.name + ".sig")
    signature = signature_path.read_bytes().decode("utf-8")
    if not signature.strip():
        raise ValueError(f"서명이 비어 있습니다: {signature_path}")
    return signature


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("installer", type=Path)
    parser.add_argument("--repo", default=DEFAULT_REPO)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        url = release_url(args.version, args.repo)
        signature = read_signature(args.installer, args.version)
        manifest = {
            "version": args.version,
            "notes": "",
            "pub_date": datetime.now(UTC)
            .isoformat(timespec="seconds")
            .replace("+00:00", "Z"),
            "platforms": {"windows-x86_64": {"signature": signature, "url": url}},
        }
        text = json.dumps(manifest, indent=2) + "\n"
        if args.output is None:
            print(text, end="")
        else:
            args.output.write_text(text, encoding="utf-8", newline="\n")
    except (OSError, UnicodeError, ValueError) as error:
        print(f"release_manifest: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
