#!/usr/bin/env python3
"""실제 세션 파일을 fixture로 바꾸고, fixtures/ 에 실제 값이 남았는지 검사한다 (PLAN 4.12).

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
import random
import re
import socket
import subprocess
import sys
import unicodedata
from collections import Counter
from datetime import UTC, datetime, timedelta
from pathlib import Path

BASE_TS = datetime(
    2026, 3, 2, 9, 0, 0, tzinfo=UTC
)  # 시각 평행이동의 도착점 (첫 레코드)
TEXT_CAP = 400  # 긴 본문은 이 길이로 줄인다. 끝 두 줄은 남겨 테스트 요약과 닫는 태그를 보존한다

UUID = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
UUID_RE = re.compile(UUID)
ID_PREFIXES = "toolu|msg|req|call|ctc|ctco|rs|resp|retained|exec|item|fc"
PREFIXED_ID = rf"(?<![A-Za-z0-9])(?:{ID_PREFIXES})[_-][A-Za-z0-9]{{10,}}"
EMBEDDED_ID_RE = re.compile(
    rf"(?<![0-9a-f-]){UUID}(?![0-9a-f-])|{PREFIXED_ID}|(?<![A-Za-z0-9])a[0-9a-f]{{16}}(?![A-Za-z0-9])"
)
ISO_RE = re.compile(
    r"^(\d{4})-(\d{2})-(\d{2})([T ])(\d{2}):(\d{2}):(\d{2})(?:([.,])(\d+))?(Z|[+-]\d{2}:?\d{2})?$"
)
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
LOOKS_ID_COMP = re.compile(r"^(?=.*\d)(?=.*[a-z])[a-z0-9]{8,20}$")

# 값이 구조라서 원래대로 남기는 키와, 키마다 허용하는 값. 집합 밖 값은 글처럼 치환한다.
# type 끝의 넷은 fixture에는 없지만 TOOL_BLOCKS 가 이름 붙인 블록이다
ENUM_VALUES = {
    "advisorModel": "claude-opus-5-5",
    "agentType": "general-purpose",
    "approval_policy": "never",
    "approvals_reviewer": "user",
    "cli_version": "0.162.0-alpha.17.2",
    "collaboration_mode_kind": "default",
    "cyber_access_program": "standard",
    "decision": "accept",
    "effort": "high|xhigh",
    "entrypoint": "claude-desktop",
    "history_mode": "paginated",
    "inference_geo": "global",
    "isolation": "worktree",
    "kind": "human|task-notification",
    "level": "info|suggestion",
    "mode": "default|normal",
    "model": "claude-opus-5-5|gpt-6-astra|opus",
    "model_provider": "openai",
    "operation": "dequeue|enqueue|remove",
    "originator": "Codex Desktop",
    "perTurnEffort": "high",
    "permissionMode": "auto",
    "personality": "friendly",
    "phase": "commentary|final_answer",
    "producer": "session-task",
    "promptSource": "sdk|system",
    "reasonType": "classifier|other|rule",
    "reasoning_effort": "xhigh",
    "requestShape": "background|foreground",
    "requestedModel": "claude-opus-5-5",
    "resolvedModel": "claude-opus-5-5",
    "role": "assistant|developer|user",
    "service_tier": "standard",
    "source": "config|unified_exec_startup|vscode",
    "speed": "standard",
    "status": "async_launched|completed",
    "stopReason": "",
    "stop_reason": "end_turn|tool_use",
    "subagent_type": "general-purpose",
    "subtype": "compact_boundary|stop_hook_summary",
    "thread_source": "user",
    "toolDenialKind": "user-rejected",
    "trigger": "auto",
    "turnOrigin": "human|peer|task_notification",
    "turn_trigger": "composer",
    "type": "AgentMessage|CommandExecution|Reasoning|Text|UserMessage|agent-name|agent_listing_delta|assistant|atis-latch|attachment|auto_mode|credential_org|custom-title|custom_tool_call|custom_tool_call_output|date|deferred_tools_delta|deferred_tools_record|direct|environment|event_msg|file|file_unchanged|input_text|instructions|item_completed|last-prompt|mcp_instructions_delta|message|mode|model|output_text|prompt_snapshot|queue-operation|queued_command|reasoning|remote_session_change|response_item|session_context|session_meta|skill_listing|system|task_complete|task_started|text|thinking|token_count|token_usage_record|tool_result|tool_use|total_tokens_reminder|turn_context|unknown|user|function_call|function_call_output|mcp_tool_use|server_tool_use",
    "userType": "external",
    "version": "2.1.295",
}  # fmt: skip
ENUM_VALUES = {k: frozenset(v.split("|")) for k, v in ENUM_VALUES.items()}

# 키 이름도 허용 집합에 있는 것만 남긴다. 사람이 고른 이름(환경변수, 파일명 등)은 kN 으로 바꾼다
ALLOWED_KEYS = {
    "active_permission_profile", "advisorModel", "agentId", "agentName", "agentType", "aggregated_output",
    "allUuids", "anchorUuid", "apiBlockIndex", "approval_policy", "approvals_reviewer", "atis", "attachment",
    "backgroundTaskId", "cache_creation", "cache_creation_input_tokens", "cache_read_input_tokens",
    "cache_write_input_tokens", "cached_input_tokens", "call_id", "caller", "canContinueAgent",
    "canReadOutputFile", "cli_version", "client_id", "cmd", "collaboration_mode", "collaboration_mode_kind",
    "command", "commandUuid", "comp_hash", "compactMetadata", "completed_at", "completed_at_ms", "container",
    "content", "context_management", "context_window", "creator_account_id", "creator_user_id",
    "cumulativeDroppedTokens", "current_date", "customTitle", "cwd", "cyber_access_program", "decision",
    "deliveryId", "description", "disabled_plugin_ids", "duration", "durationMs", "duration_ms", "effort",
    "encrypted_content", "entrypoint", "ephemeral_1h_input_tokens", "ephemeral_5m_input_tokens", "exit_code",
    "fallback_credit", "file", "filePath", "file_path", "gitBranch", "hasOutput", "headUuid", "history_mode",
    "hookAdditionalContext", "hookCount", "hookErrors", "hookInfos", "id", "inference_geo", "info",
    "initiating_agent_path", "input", "input_tokens", "interrupted", "isAsync", "isCompactSummary", "isImage",
    "isMeta", "isSidechain", "isVisibleInTranscriptOnly", "is_error", "isolation", "item", "iterations",
    "kind", "lastPrompt", "last_token_usage", "leafUuid", "level", "logicalParentUuid", "message", "mode",
    "model", "model_context_window", "model_provider", "multi_agent_version", "name", "nanos", "new_string",
    "noOutputExpected", "numLines", "old_string", "operation", "ordinal", "origin", "originator", "output",
    "outputFile", "output_tokens", "output_tokens_details", "parentUuid", "parent_turn_id", "parsed_cmd",
    "payload", "perTurnEffort", "permissionDecision", "permissionMode", "persistedOutputPath",
    "persistedOutputSize", "personality", "phase", "postTokens", "preCompactDiscoveredTools", "preTokens",
    "preservedMessages", "preservedSegment", "preventedContinuation", "process_id", "producer", "prompt",
    "promptId", "promptIndex", "promptSource", "queueSkipAttachments", "raw_content", "realtime_active",
    "reason", "reasonType", "reasoning_effort", "reasoning_output_tokens", "replace_all", "requestId",
    "requestNonInteractive", "requestShape", "requestedModel", "resolvedModel", "response_id", "role",
    "root_turn_id", "runId", "run_in_background", "runtime_workspace_roots", "secs", "server_tool_use",
    "service_tier", "sessionId", "session_id", "settings", "signature", "source", "sourceToolAssistantUUID",
    "spawnDepth", "spawnedWithWorktree", "speed", "startLine", "started_at", "started_at_ms", "status",
    "stderr", "stdout", "stopReason", "stop_details", "stop_reason", "stop_sequence", "stoppedByUser",
    "subagent_type", "subtype", "summary", "summary_text", "tailUuid", "text", "text_elements", "thinking",
    "thinkingDurationMs", "thinking_tokens", "thread_id", "thread_source", "thread_token_usage",
    "time_to_first_token_ms", "timeout", "timestamp", "timezone", "toolDenialKind", "toolUseID", "toolUseId",
    "toolUseResult", "tool_use_id", "totalDurationMs", "totalLines", "totalTokens", "totalToolUseCount",
    "total_token_usage", "total_tokens", "trigger", "turnIndex", "turnOrigin", "turnPosition",
    "turn_attribution", "turn_id", "turn_token_usage", "turn_trigger", "type", "url", "usage", "userModified",
    "userType", "uuid", "uuids", "version", "web_fetch_requests", "web_search_requests", "window_id",
    "workspace_roots", "worktreeBranch", "worktreePath",
    # 앱 내부 도구 결과(JSON 문자열) 안의 키: ccd_pr, Codex exec 출력
    "auto_archive_on_close", "bound", "chunk_id", "monitor", "original_token_count", "wall_time_seconds",
}  # fmt: skip
ID_KEYS = {
    "uuid", "parentUuid", "logicalParentUuid", "sessionId", "session_id", "requestId", "id", "tool_use_id",
    "toolUseId", "toolUseID", "agentId", "promptId", "leafUuid", "sourceToolAssistantUUID", "messageId",
    "snapshotMessageId", "commandUuid", "call_id", "turn_id", "root_turn_id", "parent_turn_id", "thread_id",
    "response_id", "message_id", "client_id", "resumedAgentId", "recipient", "to", "task_id", "backgroundTaskId",
    "shell_id", "agent_id", "runId", "senderTaskId", "from", "first_turn_id", "receiver_message_id",
    "receiver_turn_id", "window_id", "first_window_id", "previous_window_id", "compaction_response_id",
    "revision", "anchorUuid", "headUuid", "tailUuid", "allUuids", "uuids", "plugin_id", "bridgeSessionId",
}  # fmt: skip
PATH_KEYS = {
    "file_path", "path", "filePath", "notebook_path", "cwd", "live_cwd", "root", "outputFile", "persistedOutputPath",
    "worktreePath", "realParentDir", "trackingPath", "workdir", "directory", "runtime_workspace_roots", "workspace_roots", "writable_roots",
}  # fmt: skip
URL_KEYS = {"url", "image_url", "frameUrl", "type_url"}
B64_KEYS = {"signature", "encrypted_content", "data", "base64"}
LITERALS = {"main", "", "general-purpose"}
TOOL_BLOCKS = {
    "tool_use",
    "mcp_tool_use",
    "server_tool_use",
    "function_call",
    "custom_tool_call",
    "function_call_output",
    "custom_tool_call_output",
}
# 파서와 무관한 부피 큰 키, 또는 사용자 값이 섞이는 키는 통째로 뺀다
DROP_KEYS = {
    "serverClassifierContext", "serverClassifierRequest", "wireIngestContext", "wireToolInputs", "diagnostics",
    "input_transformations", "slug", "attributionSkill", "attributionMcpServer", "attributionMcpTool",
    "attributionAgent", "base_instructions", "developer_instructions", "replacement_history", "model_catalog",
    "permission_profile", "file_system_sandbox_policy", "sandbox_policy", "thread_settings", "rate_limits",
    "metadata", "internal_chat_message_metadata_passthrough", "mcp_attribution", "last_agent_message",
}  # fmt: skip
# Claude 내장 다섯, Codex custom_tool_call 의 exec, Codex exec 안 JS 의 tools.<이름> 넷 (PLAN 4.6.2), 내부 도구
TOOL_NAMES = {
    "Agent", "Bash", "Edit", "Read", "Write", "exec", "exec_command", "write_stdin", "apply_patch", "web__run",
    "mcp__ccd_pr__bind_pr", "mcp__ccd_pr__get_status",
}  # fmt: skip
CCD_TOOL_RE = re.compile(
    r"mcp__ccd_[a-z_]+__[a-z_]+"
)  # 앱 내부 도구. 사용자가 이름을 짓지 않는다
LONG_FLAGS = {
    "--", "--job", "--log-failed", "--manifest-path", "--no-edit", "--repo", "--short", "--show-current", "--list",
}  # fmt: skip
SHORT_FLAG_RE = re.compile(r"-[A-Za-z]{1,3}")
KNOWN_EXTS = {
    "rs", "toml", "lock", "md", "json", "jsonl", "txt", "log", "output", "py", "sh", "ps1", "bat", "cmd", "exe",
    "ts", "tsx", "js", "jsx", "mjs", "css", "html", "yml", "yaml", "xml", "csv", "png", "jpg", "jpeg", "gif",
    "svg", "ico", "pdf", "zip", "gz", "tar", "dll", "wasm", "ini", "cfg", "c", "h", "cpp", "go", "java",
}  # fmt: skip
# toolUseResult 에서 파서가 보는 키만 남긴다
TUR_KEEP = {
    "isAsync", "status", "agentId", "description", "resolvedModel", "prompt", "outputFile", "canReadOutputFile",
    "canContinueAgent", "backgroundTaskId", "persistedOutputPath", "persistedOutputSize", "stdout", "stderr",
    "interrupted", "noOutputExpected", "isImage", "durationSeconds", "totalDurationMs", "totalTokens",
    "totalToolUseCount", "content", "agentType", "task_id", "task_type", "success", "filePath", "type", "file",
}  # fmt: skip
KEEP_RECORDS = {
    "user", "assistant", "system", "queue-operation", "custom-title", "agent-name", "pr-link", "attachment",
    "last-prompt", "mode", "atis-latch", "session_meta", "turn_context", "response_item", "event_msg",
    "token_usage_record", "compacted",
}  # fmt: skip

PROGS = {
    "cargo", "rustc", "rustup", "git", "gh", "npm", "npx", "pnpm", "yarn", "node", "pytest", "vitest", "jest",
    "go", "dotnet", "phpunit", "make", "tsc", "gradle", "mvn", "python", "python3", "py", "uv", "uvx", "ruff",
    "pip", "cd", "ls", "echo", "sleep", "cat", "head", "tail", "sed", "grep", "rg", "find", "fd", "wc", "sort",
    "mkdir", "rm", "cp", "mv", "pwd", "which", "test", "true", "false", "less", "awk", "curl", "winget", "wsl",
    "powershell", "pwsh", "cmd", "bash", "sh", "dumpbin", "tasklist", "taskkill", "set", "export", "diff",
}  # fmt: skip
KEEP_WORDS = {
    "status", "log", "show", "branch", "tag", "diff", "commit", "push", "pull", "add", "checkout", "switch",
    "merge", "rebase", "stash", "fetch", "clone", "init", "reset", "restore", "remote", "config", "test",
    "build", "check", "run", "install", "ci", "release", "pr", "create", "view", "list", "fmt", "clippy",
    "doc", "clean", "update", "publish", "dev", "lint", "start", "tauri", "format", "python", "pytest", "ruff",
    "node", "version", "help", "issue", "checks", "worktree", "prune", "pip", "vp", "typecheck",
}  # fmt: skip
STRUCT_TAGS = (
    "task-notification|summary|note|result|usage|worktree|worktreeBranch|persisted-output|system-reminder|"
    "agent-message|subagent_tokens|tool_uses|duration_ms|task-id|tool-use-id|output-file|status|worktreePath"
)
LITERAL_PHRASES = [
    (
        "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file "
        "edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to "
        "tell you how to proceed."
    ),
    "The user doesn't want to proceed with this tool use",
    "User rejected tool use",
    "Conversation compacted",
    "Full output saved to: ",
    "Async agent launched successfully.",
    "agentId: ",
    "output_file: ",
    "Script completed",
    "Script running",
    "Script failed",
    "Wall time",
    "Output:",
]
TEST_SUMMARY = (
    r"test result: (?:ok|FAILED)\. \d+ passed; \d+ failed(?:; \d+ ignored)?(?:; \d+ measured)?"
    r"(?:; \d+ filtered out)?|Tests:\s+\d+ failed, \d+ passed(?:, \d+ total)?|\d+ passed|\d+ failed|"
    r"[Ee]xit code \d+|Output too large \(\d+(?:\.\d+)? ?(?:B|KB|MB|GB)\)"
)
STRUCT_RE = re.compile(
    rf"(?P<kv><(?P<ktag>task-id|tool-use-id|output-file|status|subagent_tokens|tool_uses|duration_ms|worktreePath)>"
    rf"(?P<kval>[^<]*)</(?P=ktag)>)"
    rf"|(?P<tag></?(?:{STRUCT_TAGS})>)"
    rf"|(?P<lit>{'|'.join(re.escape(p) for p in LITERAL_PHRASES)})"
    rf"|(?P<test>{TEST_SUMMARY})"
    rf"|(?P<ids>{EMBEDDED_ID_RE.pattern})"
    rf"|(?P<spath>(?<=saved to: )\S+|(?<=output_file: )\S+)"
)
HANGUL = [chr(c) for c in range(0xAC00, 0xD7A4, 7)]
LOWER, UPPER, DIGIT, HEX = (
    "abcdefghijklmnopqrstuvwxyz",
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "0123456789",
    "0123456789abcdef",
)
B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"


class Anon:
    def __init__(self, delta_ms: int, seed: str):
        self.delta = timedelta(milliseconds=delta_ms)
        self.delta_ms = delta_ms
        self.rng = random.Random(
            seed
        )  # 시나리오 이름별로 고정. 시나리오끼리 식별자가 겹치지 않는다
        self.ids: dict[str, str] = {}
        self.used: set[str] = set()
        self.names: dict[str, str] = {}
        self.args: dict[str, str] = {}
        self.texts: dict[str, str] = {}
        self.keys: dict[str, str] = {}
        self.flags: dict[str, str] = {}
        self.aliases: dict[tuple[str, str], str] = {}
        self.cur = ""  # 지금 값을 처리 중인 키 (치환 보고용)
        self.dropped: Counter[tuple[str, str]] = Counter()

    def safe(self, k: str) -> str:
        """보고에 찍어도 되는 키 이름. 허용 집합 밖 키는 치환된 이름으로 찍는다."""
        if k in ALLOWED_KEYS or not k:
            return k or "(최상위)"
        return self.keys.get(k) or self.ids.get(k) or "?"

    def note(self, kind: str, key: str | None = None) -> None:
        self.dropped[(kind, self.safe(self.cur if key is None else key))] += 1

    def alias(self, kind: str, s: str) -> str:
        if (kind, s) not in self.aliases:
            n = sum(1 for k, _ in self.aliases if k == kind) + 1
            self.aliases[(kind, s)] = f"{kind}{n}"
        return self.aliases[(kind, s)]

    def tool_name(self, name: str) -> str:
        if name in TOOL_NAMES or CCD_TOOL_RE.fullmatch(name):
            return name
        self.note("도구 이름")
        m = re.fullmatch(r"mcp__(.+?)__(.+)", name)
        if m:
            return f"mcp__{self.alias('srv', m.group(1))}__{self.alias('tool', m.group(2))}"
        return self.alias("tool", name)

    # ---- 식별자 ----
    def fake_like(self, s: str) -> str:
        hexlike = all(c in "0123456789abcdef-" for c in s)
        out = []
        for c in s:
            if c in "-_.":
                out.append(c)
            elif c.isdigit():
                out.append(self.rng.choice(HEX if hexlike else DIGIT))
            elif c.isupper():
                out.append(self.rng.choice(UPPER))
            else:
                out.append(self.rng.choice(HEX if hexlike else LOWER))
        return "".join(out)

    def map_id(self, s: str) -> str:
        if s in self.ids:
            return self.ids[s]
        m = re.match(rf"^((?:{ID_PREFIXES})[_-])(.+)$", s)
        head, body = (m.group(1), m.group(2)) if m else ("", s)
        if re.fullmatch(r"a[0-9a-f]{16}", body):
            head, body = "a", body[1:]
        while True:
            fake = head + self.fake_like(body)
            if fake not in self.used:
                break
        self.used.add(fake)
        self.ids[s] = fake
        return fake

    # ---- 시각 ----
    def shift_iso(self, s: str) -> str | None:
        m = ISO_RE.match(s)
        if not m:
            return None
        try:
            dt = iso_wall(m) + self.delta
        except (ValueError, OverflowError):
            return None
        out = dt.strftime(f"%Y-%m-%d{m.group(4)}%H:%M:%S")
        frac = m.group(9) or ""
        if frac:
            out += m.group(8) + f"{dt.microsecond:06d}"[: len(frac)].ljust(
                len(frac), "0"
            )
        return out + (
            m.group(10) or ""
        )  # 오프셋은 벽시계와 함께 움직이므로 그대로 둔다

    def shift_num(self, v):
        """에포크로 보이는 숫자는 키 이름과 상관없이 옮긴다 (초 1e9~1e10, 밀리초 1e12~1e13)."""
        if isinstance(v, bool) or not isinstance(v, int | float):
            return v
        if 1e12 <= v < 1e13:
            return v + self.delta_ms
        if 1e9 <= v < 1e10:
            return (
                type(v)(v + self.delta_ms / 1000)
                if isinstance(v, float)
                else v + self.delta_ms // 1000
            )
        return v

    # ---- 글 ----
    def scramble(self, s: str) -> str:
        if s in self.texts:
            return self.texts[s]
        out = []
        for c in s:
            if c in " \t\r\n":
                out.append(c)
            elif "a" <= c <= "z":
                out.append(self.rng.choice(LOWER))
            elif "A" <= c <= "Z":
                out.append(self.rng.choice(UPPER))
            elif c.isdigit():
                out.append(self.rng.choice(DIGIT))
            elif "가" <= c <= "힣":
                out.append(self.rng.choice(HANGUL))
            elif c == "@":
                out.append("_")
            elif ord(c) > 127 and unicodedata.category(c)[0] in "LMN":
                out.append(self.rng.choice(LOWER))
            else:
                out.append(c)
        self.texts[s] = "".join(out)
        return self.texts[s]

    @staticmethod
    def cap(s: str, n: int = TEXT_CAP) -> str:
        if len(s) <= n:
            return s
        tail = "\n".join(s.rstrip("\n").split("\n")[-2:])[-160:]
        return s[: max(n - len(tail), 0)] + "\n...\n" + tail

    def text(self, s: str) -> str:
        if "<task-notification>" in s:  # 본문이 긴 알림도 <usage> 까지 살아남게 한다
            s = re.sub(
                r"(<(?:result|summary)>)(.{200}).*?(</(?:result|summary)>)",
                r"\1\2\3",
                s,
                flags=re.DOTALL,
            )
        s = self.cap(s, 1500 if "<task-notification>" in s else TEXT_CAP)
        out, pos = [], 0
        for m in STRUCT_RE.finditer(s):
            out.append(self.scramble(s[pos : m.start()]))
            pos = m.end()
            if m.group("kv"):
                tag, val = m.group("ktag"), m.group("kval")
                if not val:
                    pass
                elif tag in ("task-id", "tool-use-id"):
                    val = self.map_id(val)
                elif tag in ("output-file", "worktreePath"):
                    val = self.path(val)
                elif tag == "status":
                    if val not in ENUM_VALUES["status"]:
                        self.dropped[("태그 값", tag)] += 1
                        val = self.scramble(val)
                elif not re.fullmatch(r"[0-9]+", val):
                    self.dropped[("태그 값", tag)] += 1
                    val = self.scramble(val)
                out.append(f"<{tag}>{val}</{tag}>")
            elif m.group("ids"):
                out.append(self.map_id(m.group("ids")))
            elif m.group("spath"):
                out.append(self.path(m.group("spath")))
            else:
                out.append(m.group(0))
        out.append(self.scramble(s[pos:]))
        return "".join(out)

    # ---- 경로 ----
    def comp(self, c: str) -> str:
        if c in (".", "..", "~", "") or set(c) <= set("*?[]{}") or c in SAFE_COMPS:
            return c
        if UUID_RE.fullmatch(c) or EMBEDDED_ID_RE.fullmatch(c):
            return self.map_id(c)
        stem, dot, ext = c.rpartition(".")
        if not dot or not stem or ext.lower() not in KNOWN_EXTS:
            if dot and stem:  # john.smith 같은 조각은 통째로 하나의 이름이다
                self.note("확장자")
            stem, ext = c, ""
        m = re.fullmatch(r"agent-(a[0-9a-f]{16})", stem)
        if m:
            new = "agent-" + self.map_id(m.group(1))
        elif EMBEDDED_ID_RE.fullmatch(stem) or LOOKS_ID_COMP.match(stem):
            new = self.map_id(stem)  # 다른 필드의 같은 id 와 같은 가짜 값이 된다
        elif stem == "*":
            new = "*"
        else:
            if stem not in self.names:
                self.names[stem] = str(len(self.names) + 1)
            new = ("file" if ext else "dir") + self.names[stem]
        return new + ("." + ext if ext else "")

    def path(self, p: str) -> str:
        if not p or len(p) > 400:
            return self.text(p)
        prefix = ""
        for pre in ("file:///", "\\\\?\\", "\\\\"):
            if p.startswith(pre):
                prefix, p = pre, p[len(pre) :]
                break
        root = ""
        m = re.match(r"^([A-Za-z]):(?=[\\/]|$)", p)
        if m:
            root, p = "C:", p[2:]
        else:
            m = re.match(r"^(/mnt)?/([A-Za-z])(?=/|$)", p)
            if m:
                root, p = (m.group(1) or "") + "/c", p[m.end() :]
        parts = re.split(r"([\\/]+)", p)
        out = [self.comp(c) if i % 2 == 0 else c for i, c in enumerate(parts)]
        return prefix + root + "".join(out)

    # ---- 명령 ----
    def arg(self, run: str, index: int, prog_ok: bool) -> str:
        if SHORT_FLAG_RE.fullmatch(run) or run in LONG_FLAGS:
            return run
        if re.fullmatch(r"\d*[<>]+&?\d*|/dev/null|\.\.?", run):
            return run
        if prog_ok and index in (1, 2) and run in KEEP_WORDS:
            return run
        if "=" in run and run.startswith("-"):
            flag, _, val = run.partition("=")
            return self.arg(flag, 99, False) + "=" + self.arg(val, 99, False)
        # 집합 밖 긴 플래그. -pSecret 같은 짧은 꼴은 아래에서 인자로 치환된다
        if run.startswith("--"):
            self.note("플래그")
            return self.flags.setdefault(run, f"--flag{len(self.flags) + 1}")
        if re.search(r"[\\/]|^~|\.[A-Za-z0-9]{1,5}$", run):
            return self.path(run)
        if run not in self.args:
            self.args[run] = f"arg{len(self.args) + 1}"
        return self.args[run]

    def command(self, cmd: str) -> str:
        cmd = self.cap(cmd, 600)
        head, sep, body = cmd.partition("<<")  # heredoc 본문은 글로 본다
        if sep:
            first, nl, rest = body.partition("\n")
            return (
                self.command(head) + sep + self.scramble(first) + nl + self.text(rest)
            )
        out, at_start, idx, prog_ok = [], True, 0, False
        for tok in split_shell(cmd):
            if tok.isspace():
                out.append(tok)
                at_start = at_start or "\n" in tok
                idx = 0 if "\n" in tok else idx
            elif tok in ("&&", "||", "|", ";", "&"):
                out.append(tok)
                at_start, idx = True, 0
            elif at_start:
                lead, core, trail = re.match(
                    r"^([$(`{]*)(.*?)([)`}]*)$", tok, re.DOTALL
                ).groups()  # type: ignore[union-attr]
                if m := re.match(
                    r"^([A-Za-z_]\w*)=(.*)$", core, re.DOTALL
                ):  # 환경변수 지정은 다음 토큰이 프로그램
                    var = self.arg(m.group(1), 99, False).replace("arg", "VAR", 1)
                    out.append(
                        lead + var + "=" + self.word(m.group(2), 99, False) + trail
                    )
                    continue
                base = (
                    core.replace("\\", "/")
                    .rsplit("/", 1)[-1]
                    .lower()
                    .removesuffix(".exe")
                )
                if base in PROGS:
                    out.append(
                        lead
                        + (self.path(core) if "/" in core.replace("\\", "/") else core)
                        + trail
                    )
                    prog_ok = base in SUBCMD_PROGS
                else:
                    out.append(
                        lead
                        + self.arg(core, 99, False).replace("arg", "prog", 1)
                        + trail
                    )
                    prog_ok = False
                at_start = False
            else:
                idx += 1
                out.append(self.word(tok, idx, prog_ok))
        return "".join(out)

    def word(self, tok: str, idx: int, prog_ok: bool) -> str:
        qm = re.fullmatch(r"""(["'])(.*)\1""", tok, flags=re.DOTALL)
        if qm and re.search(r"\s", qm.group(2)):
            return qm.group(1) + self.scramble(qm.group(2)) + qm.group(1)
        runs = re.split(r"""([^\s$(){}`"'|&;<>=]+)""", tok)
        out = []
        for i, r in enumerate(runs):
            if i % 2 == 0:
                out.append(r)
            elif (
                r.isascii()
                and r.isdigit()
                and (runs[i + 1][:1] in ("<", ">") or re.search(r"[<>]&$", runs[i - 1]))
            ):  # 2>&1 같은 파일 디스크립터
                out.append(r)
            elif runs[i - 1].endswith("="):  # --x=값 의 값은 서브커맨드 자리가 아니다
                out.append(self.arg(r, 99, False))
            else:
                out.append(self.arg(r, idx, prog_ok))
        return "".join(out)

    def js(self, code: str) -> str:
        """Codex exec 의 JS 입력. tools.<이름>, 인자 키, cmd 안의 셸 명령만 구조로 남긴다."""
        keep = r"tools\.[A-Za-z_]\w*|\b(?:await|text|workdir|yield_time_ms|max_output_tokens|session_id|chars)\b"
        pat = re.compile(
            rf'(?P<cmd>\bcmd\s*:\s*)"(?P<val>(?:[^"\\]|\\.)*)"|(?P<keep>{keep})'
        )
        code = self.cap(code, 800)
        out, pos = [], 0
        for m in pat.finditer(code):
            out.append(self.scramble(code[pos : m.start()]))
            pos = m.end()
            if m.group("cmd"):
                try:
                    inner = json.loads('"' + m.group("val") + '"')
                    out.append(
                        m.group("cmd")
                        + json.dumps(self.command(inner), ensure_ascii=False)
                    )
                except ValueError:
                    out.append(
                        m.group("cmd") + '"' + self.scramble(m.group("val")) + '"'
                    )
            elif m.group("keep").startswith("tools."):
                out.append("tools." + self.tool_name(m.group("keep")[6:]))
            else:
                out.append(m.group(0))
        out.append(self.scramble(code[pos:]))
        return "".join(out)

    def url(self, u: str) -> str:
        if u.startswith("data:"):
            return "data:image/png;base64," + self.b64(u[:64], 16)
        m = re.match(r"^(https?://)([^/?#]*)(.*)$", u)
        if not m:
            return self.text(u)
        host = self.args.setdefault(
            "host:" + m.group(2), f"host{len(self.args) + 1}.example"
        )
        return m.group(1) + host + self.scramble(m.group(3)[:80])

    def b64(self, s: str, n: int | None = None) -> str:
        return "".join(
            self.rng.choice(B64) for _ in range(min(len(s), 200) if n is None else n)
        )

    # ---- 구조 ----
    def key(self, k: str, parent: str) -> str:
        if EMBEDDED_ID_RE.fullmatch(k):
            return self.map_id(k)
        if k in ALLOWED_KEYS:
            return k
        self.note("키", parent)
        if k not in self.keys:
            self.keys[k] = f"k{len(self.keys) + 1}"
        return self.keys[k]

    def value(self, v, key: str = "", parent: str = "", ctx: dict | None = None):
        if isinstance(v, dict):
            return self.dict(v, key)
        if isinstance(v, list):
            if key == "command" and v and all(isinstance(x, str) for x in v):
                return [self.argv(x) for x in v]
            return [self.value(x, key, parent, ctx) for x in v]
        if isinstance(v, str):
            return self.string(v, key, ctx or {})
        return self.shift_num(v)

    def argv(self, x: str) -> str:
        if x.startswith("-"):
            return self.arg(x, 99, False)
        if re.search(r"[\\/]", x) and " " not in x and len(x) < 200:
            return self.path(x)
        return self.command(x)

    def dict(self, d: dict, parent: str = "") -> dict:
        out = {}
        typ = d.get("type") if isinstance(d.get("type"), str) else ""
        for k, x in d.items():
            if k in DROP_KEYS or (parent == "toolUseResult" and k not in TUR_KEEP):
                continue
            kk = self.key(k, parent)
            self.cur = k
            if typ in TOOL_BLOCKS and k == "name" and isinstance(x, str):
                out[kk] = self.tool_name(x)
            elif k == "toolUseResult" and not isinstance(x, dict):
                out[kk] = self.text(x) if isinstance(x, str) else None
            else:
                out[kk] = self.value(x, k, parent, {"type": typ, "name": d.get("name")})
        return out

    def string(self, s: str, key: str, ctx: dict) -> str:
        if key in PATH_KEYS:
            return self.path(s)
        if key in URL_KEYS:
            return self.url(s)
        if key == "command" and ctx.get("type") in ("tool_use", ""):
            return self.command(s)
        if key == "input" and ctx.get("name") == "exec":
            return self.js(s)
        if key == "timezone":
            return "UTC"
        if key in ID_KEYS and len(s) >= 8:
            return self.map_id(s)
        if (iso := self.shift_iso(s)) is not None:
            return iso
        if DATE_RE.match(s):
            return (
                datetime.fromisoformat(s).replace(tzinfo=UTC) + self.delta
            ).strftime("%Y-%m-%d")
        if s in LITERALS:
            return s
        if key in ENUM_VALUES:
            if s in ENUM_VALUES[key]:
                return s
            self.note("열거 값", key)
        if key in B64_KEYS and len(s) > 40:
            return self.b64(s, min(len(s), 300) if key == "signature" else 64)
        if s[:1] in "{[" and len(s) < 200_000:
            try:
                parsed = json.loads(s)
            except ValueError:
                parsed = None
            if isinstance(parsed, dict | list):
                return json.dumps(
                    self.value(parsed, key), ensure_ascii=False, separators=(",", ":")
                )
        return self.text(s)

    def record(self, r: dict) -> dict | None:
        t = r.get("type")
        if t not in KEEP_RECORDS:
            return None
        old_att = r.get("attachment") or {}
        if (
            t == "attachment"
        ):  # 파서가 무시하는 부피 큰 주입물. 공통 필드와 종류만 남긴다
            common = {
                "parentUuid",
                "isSidechain",
                "type",
                "uuid",
                "timestamp",
                "userType",
                "entrypoint",
            }
            common |= {"cwd", "sessionId", "version", "gitBranch", "agentId"}
            r = {k: v for k, v in r.items() if k in common}
            r["attachment"] = {"type": old_att.get("type")}
        return self.dict(r)


def split_shell(cmd: str) -> list[str]:
    """따옴표 안의 구분자는 자르지 않는 간단한 분해 (PLAN 4.6.4와 같은 규칙)."""
    parts: list[str] = []
    cur, quote, i = "", "", 0
    while i < len(cmd):
        c = cmd[i]
        if quote:
            cur += c
            if c == "\\" and quote == '"' and i + 1 < len(cmd):
                cur += cmd[i + 1]
                i += 1
            elif c == quote:
                quote = ""
        elif c in "\"'":
            quote, cur = c, cur + c
        elif c.isspace():
            if cur:
                parts.append(cur)
                cur = ""
            j = i
            while j < len(cmd) and cmd[j].isspace():
                j += 1
            parts.append(cmd[i:j])
            i = j
            continue
        elif c in "|&;" and not (c == "&" and cur.endswith(">")):
            if cur:
                parts.append(cur)
                cur = ""
            if cmd[i : i + 2] in ("&&", "||"):
                parts.append(cmd[i : i + 2])
                i += 2
                continue
            parts.append(c)
        else:
            cur += c
        i += 1
    return parts + ([cur] if cur else [])


SUBCMD_PROGS = {
    "git",
    "gh",
    "cargo",
    "npm",
    "npx",
    "pnpm",
    "uv",
    "uvx",
    "go",
    "dotnet",
    "python",
    "python3",
    "make",
}
SAFE_COMPS = {
    "tool-results",
    "subagents",
    "tasks",
    ".claude",
    ".codex",
    "projects",
    "sessions",
    ".git",
    "wsl.localhost",
}


# ---- 입출력 ----
def parse_spec(spec: str | None, n: int) -> list[int]:
    if not spec:
        return list(range(n))
    idx: list[int] = []
    for part in spec.split(","):
        a, _, b = part.partition("-")
        idx.extend(range(int(a) - 1, min(int(b or a), n)))
    return idx


def load(path: Path, spec: str | None) -> list[dict]:
    lines = path.read_text(encoding="utf-8").splitlines()
    return [json.loads(lines[i]) for i in parse_spec(spec, len(lines))]


def iso_wall(m: re.Match) -> datetime:
    """오프셋은 무시한 벽시계 시각. 평행이동은 벽시계에 더해도 같은 결과다."""
    frac = (m.group(9) or "") + "000000"
    ymd = (int(m.group(i)) for i in (1, 2, 3, 5, 6, 7))
    return datetime(*ymd, int(frac[:6]), tzinfo=UTC)


def first_ts(records: list[dict]) -> datetime | None:
    for r in records:
        m = ISO_RE.match(r.get("timestamp") or "")
        if m:
            return iso_wall(m)
    return None


def write_jsonl(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as f:
        for r in records:
            f.write(json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n")


def convert(
    kind: str, src: Path, out: Path, spec: str | None, agents: list[str]
) -> None:
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
        sys.exit("타임스탬프가 있는 레코드가 없다")
    an = Anon(int((BASE_TS - min(stamps)).total_seconds() * 1000), out.name)
    sid = UUID_RE.search(src.stem).group(0)  # type: ignore[union-attr]
    if kind == "claude":
        base = out / f"{an.map_id(sid)}"
        write_jsonl(
            base.with_suffix(".jsonl"), [x for r in main if (x := an.record(r))]
        )
    else:
        shifted = an.shift_iso(main[0]["timestamp"]) or ""
        name = f"rollout-{shifted[:19].replace(':', '-')}-{an.map_id(sid)}.jsonl"
        write_jsonl(out / name, [x for r in main if (x := an.record(r))])
    for aid, recs, meta in subs:
        stem = base / "subagents" / f"agent-{an.map_id(aid)}"
        write_jsonl(
            stem.with_suffix(".jsonl"), [x for r in recs if (x := an.record(r))]
        )
        if meta.exists():
            m = an.dict(json.loads(meta.read_text(encoding="utf-8")))
            stem.with_suffix(".meta.json").write_text(
                json.dumps(m, ensure_ascii=False, separators=(",", ":")),
                encoding="utf-8",
                newline="\n",
            )
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
    files = [p for p in sorted(root.rglob("*")) if p.is_file()]
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
        "dir", nargs="?", type=Path, default=Path("fixtures")
    )
    a = ap.parse_args()
    if a.cmd == "check":
        return check(a.dir)
    if a.cmd == "codex" and a.agent:
        sys.exit("codex 에는 --agent 가 없다")
    convert(a.cmd, a.src, a.out, a.lines, a.agent)
    return 0


if __name__ == "__main__":
    sys.exit(main())
