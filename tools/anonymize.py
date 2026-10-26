#!/usr/bin/env python3
"""실제 세션 파일을 fixture로 바꾸고, fixtures/ 에 실제 값이 남았는지 검사한다 (docs/CONTRACTS.md C12).

호출: uv run python tools/anonymize.py <claude|codex|check> ...
  claude SRC.jsonl OUT [--lines 1-3,100-140] [--agent ID[:1-30,400-420]]...
  codex  SRC.jsonl OUT [--lines ...]
  check  [DIR]            기본 fixtures/. 실제 값이 하나라도 있으면 종료 코드 1

허용 목록 방식이다. 레코드 type, 키 이름, 도구 이름, 열거형 값, 명령 플래그, 파일 확장자가
알려진 닫힌 집합에 있을 때만 원래 값으로 남고 그 밖은 전부 치환한다.
집합 밖이라 치환한 것은 stderr 에 부모 키와 개수만 알린다. 원본은 읽기만 하고,
익명화 결과만 OUT에 쓴다. 매핑 표는 메모리에만 둔다.
check 는 이 PC 의 값(사용자·컴퓨터 이름, git 설정, 세션 cwd 조각)만 대조하므로, 사람 글에서 온
고유명사(동료·회사 이름 등)는 못 잡는다. check 통과가 사람의 눈 확인을 대신하지 않는다.
"""

import argparse
import getpass
import json
import os
import re
import socket
import subprocess
import sys
from datetime import datetime
from pathlib import Path

from anonymize_core import BASE_TS, ISO_RE, UUID_RE, Anon, first_ts

ROOT = Path(__file__).resolve().parent.parent


# ---- 입출력 ----
def parse_spec(spec: str | None, n: int) -> list[int]:
    if spec is None:
        return list(range(n))
    idx: list[int] = []
    for part in spec.split(","):
        match = re.fullmatch(r"([0-9]+)(?:-([0-9]+))?", part.strip())
        if not match:
            raise ValueError(
                "행 범위는 양의 정수 또는 시작-끝을 쉼표로 구분해 입력하세요"
            )
        start, end = int(match[1]), int(match[2] or match[1])
        if start < 1 or end < 1:
            raise ValueError("행 번호는 1 이상이어야 합니다")
        if start > end:
            raise ValueError("행 범위의 시작은 끝보다 클 수 없습니다")
        if end > n:
            raise ValueError(f"행 범위가 파일의 전체 {n}행을 넘습니다")
        idx.extend(range(start - 1, end))
    return idx


def load(path: Path, spec: str | None) -> list[dict]:
    lines = path.read_text(encoding="utf-8").splitlines()
    records = []
    for i in parse_spec(spec, len(lines)):
        try:
            record = json.loads(lines[i])
        except json.JSONDecodeError as error:
            raise ValueError(
                f"{path}:{i + 1}: 유효한 JSON 레코드가 아닙니다"
            ) from error
        if not isinstance(record, dict):
            raise ValueError(f"{path}:{i + 1}: 레코드는 객체여야 합니다")  # noqa: TRY004
        if "type" in record and not isinstance(record["type"], str):
            raise ValueError(f"{path}:{i + 1}: type은 문자열이어야 합니다")
        if (
            record.get("type") == "attachment"
            and record.get("attachment") is not None
            and not isinstance(record["attachment"], dict)
        ):
            raise ValueError(f"{path}:{i + 1}: attachment는 객체여야 합니다")
        validate_timestamp(record, path, i + 1)
        records.append(record)
    return records


def validate_timestamp(record: dict, path: Path, line: int) -> None:
    if record.get("timestamp") is None or record.get("timestamp") == "":
        return
    value = record["timestamp"]
    if not isinstance(value, str) or not ISO_RE.fullmatch(value):
        raise ValueError(f"{path}:{line}: timestamp 형식이 올바르지 않습니다")
    try:
        datetime.fromisoformat(value)
    except ValueError as error:
        raise ValueError(
            f"{path}:{line}: timestamp 형식이 올바르지 않습니다"
        ) from error


def read_meta(path: Path) -> dict:
    try:
        meta = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ValueError(
            f"{path}:{error.lineno}: 유효한 JSON 메타가 아닙니다"
        ) from error
    if not isinstance(meta, dict):
        raise ValueError(f"{path}:1: 메타는 객체여야 합니다")  # noqa: TRY004
    validate_timestamp(meta, path, 1)
    return meta


def jsonl_text(records: list[dict]) -> str:
    return "".join(
        json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n" for r in records
    )


def convert(
    kind: str, src: Path, out: Path, spec: str | None, agents: list[str]
) -> None:
    match = UUID_RE.search(src.stem)
    if match is None:
        raise ValueError("원본 파일명에 세션 UUID가 필요합니다")
    sid = match.group(0)
    main = load(src, spec)
    subs = []
    for a in agents:
        aid, _, aspec = a.partition(":")
        sub_dir = src.with_suffix("") / "subagents"
        subs.append(
            (
                aid,
                load(sub_dir / f"agent-{aid}.jsonl", aspec or None),
                sub_dir / f"agent-{aid}.meta.json",
            )
        )
    stamps = [t for t in [first_ts(main)] + [first_ts(r) for _, r, _ in subs] if t]
    if not stamps:
        raise ValueError(f"{src}:1: 타임스탬프가 있는 레코드가 없습니다")
    an = Anon(int((BASE_TS - min(stamps)).total_seconds() * 1000), out.name)
    outputs: list[tuple[Path, str]] = []
    if kind == "claude":
        base = out / f"{an.map_id(sid)}"
        outputs.append(
            (
                base.with_suffix(".jsonl"),
                jsonl_text([x for r in main if (x := an.record(r))]),
            )
        )
    else:
        first_main = next((r["timestamp"] for r in main if r.get("timestamp")), None)
        if first_main is None:
            raise ValueError(f"{src}:1: 메인 레코드에 타임스탬프가 없습니다")
        shifted = an.shift_iso(first_main) or ""
        name = f"rollout-{shifted[:19].replace(':', '-')}-{an.map_id(sid)}.jsonl"
        outputs.append(
            (out / name, jsonl_text([x for r in main if (x := an.record(r))]))
        )
    for aid, recs, meta in subs:
        stem = base / "subagents" / f"agent-{an.map_id(aid)}"
        outputs.append(
            (
                stem.with_suffix(".jsonl"),
                jsonl_text([x for r in recs if (x := an.record(r))]),
            )
        )
        if meta.exists():
            m = an.dict(read_meta(meta))
            outputs.append(
                (
                    stem.with_suffix(".meta.json"),
                    json.dumps(m, ensure_ascii=False, separators=(",", ":")),
                )
            )
    # ponytail: 출력 I/O 오류 때 앞 파일은 남을 수 있다. 다중 파일 원자성이 필요해지면 올린다.
    for path, text in outputs:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8", newline="\n")
    for (what, parent), n in sorted(an.dropped.items()):
        print(f"허용 집합 밖 치환: {what} 부모={parent} {n}건", file=sys.stderr)


# ---- 검사 ----
TOKEN_RES = {
    "토큰(sk-)": r"sk-[A-Za-z0-9_-]{20,}",
    "토큰(gh)": r"gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}",
    "토큰(slack)": r"xox[abprs]-[A-Za-z0-9-]{10,}",
    "토큰(aws)": r"AKIA[0-9A-Z]{16}",
    "토큰(google)": r"AIza[0-9A-Za-z_-]{35}",
    "토큰(jwt)": r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.",
    "개인키": r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
    "Bearer": r"Bearer\s+[A-Za-z0-9._~+/-]{20,}",
    "이메일": r"[A-Za-z0-9._%+-]+@(?!example\.(?:com|org)\b)[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
    "홈 경로": r"(?i)users[\\/]+[^\\/\s\"]+|/home/[^/\s\"]+",
}
SHORT_MIN = 3  # 이보다 짧은 이름은 대조하지 않는다. 오탐이 너무 많다
STOP = {
    "claude",
    "codex",
    "users",
    "workspace",
    "windows",
    "appdata",
    "roaming",
    "local",
    "scratch",
    "desktop",
    "documents",
    "program",
    "files",
}


def git(*args: str, cwd: str | None = None) -> str:
    try:
        r = subprocess.run(
            ["git", *args],
            capture_output=True,
            text=True,
            timeout=10,
            cwd=cwd,
            check=False,
        )
        return r.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def session_cwds(files: list[Path]) -> set[str]:
    cwds = set()
    for f in files:
        with f.open(encoding="utf-8") as fh:
            for _, line in zip(range(60), fh, strict=False):
                m = re.search(r'"cwd"\s*:\s*"((?:[^"\\]|\\.)*)"', line)
                if m:
                    cwds.add(json.loads('"' + m.group(1) + '"'))
    return cwds


def real_values() -> dict[str, list[str]]:
    """이 PC의 실제 값을 그때그때 모은다. 파일로 저장하지 않는다."""
    home = Path.home()
    vals: dict[str, set[str]] = {
        "사용자 이름": {getpass.getuser(), home.name, os.environ.get("USERNAME", "")}
    }
    vals["홈 경로"] = {
        str(home),
        str(home).replace("\\", "/"),
        str(home).replace("\\", "\\\\"),
    }
    vals["컴퓨터 이름"] = {os.environ.get("COMPUTERNAME", ""), socket.gethostname()}
    email = git("config", "--global", "user.email")
    local, _, domain = email.partition("@")
    vals["git 이메일"] = {email, local, domain}
    name = git("config", "--global", "user.name")
    vals["git 이름"] = {name, *name.split()}
    proj = home / ".claude" / "projects"
    names = [p.name for p in proj.iterdir()] if proj.exists() else []
    vals["프로젝트 폴더"] = set(names)
    frags, remotes = set(), set()
    for n in names:
        frags.update(
            t
            for t in re.split(r"-+", n)
            if re.fullmatch(r"[A-Za-z][A-Za-z0-9]{4,}", t)
            and not re.fullmatch(r"[0-9a-f]+", t.lower())
        )
    codex = home / ".codex" / "sessions"
    cwds = session_cwds(
        [f for n in names for f in (proj / n).glob("*.jsonl")]
        + (list(codex.rglob("rollout-*.jsonl")) if codex.exists() else [])
    )
    for c in cwds:
        frags.update(
            t
            for t in re.split(r"[\\/:.]+", c)
            if re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{4,}", t)
        )
        if Path(c).exists():
            for line in git("remote", "-v", cwd=c).splitlines():
                parts = line.split()
                if len(parts) > 1:
                    remotes.add(parts[1])
                    remotes.add(
                        re.sub(r"^.*[:/]([^:/]+/[^/]+?)(?:\.git)?$", r"\1", parts[1])
                    )
    vals["경로 조각"] = {
        f
        for f in frags
        if f.lower() not in STOP and not re.fullmatch(r"[0-9a-f]+", f.lower())
    }
    vals["원격 저장소"] = remotes
    return {k: sorted(x for x in v if len(x) >= SHORT_MIN) for k, v in vals.items()}


def check(root: Path) -> int:
    if not root.is_dir():
        raise ValueError("검사 대상 폴더가 없습니다")
    files = [p for p in sorted(root.rglob("*")) if p.is_file()]
    if not files:
        raise ValueError("검사 대상 파일이 없습니다")
    values = [(label, v.lower()) for label, vs in real_values().items() for v in vs]
    values += [(label, v.replace("\\", "\\\\")) for label, v in values if "\\" in v]
    # 4자 미만은 다른 단어 속에 우연히 들어가므로 단어 경계로만 대조한다
    needles = [(label, re.compile(re.escape(v))) for label, v in values if len(v) >= 4]
    needles += [
        (label, re.compile(rf"(?<![a-z0-9]){re.escape(v)}(?![a-z0-9])"))
        for label, v in values
        if len(v) < 4
    ]
    token_res = {k: re.compile(v) for k, v in TOKEN_RES.items()}
    hits: dict[tuple[str, str], int] = {}
    for p in files:
        text = p.read_text(encoding="utf-8", errors="replace")
        low = text.lower()
        for label, rx in needles:
            n = len(rx.findall(low))
            if n:
                hits[(str(p), label)] = hits.get((str(p), label), 0) + n
        for label, rx in token_res.items():
            n = len(rx.findall(text))
            if n:
                hits[(str(p), label)] = n
    print(
        f"검사 파일 {len(files)}개, 대조 값 {len(needles)}개 (값 자체는 출력하지 않는다)"
    )
    for (path, label), n in sorted(hits.items()):
        print(f"발견: {path} [{label}] {n}건")
    print("실패" if hits else "통과: 실제 값 없음")
    return 1 if hits else 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = ap.add_subparsers(dest="cmd", required=True)
    for kind in ("claude", "codex"):
        p = sub.add_parser(kind)
        p.add_argument("src", type=Path)
        p.add_argument("out", type=Path)
        p.add_argument("--lines")
        p.add_argument("--agent", action="append", default=[])
    sub.add_parser("check").add_argument(
        "dir", nargs="?", type=Path, default=ROOT / "fixtures"
    )
    a = ap.parse_args()
    try:
        if a.cmd == "check":
            return check(a.dir)
        if a.cmd == "codex" and a.agent:
            raise ValueError("codex에는 --agent가 없습니다")
        convert(a.cmd, a.src, a.out, a.lines, a.agent)
    except UnicodeError:
        print("anonymize: UTF-8 파일을 읽거나 쓰지 못했습니다", file=sys.stderr)
        return 1
    except (OSError, ValueError) as error:
        print(f"anonymize: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
