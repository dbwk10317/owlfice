"""릴리스·익명화 CLI의 입력 경계와 파일 출력을 합성 자료로 검사한다."""

import io
import json
import tempfile
import unittest
from contextlib import chdir, redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

import anonymize
import release_check
import release_manifest
from anonymize_core import Anon


def run_main(module, *args):
    stdout, stderr = io.StringIO(), io.StringIO()
    with (
        patch("sys.argv", [module.__file__, *map(str, args)]),
        redirect_stdout(stdout),
        redirect_stderr(stderr),
    ):
        code = module.main()
    return code, stdout.getvalue(), stderr.getvalue()


class ReleaseManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.installer = self.root / "Owlfice_1.2.3_x64-setup.exe"
        self.installer.write_bytes(b"synthetic installer")
        self.signature = "untrusted comment: 합성 서명\r\nsynthetic-signature\r\n"
        self.installer.with_name(self.installer.name + ".sig").write_bytes(
            self.signature.encode("utf-8")
        )

    def test_output_is_utf8_without_bom_uses_lf_and_preserves_signature(self):
        target = self.root / "latest.json"
        code, stdout, stderr = run_main(
            release_manifest, "1.2.3", self.installer, "--output", target
        )
        self.assertEqual((code, stdout, stderr), (0, "", ""))
        data = target.read_bytes()
        self.assertFalse(data.startswith(b"\xef\xbb\xbf"))
        self.assertNotIn(b"\r", data)
        self.assertTrue(data.endswith(b"\n"))
        manifest = json.loads(data.decode("utf-8"))
        self.assertEqual(
            manifest["platforms"]["windows-x86_64"]["signature"], self.signature
        )
        self.assertEqual(
            release_manifest.read_signature(self.installer, "1.2.3"),
            self.signature,
        )

    def test_stdout_output_remains_json(self):
        code, stdout, stderr = run_main(release_manifest, "1.2.3", self.installer)
        self.assertEqual((code, stderr), (0, ""))
        self.assertEqual(json.loads(stdout)["version"], "1.2.3")

    def test_input_and_output_errors_return_failure_without_traceback(self):
        unsigned = self.root / "unsigned" / self.installer.name
        unsigned.parent.mkdir()
        unsigned.write_bytes(b"synthetic installer")
        for args in (
            ["1.2.3", self.root / "missing" / self.installer.name],
            ["1.2.3", unsigned],
            ["1.2.3", self.installer, "--output", self.root],
        ):
            with self.subTest(args=args):
                code, stdout, stderr = run_main(release_manifest, *args)
                self.assertEqual((code, stdout), (1, ""))
                self.assertIn("release_manifest:", stderr)
                self.assertNotIn("Traceback", stderr)

    def test_stdout_oserror_is_reported(self):
        with patch.object(
            release_manifest,
            "print",
            side_effect=[OSError("출력 실패"), None],
            create=True,
        ) as output:
            code, _, _ = run_main(release_manifest, "1.2.3", self.installer)
        self.assertEqual(code, 1)
        self.assertIn("출력 실패", output.call_args.args[0])


class ReleaseCheckTests(unittest.TestCase):
    def test_missing_manifest_fails_before_opening_files(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        with (
            tempfile.TemporaryDirectory() as directory,
            chdir(directory),
            patch("sys.argv", [release_check.__file__, "1.2.3"]),
            patch.object(Path, "open") as file_open,
            redirect_stdout(stdout),
            redirect_stderr(stderr),
            self.assertRaises(SystemExit) as exit_error,
        ):
            release_check.main()
        self.assertEqual(exit_error.exception.code, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("required: --manifest", stderr.getvalue())
        file_open.assert_not_called()


class AnonymizeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_check_default_is_repo_relative_from_another_cwd(self):
        fixtures = self.root / "repo" / "fixtures"
        fixtures.mkdir(parents=True)
        (fixtures / "synthetic.jsonl").write_text("{}\n", encoding="utf-8")
        elsewhere = self.root / "elsewhere"
        elsewhere.mkdir()
        with (
            patch.object(anonymize, "ROOT", fixtures.parent),
            patch.object(anonymize, "real_values", return_value={}),
            chdir(elsewhere),
        ):
            code, stdout, stderr = run_main(anonymize, "check")
        self.assertEqual((code, stderr), (0, ""))
        self.assertIn("검사 파일 1개", stdout)

    def test_check_missing_empty_and_directories_only_fail_before_home_scan(self):
        empty = self.root / "empty"
        empty.mkdir()
        directories = self.root / "directories"
        (directories / "nested").mkdir(parents=True)
        file = self.root / "file.jsonl"
        file.write_text("{}\n", encoding="utf-8")
        for target in (self.root / "missing", empty, directories, file):
            with (
                self.subTest(target=target),
                patch.object(anonymize, "real_values") as values,
            ):
                code, stdout, stderr = run_main(anonymize, "check", target)
                self.assertEqual((code, stdout), (1, ""))
                self.assertIn("검사 대상", stderr)
                self.assertNotIn("Traceback", stderr)
                values.assert_not_called()

    def test_line_selection_boundaries(self):
        self.assertEqual(anonymize.parse_spec(None, 3), [0, 1, 2])
        self.assertEqual(anonymize.parse_spec("1, 2-3", 3), [0, 1, 2])
        for spec in (
            "",
            "0",
            "-1",
            "3-2",
            "1-4",
            "4",
            "one",
            "1-x",
            "1,",
            "1-",
            "1--2",
        ):
            with self.subTest(spec=spec), self.assertRaises(ValueError):
                anonymize.parse_spec(spec, 3)

    def test_missing_uuid_fails_before_reading_input(self):
        with patch.object(anonymize, "load") as load:
            code, stdout, stderr = run_main(
                anonymize, "claude", self.root / "no-id.jsonl", self.root / "out"
            )
        self.assertEqual((code, stdout), (1, ""))
        self.assertIn("UUID", stderr)
        self.assertNotIn("Traceback", stderr)
        load.assert_not_called()

    def test_cli_bad_range_and_json_do_not_leak_source(self):
        source = self.root / "00000000-0000-4000-8000-000000000001.jsonl"
        source.write_text('{"private-content": invalid}\n', encoding="utf-8")
        for options in ([], ["--lines", "0"], ["--lines", "2"], ["--lines", "bad"]):
            with self.subTest(options=options):
                code, stdout, stderr = run_main(
                    anonymize, "claude", source, self.root / "out", *options
                )
                self.assertEqual((code, stdout), (1, ""))
                self.assertIn("anonymize:", stderr)
                self.assertNotIn("Traceback", stderr)
                self.assertNotIn("private-content", stderr)
        source.write_bytes(b"\xffprivate-content\n")
        code, _, stderr = run_main(anonymize, "claude", source, self.root / "out")
        self.assertEqual(code, 1)
        self.assertIn("UTF-8", stderr)
        self.assertNotIn("private-content", stderr)

    def test_timestamp_shift_preserves_offset_and_intervals(self):
        anon = Anon(60_000, "synthetic")
        for zone in ("+09:00", "-0530", "Z"):
            with self.subTest(zone=zone):
                first = anon.shift_iso(f"2025-01-01T23:59:30.123{zone}")
                second = anon.shift_iso(f"2025-01-02T00:00:00.123{zone}")
                self.assertEqual(first, f"2025-01-02T00:00:30.123{zone}")
                self.assertEqual(second, f"2025-01-02T00:01:00.123{zone}")

    def test_non_object_record_reports_line_without_writing(self):
        source = self.root / "00000000-0000-4000-8000-000000000001.jsonl"
        source.write_text(
            '{"type":"user","timestamp":"2025-01-01T00:00:00Z"}\n42\n',
            encoding="utf-8",
        )
        out = self.root / "out"
        code, stdout, stderr = run_main(anonymize, "claude", source, out)
        self.assertEqual((code, stdout), (1, ""))
        self.assertIn(f"{source}:2", stderr)
        self.assertNotIn("Traceback", stderr)
        self.assertFalse(out.exists())

    def test_codex_uses_first_valid_main_timestamp_for_name(self):
        source = self.root / "rollout-00000000-0000-4000-8000-000000000001.jsonl"
        for label, first in (
            ("missing", '{"type":"session_meta"}'),
            ("null", '{"type":"session_meta","timestamp":null}'),
            ("empty", '{"type":"session_meta","timestamp":""}'),
        ):
            with self.subTest(label=label):
                source.write_text(
                    first
                    + '\n{"type":"event_msg","timestamp":"2025-01-01T00:00:00Z"}\n',
                    encoding="utf-8",
                )
                out = self.root / label
                code, stdout, stderr = run_main(anonymize, "codex", source, out)
                self.assertEqual((code, stdout, stderr), (0, "", ""))
                files = list(out.glob("rollout-2026-03-02T09-00-00-*.jsonl"))
                self.assertEqual(len(files), 1)
                self.assertEqual(
                    json.loads(files[0].read_text(encoding="utf-8").splitlines()[0]),
                    json.loads(first),
                )

    def test_bad_meta_preserves_existing_output(self):
        source = self.root / "00000000-0000-4000-8000-000000000001.jsonl"
        source.write_text(
            '{"type":"user","timestamp":"2025-01-01T00:00:00Z"}\n',
            encoding="utf-8",
        )
        sub = source.with_suffix("") / "subagents"
        sub.mkdir(parents=True)
        (sub / "agent-agent1.jsonl").write_text(
            '{"type":"user","timestamp":"2025-01-01T00:00:01Z"}\n',
            encoding="utf-8",
        )
        meta = sub / "agent-agent1.meta.json"
        meta.write_text('{"type":"agent-name"}', encoding="utf-8")
        out = self.root / "out"
        self.assertEqual(
            run_main(anonymize, "claude", source, out, "--agent", "agent1")[0],
            0,
        )
        existing = {p: p.read_bytes() for p in out.rglob("*") if p.is_file()}
        meta.write_text("[1]", encoding="utf-8")
        code, stdout, stderr = run_main(
            anonymize, "claude", source, out, "--agent", "agent1"
        )
        self.assertEqual((code, stdout), (1, ""))
        self.assertIn(f"{meta}:1", stderr)
        self.assertNotIn("Traceback", stderr)
        self.assertEqual(
            {p: p.read_bytes() for p in out.rglob("*") if p.is_file()}, existing
        )
        fresh = self.root / "fresh"
        self.assertEqual(
            run_main(anonymize, "claude", source, fresh, "--agent", "agent1")[0],
            1,
        )
        self.assertFalse(fresh.exists())

    def test_bad_timestamp_reports_line_without_writing(self):
        source = self.root / "00000000-0000-4000-8000-000000000001.jsonl"
        for label, timestamp in (
            ("wrong-type", 42),
            ("bad-calendar", "2025-99-01T00:00:00Z"),
        ):
            with self.subTest(label=label):
                source.write_text(
                    json.dumps({"type": "user", "timestamp": timestamp}) + "\n",
                    encoding="utf-8",
                )
                out = self.root / label
                code, stdout, stderr = run_main(anonymize, "claude", source, out)
                self.assertEqual((code, stdout), (1, ""))
                self.assertIn(f"{source}:1", stderr)
                self.assertNotIn("Traceback", stderr)
                self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
