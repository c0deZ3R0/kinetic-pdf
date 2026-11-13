# Developer-only bridge transport checks; no CNG keys or pyHanko needed.
#
# PATH: experiments/signature-key-broker/tests/test_bridge.py
# CONSUMED BY: Python unittest; validates bounded/correlated public responses and cleanup.

import importlib.util
import io
import json
from pathlib import Path
import queue
import subprocess
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "bridge_pyhanko.py"
spec = importlib.util.spec_from_file_location("native_bridge", SCRIPT)
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


class BridgeTests(unittest.TestCase):
    def frame(self, **changes):
        response = {"version": 1, "operation_id": "op-1", "body": {"kind": "public_key",
            "algorithm": bridge.ALGORITHM, "key_id": "a" * 64, "test_only": True, "spki_der_b64": "AA=="}}
        response.update(changes)
        return json.dumps(response).encode() + b"\n"

    def test_response_shape_and_correlation(self):
        self.assertEqual(bridge.response_body(self.frame(), "op-1", "public_key")["kind"], "public_key")
        for changes in ({"version": 2}, {"version": True}, {"operation_id": "wrong"}, {"extra": "forbidden"}):
            with self.subTest(changes=changes), self.assertRaises(RuntimeError):
                bridge.response_body(self.frame(**changes), "op-1", "public_key")
        body = json.loads(self.frame())["body"]
        body["private_material"] = "forbidden"
        with self.assertRaises(RuntimeError):
            bridge.response_body(self.frame(body=body), "op-1", "public_key")

    def test_duplicate_and_oversized_or_unterminated_frames_rejected(self):
        for frame in (self.frame().replace(b'"version": 1', b'"version":1,"version":1'),
                      b"x" * bridge.MAX_REPLY + b"\n", self.frame().rstrip(b"\n"), b"not-json\n"):
            with self.subTest(length=len(frame)), self.assertRaises(RuntimeError):
                bridge.response_body(frame, "op-1", "public_key")

    def test_empty_channel_has_bounded_timeout(self):
        with self.assertRaisesRegex(RuntimeError, "IPC timeout"):
            bridge.bounded_wait(queue.Queue(maxsize=1), timeout=0.001)

    def test_close_kills_only_its_own_timed_out_child(self):
        class Child:
            stdin, stdout = io.BytesIO(), io.BytesIO()
            killed = 0
            def poll(self):
                return -9 if self.killed else None
            def wait(self, timeout):
                if not self.killed:
                    raise subprocess.TimeoutExpired("synthetic own child", timeout)
                return -9
            def kill(self):
                self.killed += 1
        class Reader:
            def join(self, timeout):
                pass
        owner = object.__new__(bridge.Broker)
        owner.process, owner.reader = Child(), Reader()
        owner.close()
        self.assertEqual(owner.process.killed, 1)
        self.assertEqual(owner.exit_code, -9)
        self.assertTrue(owner.process.stdin.closed and owner.process.stdout.closed)


if __name__ == "__main__":
    unittest.main()
