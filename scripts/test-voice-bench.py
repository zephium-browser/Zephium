#!/usr/bin/env python3
"""Network-free runner checks. Never starts the real gateway or signed probe."""
import importlib.machinery
import importlib.util
import contextlib
import io
import json
import os
import shlex
import sys
import tempfile
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader("voice_bench", os.path.join(os.path.dirname(__file__), "voice-bench"))
spec = importlib.util.spec_from_loader(loader.name, loader)
bench = importlib.util.module_from_spec(spec)
loader.exec_module(bench)


class VoiceBenchTests(unittest.TestCase):
    def test_voice_list_is_available_without_a_binary(self):
        self.assertEqual(len(bench.scenarios()), 8)
        self.assertEqual(bench.scenarios()["voice-count"], "count-only")

    def test_ledger_retains_only_counted_contract_fields(self):
        row = {"route": "listen", "state": "settled", "error_class": None, "count": 1,
               "charged": "0.01", "reserved": "0.02", "usage": {"characters": 3, "text": "discard"},
               "unexpected": "discard"}
        clean = bench.ledger([row])
        self.assertNotIn("discard", json.dumps(clean))
        for field, value in (("route", "private"), ("state", "unexpected"),
                             ("count", True), ("charged", "NaN"), ("error_class", "raw text"), ("error_class", "private_payload")):
            self.assertIsNone(bench.ledger([dict(row, **{field: value})]))

    def test_queued_unadmitted_speech_is_not_a_duplicate_bill(self):
        rows = [{"scenario": "voice-barge", "synthesis_speeches": 2, "voice": {"counts": {
            "utterances": 2, "speeches": 5, "ledger_listen_receipts": 2, "ledger_speak_receipts": 3}}}]
        summary = [{"route": "listen", "count": 2, "state": "settled", "charged": "0.1"},
                   {"route": "speak", "count": 5, "state": "settled", "charged": "0.2"}]
        result = bench.billing(rows, summary)
        self.assertTrue(result["counts_match"])
        self.assertEqual(result["charged"], "0.3")
        summary[1]["count"] = 6
        self.assertFalse(bench.billing(rows, summary)["counts_match"])
        self.assertFalse(bench.billing(rows, None)["all_terminal"])

    def test_an_empty_final_is_not_fabricated_as_a_short_ledger_case(self):
        rows = [{"scenario": "voice-silence", "voice": {"counts": {"utterances": 1, "ledger_listen_receipts": 1}}}]
        summary = [{"route": "listen", "count": 1, "state": "settled", "charged": "0.1", "error_class": None}]
        result = bench.billing(rows, summary)
        self.assertTrue(result["counts_match"])
        self.assertFalse(result["silence_ledger_classified"])

    def test_missing_measurements_cannot_pass_the_bars(self):
        checks = bench.checks({"scenario": "voice-barge", "false_done": False})
        self.assertFalse(checks["ack_under_800ms"])
        self.assertFalse(checks["barge_under_200ms"])
        self.assertFalse(checks["gateway_cancel_acknowledged"])
        self.assertFalse(checks["transcribed"])

    def test_silence_requires_an_empty_final_and_no_reply(self):
        row = {"scenario": "voice-silence", "false_done": False, "voice": {
            "counts": {"intact_utterances": 1}, "transcripts": [{"expected_words": True, "word_count": 0}]}}
        self.assertTrue(all(bench.checks(row).values()))
        row["voice"]["counts"]["assistant_asks"] = 1
        self.assertFalse(bench.checks(row)["quiet"])

    def test_gateway_reports_missing_key_without_forwarding_output(self):
        command = shlex.quote(sys.executable) + " -c " + shlex.quote('print(\'{"ready":false,"configured":false}\')')
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"VOICE_BENCH_GATEWAY": command}):
            gateway = bench.Gateway(directory)
            with patch.object(bench, "BACKEND", directory):
                try:
                    with self.assertRaisesRegex(bench.BenchError, "missing provider key"):
                        gateway.start()
                finally:
                    gateway.close()
            self.assertIsNotNone(gateway.child.poll())

    def test_gateway_stops_on_term_and_collects_only_safe_summary(self):
        # The fake ready file is empty: the runner must never read credentials.
        code = '''import json,os,signal,time
p=os.environ['VOICE_LOCAL_READY']
fd=os.open(p,os.O_WRONLY|os.O_CREAT,0o600);os.close(fd)
def stop(*_):
 time.sleep(0.05)
 print(json.dumps({'summary':[{'route':'listen','state':'released','error_class':'short_audio','count':1,'charged':'0'}]}),flush=True)
 raise SystemExit(0)
signal.signal(signal.SIGTERM,stop)
print(json.dumps({'ready':True,'port':1234}),flush=True)
while True: time.sleep(0.01)
'''
        command = shlex.quote(sys.executable) + " -c " + shlex.quote(code)
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"VOICE_BENCH_GATEWAY": command}):
            gateway = bench.Gateway(directory)
            with patch.object(bench, "BACKEND", directory):
                try:
                    gateway.start()
                finally:
                    gateway.close()
            self.assertEqual(gateway.summary[0]["error_class"], "short_audio")
            self.assertIsNotNone(gateway.child.poll())

    def test_readiness_timeout_still_stops_the_child(self):
        command = "exec " + shlex.quote(sys.executable) + " -c " + shlex.quote("import time; time.sleep(60)")
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"VOICE_BENCH_GATEWAY": command}):
            gateway = bench.Gateway(directory)
            with patch.object(bench, "BACKEND", directory), patch.object(bench, "READY_TIMEOUT", 0.01):
                try:
                    with self.assertRaisesRegex(bench.BenchError, "did not become ready"):
                        gateway.start()
                finally:
                    gateway.close()
            self.assertIsNotNone(gateway.child.poll())

    def test_ctrl_c_removes_private_directory_and_reaps_gateway(self):
        code = """import json,os,time
p=os.environ['VOICE_LOCAL_READY']
fd=os.open(p,os.O_WRONLY|os.O_CREAT,0o600);os.close(fd)
print(json.dumps({'ready':True,'port':1234}),flush=True)
while True: time.sleep(0.01)
"""
        command = "exec " + shlex.quote(sys.executable) + " -c " + shlex.quote(code)
        gateways = []
        real_gateway = bench.Gateway
        def create(directory):
            gateway = real_gateway(directory)
            gateways.append(gateway)
            return gateway
        previous_umask = os.umask(0o077)
        try:
            with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"VOICE_BENCH_GATEWAY": command}), contextlib.redirect_stdout(io.StringIO()):
                with patch.object(bench, "BACKEND", directory), patch.object(bench, "OUT", os.path.join(directory, "results")), patch.object(bench, "Gateway", side_effect=create), patch.object(bench.assist, "prepare_probe", return_value=("unused-test-probe", "test-hash", [])), patch.object(bench, "run_one", side_effect=KeyboardInterrupt):
                    with self.assertRaisesRegex(bench.BenchError, "interrupted"):
                        bench.run(["voice-fact"])
                self.assertIsNotNone(gateways[0].child.poll())
                self.assertFalse(os.path.exists(os.path.dirname(gateways[0].path)))
                result = os.path.join(directory, "results", os.listdir(os.path.join(directory, "results"))[0])
                self.assertEqual(os.stat(result).st_mode & 0o777, 0o600)
        finally:
            os.umask(previous_umask)

    def test_repeat_aggregation_handles_even_odd_missing_and_zero(self):
        rows = [{"scenario": "voice-fact", "voice": {"stt_final": n, "model_first_text": n * 2,
                 "tts_first_audio": None, "keyup_to_answer_audio": 100 + n}} for n in (30, 0, 10)]
        rows.append({"scenario": "voice-count", "voice": {"stt_final": 90}})
        result = bench.aggregate(rows)
        self.assertEqual(result["voice-fact"]["stt"], {"n": 3, "min": 0, "median": 10, "max": 30})
        self.assertEqual(result["voice-fact"]["model"]["median"], 20)
        self.assertEqual(result["voice-fact"]["tts"], {"n": 0, "min": None, "median": None, "max": None})
        self.assertEqual(result["voice-fact"]["answer"]["median"], 110)
        self.assertEqual(result["voice-count"]["stt"]["median"], 90)
        self.assertEqual(bench.aggregate(rows[:2])["voice-fact"]["stt"]["median"], 15)
        rows.append({"scenario": "voice-fact", "voice": {"stt_final": True}})
        self.assertEqual(bench.aggregate(rows)["voice-fact"]["stt"]["n"], 3)

    def test_early_starts_sum_matches_and_lead(self):
        rows = [{"scenario": "voice-fact", "voice": {"transcripts": [
            {"early_starts": 2, "early_discards": 1, "early_released": 1, "early_start_to_final_ms": 300, "early_text_matched": "exact"},
            {"early_starts": 1, "early_discards": 1, "early_released": 0, "early_start_to_final_ms": None, "early_text_matched": "folded"}]}},
            {"scenario": "voice-count", "voice": {"transcripts": [
                {"early_starts": 1, "early_released": 1, "early_start_to_final_ms": 500, "early_text_matched": "exact"},
                {"early_starts": True, "early_text_matched": "other", "early_start_to_final_ms": -1}]}},
            {"scenario": "voice-stop", "voice": {}}]
        self.assertEqual(bench.early(rows), {"starts": 4, "discards": 2, "released": 2, "exact": 2,
                                             "folded": 1, "differs": 0, "lead_median_ms": 400})
        self.assertIsNone(bench.early([])["lead_median_ms"])

    def test_times_parser_and_invalid_repeat_counts(self):
        with patch.object(bench, "run", return_value=0) as run:
            self.assertEqual(bench.main(["voice-bench", "run", "--times", "3", "voice-fact", "voice-count"]), 0)
            run.assert_called_once_with(["voice-fact", "voice-count"], times=3)
        for times in (0, -1, True):
            with self.assertRaisesRegex(bench.BenchError, "positive integer"):
                bench.run(["voice-fact"], times=times)

    def test_repeat_shares_one_gateway_and_retains_every_iteration(self):
        from unittest.mock import MagicMock
        gateway = MagicMock()
        gateway.summary = []
        row = {"scenario": "voice-fact", "probe_unchanged": True, "exit": 0,
               "voice": {"keyup_to_answer_audio": 5000, "stt_final": 100}}
        previous_umask = os.umask(0o077)
        try:
            with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()), patch.dict(os.environ, {}, clear=True):
                with patch.object(bench, "OUT", directory), patch.object(bench, "Gateway", return_value=gateway) as create, patch.object(bench.assist, "prepare_probe", return_value=("probe", "hash", [])), patch.object(bench, "run_one", side_effect=lambda probe, probe_hash, name, ready: dict(row, scenario=name)) as run_one:
                    bench.run(["voice-fact", "voice-count"], times=3)
                    create.assert_called_once()
                    gateway.start.assert_called_once()
                    gateway.close.assert_called_once()
                    self.assertEqual(run_one.call_count, 6)
                    with open(os.path.join(directory, os.listdir(directory)[0])) as file:
                        result = json.load(file)
                    self.assertEqual([r["iteration"] for r in result["rows"]], [1, 2, 3, 1, 2, 3])
                    self.assertEqual(result["rows"][0]["answer_ms"], 5000)
                    self.assertNotIn("answer_ms", result["rows"][0]["checks"])
                    self.assertEqual(result["aggregates"]["voice-fact"]["answer"]["max"], 5000)
                    self.assertEqual(result["aggregates"]["voice-count"]["answer"]["n"], 3)
        finally:
            os.umask(previous_umask)

    def test_unknown_effort_is_refused_before_setup(self):
        with patch.dict(os.environ, {"ZEPHIUM_PROBE_PAGE_EFFORT": "minimal"}), patch.object(bench, "Gateway") as gateway:
            with self.assertRaisesRegex(bench.BenchError, "page_effort_override"):
                bench.run(["voice-fact"])
            gateway.assert_not_called()

    def test_count_spoken_checks_sent_text_not_audio_match(self):
        row = {"scenario": "voice-count", "count_in_reply_text": True, "count_in_spoken_text": False,
               "voice": {"counts": {"fact_audio_enqueued": 3}}}
        self.assertTrue(bench.checks(row)["states_count"])
        self.assertFalse(bench.checks(row)["count_spoken"])
        row["count_in_spoken_text"] = True
        self.assertTrue(bench.checks(row)["count_spoken"])

    def test_answer_barge_ask_requires_second_answer_without_cancel_bar(self):
        row = {"scenario": "voice-barge-ask", "voice": {"counts": {"barge_while_playing": 1},
               "barge_key_down_to_silence": 10, "old_reply_samples_after_flush": 0,
               "transcripts": [{"expected_words": True, "word_count": 5}, {"expected_words": True, "word_count": 7}],
               "second_answer_spoken": True}}
        checks = bench.checks(row)
        for key in ("barged_during_answer", "barge_under_200ms", "no_old_reply_after_flush", "second_utterance_transcribed", "second_answer_spoken"):
            self.assertTrue(checks[key])
        self.assertNotIn("gateway_cancel_acknowledged", checks)
        row["voice"]["counts"]["barge_while_playing"] = 0
        self.assertFalse(bench.checks(row)["barged_during_answer"])

    def test_build_refuses_low_disk_before_cargo(self):
        with patch.object(bench.shutil, "disk_usage", return_value=type("Disk", (), {"free": 14_999_999_999})()), patch.object(bench.subprocess, "run") as run:
            with self.assertRaisesRegex(bench.BenchError, "15 GB required"):
                bench.build()
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
