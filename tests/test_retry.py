"""cli.join_and_stream: a failure before the stream started is retried once from scratch (status: retrying)."""
import contextlib
import io
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src", "miracast"))
import cli  # noqa: E402

TARGET = {"name": "Living Room"}


def run(connect_results, stream_fails=False, played=False):
    """connect_results: per call, an iface or an exception. Returns (stdout, error or None, disconnect calls)."""
    results = iter(connect_results)

    def connect(*a):
        r = next(results)
        if isinstance(r, Exception):
            raise r
        return r

    async def stream(conf, iface, cast, seen):
        if played:
            seen.append(True)
        if stream_fails:
            raise RuntimeError("the TV joined but never opened RTSP to us")

    out, disc = io.StringIO(), []
    with mock.patch.object(cli.p2p, "connect", connect), mock.patch.object(cli, "stream", stream), \
            mock.patch.object(cli.p2p, "disconnect", lambda dev: disc.append(dev)), contextlib.redirect_stdout(out):
        try:
            cli.join_and_stream({"NAME": "Laptop"}, "p2p-dev-wlo1", TARGET, None)
            return out.getvalue(), None, disc
        except RuntimeError as e:
            return out.getvalue(), str(e), disc


class Retry(unittest.TestCase):
    def test_first_round_works(self):
        out, err, disc = run(["p2p-wlo1-mir"])
        self.assertIsNone(err)
        self.assertNotIn("status: retrying", out)
        self.assertEqual(disc, [])

    def test_failed_join_is_retried_once(self):
        out, err, disc = run([RuntimeError("could not join"), "p2p-wlo1-mir"])
        self.assertIsNone(err)
        self.assertIn("status: retrying could not join\n", out)
        self.assertEqual(out.count("status: joining Living Room"), 2)
        self.assertEqual(disc, ["p2p-dev-wlo1"])

    def test_gives_up_after_the_second_round(self):
        out, err, disc = run([RuntimeError("one"), RuntimeError("two")])
        self.assertEqual(err, "two")
        self.assertEqual(out.count("status: retrying"), 1)

    def test_no_rtsp_is_retried(self):
        out, err, disc = run(["p2p-wlo1-mir", "p2p-wlo1-mir"], stream_fails=True)
        self.assertEqual(err, "the TV joined but never opened RTSP to us")
        self.assertIn("status: retrying the TV joined", out)

    def test_failure_after_streaming_is_not_retried(self):
        out, err, disc = run(["p2p-wlo1-mir"], stream_fails=True, played=True)
        self.assertEqual(err, "the TV joined but never opened RTSP to us")
        self.assertNotIn("status: retrying", out)


if __name__ == "__main__":
    unittest.main()
