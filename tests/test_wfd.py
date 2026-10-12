"""WFD source vs a fake Roku sink: full M1..M7 handshake, no radio, no TV."""
import asyncio
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src", "miracast"))
import capture  # noqa: E402
import cli  # noqa: E402
import p2p  # noqa: E402
import wayland  # noqa: E402
import wfd  # noqa: E402

CAPS = ("wfd_video_formats: 00 00 03 10 0001ffff 1fffffff 00000fff 00 0000 0000 11 none none\r\n"
        "wfd_audio_codecs: LPCM 00000003 00, AAC 0000000f 00\r\n"
        "wfd_client_rtp_ports: RTP/AVP/UDP;unicast 19000 0 mode=play\r\n")


async def fake_sink(port, result):
    """Behaves like a Roku: connects to the source, answers M1, sends M2, answers M3/M4/M5, then M6 SETUP, M7 PLAY."""
    reader, writer = await asyncio.open_connection("127.0.0.1", port)

    async def send(text):
        writer.write(text.encode())
        await writer.drain()

    m1 = await wfd.read_msg(reader)                      # source opens with M1
    assert m1.method == "OPTIONS" and "org.wfa.wfd1.0" in m1.headers["require"]
    await send(f"RTSP/1.0 200 OK\r\nCSeq: {m1.cseq}\r\nPublic: org.wfa.wfd1.0, SETUP, PLAY, TEARDOWN, GET_PARAMETER, SET_PARAMETER\r\n\r\n")
    await send("OPTIONS * RTSP/1.0\r\nCSeq: 1\r\nRequire: org.wfa.wfd1.0\r\n\r\n")   # M2
    r = await wfd.read_msg(reader)
    assert r.start.startswith("RTSP/1.0 200") and "org.wfa.wfd1.0" in r.headers["public"]
    while True:
        m = await wfd.read_msg(reader)
        if m.method == "GET_PARAMETER":
            await send(f"RTSP/1.0 200 OK\r\nCSeq: {m.cseq}\r\nContent-Type: text/parameters\r\nContent-Length: {len(CAPS)}\r\n\r\n{CAPS}")
        elif m.method == "SET_PARAMETER":
            result.setdefault("set", []).append(m.body)
            await send(f"RTSP/1.0 200 OK\r\nCSeq: {m.cseq}\r\n\r\n")
            if "wfd_trigger_method" in m.body:
                break
    await send("SETUP rtsp://x/wfd1.0/streamid=0 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/UDP;unicast;client_port=19000\r\n\r\n")
    setup = await wfd.read_msg(reader)
    result["setup_resp"] = setup
    await send(f"PLAY rtsp://x/wfd1.0/streamid=0 RTSP/1.0\r\nCSeq: 3\r\nSession: {setup.headers['session'].split(';')[0]}\r\n\r\n")
    result["play_resp"] = await wfd.read_msg(reader)
    await asyncio.sleep(0.1)
    writer.close()


async def _delayed_sink(port, result):
    await asyncio.sleep(0.2)
    await fake_sink(port, result)


class Handshake(unittest.TestCase):
    def test_full_handshake_and_play(self):
        result, played = {}, {}

        async def go():
            port = 17236
            src = wfd.Source("127.0.0.1", ["1080p30"], log=lambda *_: None)

            async def on_play(n, ip, sp):
                played.update(n=n, ip=ip, sp=sp)
            src.on_play = on_play
            await asyncio.wait_for(asyncio.gather(src.serve(port), _delayed_sink(port, result)), 10)

        asyncio.run(go())
        n = played["n"]
        self.assertEqual((n.width, n.height, n.fps), (1920, 1080, 30))
        self.assertEqual(n.profile, "baseline")          # sink offers CBP -> most compatible
        self.assertEqual(n.audio, "AAC")
        self.assertEqual(n.rtp_port, 19000)
        m4 = result["set"][0]
        self.assertIn("wfd_presentation_URL: rtsp://127.0.0.1/wfd1.0/streamid=0 none", m4)
        self.assertIn("wfd_video_formats: 00 00 01 04 00000080", m4)   # CBP, L4.0, only bit7 (1080p30)
        self.assertIn("server_port=", result["setup_resp"].headers["transport"])
        self.assertTrue(result["play_resp"].start.startswith("RTSP/1.0 200"))


class Choose(unittest.TestCase):
    def test_preference_and_fallback(self):
        caps = wfd.parse_video_formats("00 00 03 10 0001ffff 1fffffff 00000fff 00 0000 0000 11 none none")
        self.assertEqual(wfd.choose_video(caps, ["720p60"])[1], (1280, 720, 60))
        only480 = dict(caps, cea=0x1)
        self.assertEqual(wfd.choose_video(only480, ["1080p30"])[1], (640, 480, 60))

    def test_roku_sink_caps_accepts_unknown_levels(self):
        caps = wfd.parse_video_formats("00 00 01 02 00000080 00000000 00000000 00 0000 0000 00 none none")
        bit, res, prof, level = wfd.choose_video(caps)
        self.assertEqual((res, prof), ((1920, 1080, 30), 1))
        self.assertEqual(level, 0x02)    # sink only does L3.2 -> never exceed what it said


class P2P(unittest.TestCase):
    def test_p2p_mac_matches_observed_roku(self):
        self.assertEqual(p2p.p2p_mac_of("4c:50:dd:21:8b:c4"), "4e:50:dd:21:8b:c4")
        self.assertEqual(p2p.p2p_mac_of("a8:16:9d:d4:b8:c5"), "aa:16:9d:d4:b8:c5")

    def test_pick_refuses_unknown_and_ambiguous(self):
        rokus = [{"name": "A"}, {"name": "B"}, {"name": "B"}]
        self.assertEqual(p2p.pick(rokus, "a")["name"], "A")
        with self.assertRaises(RuntimeError):
            p2p.pick(rokus, "JERICAS TV")      # neighbour's TV: not on our LAN
        with self.assertRaises(RuntimeError):
            p2p.pick(rokus, "B")

    def test_assoc_ies_byte_identical_to_working_phone(self):
        # Same layout as the assoc-request vendor IEs of an Android phone the Roku accepts (monitor capture),
        # with the phone's address and name replaced.
        phone = ("dd26" "506f9a09"                  # P2P IE
                 "0202002700"                       # P2P capability: device 0x27, group 0x00
                 "0d1a00" "021122334455"            # device info (len 26), P2P device address
                 "0188" "000a0050f2040005" "00"     # config methods, type 10-0050F204-5, no secondary types
                 "10110005" "50686f6e65"            # WPS device name "Phone"
                 "dd0d" "506f9a0a" "00000601101c440032"   # WFD IE: dev info = source, RTSP 7236, 50 Mbps
                 "dd0b0000f022000104000000ed" "dd0b506f9a23000104010f010f")   # extra vendor IEs, verbatim
        self.assertEqual(p2p.assoc_ies("02:11:22:33:44:55", "Phone"), phone)

    def test_iface_mac_is_local_and_distinct(self):
        self.assertEqual(p2p.iface_mac("00:11:22:33:c4:55"), "02:11:22:33:44:55")

    def test_parse_peer(self):
        info = p2p.parse_peer("4e:11:22:33:44:55\ndevice_name=Living Room\ninterface_addr=46:11:22:33:44:55\n")
        self.assertEqual((info["device_name"], info["interface_addr"]), ("Living Room", "46:11:22:33:44:55"))


class Capture(unittest.TestCase):
    def test_rtp_output_and_profile(self):
        n = wfd.Negotiated(1920, 1080, 30, "baseline", "4.0", "AAC", 19000)
        cmd = capture.build_cmd(n, "192.168.49.2", 22222, monitor="m.monitor")
        self.assertEqual(cmd[-1], "rtp://192.168.49.2:19000?localport=22222&pkt_size=1316")
        self.assertEqual(cmd[cmd.index("-profile:v") + 1], "baseline")
        self.assertIn("pulse", cmd)
        self.assertEqual(cmd[cmd.index("-g") + 1], "30")

    def test_display_with_screen_suffix(self):
        n = wfd.Negotiated(1920, 1080, 30, "high", "4.0", "AAC", 19000)
        cmd = capture.build_cmd(n, "192.168.49.2", 22222, display=":0.0")   # $DISPLAY is often ":0.0"
        self.assertEqual(cmd[cmd.index("x11grab") + 6], ":0.0+0,0")

    def test_vaapi_uploads_frames_to_gpu(self):
        n = wfd.Negotiated(1920, 1080, 30, "high", "4.0", "AAC", 19000)
        cmd = capture.build_cmd(n, "192.168.49.2", 22222, encoder="h264_vaapi")
        self.assertLess(cmd.index("-vaapi_device"), cmd.index("-i"))
        self.assertTrue(cmd[cmd.index("-vf") + 1].endswith("format=nv12,hwupload"))

    def test_vaapi_profile_and_level_spelling(self):
        # h264_vaapi rejects "baseline" and fails mid-encode with "-level 4.0" (found by rokast selftest on Intel iHD)
        n = wfd.Negotiated(1920, 1080, 30, "baseline", "4.0", "AAC", 19000)
        cmd = capture.build_cmd(n, "192.168.49.2", 22222, encoder="h264_vaapi")
        self.assertEqual(cmd[cmd.index("-profile:v") + 1], "constrained_baseline")
        self.assertEqual(cmd[cmd.index("-level") + 1], "40")
        n.level = "4.2"
        self.assertEqual(capture.build_cmd(n, "x", 1, encoder="h264_vaapi")[cmd.index("-level") + 1], "42")

    def test_file_output_for_selftest(self):
        n = wfd.Negotiated(1280, 720, 30, "high", "4.0", "AAC", 19000)
        cmd = capture.build_cmd(n, None, None, out="/tmp/x.ts", seconds=5)
        self.assertEqual(cmd[cmd.index("-t"):], ["-t", "5", "-y", "-f", "mpegts", "/tmp/x.ts"])
        self.assertNotIn("rtp_mpegts", cmd)

    def test_raw_stdin_input_for_wayland(self):
        n = wfd.Negotiated(1280, 720, 60, "high", "4.0", "AAC", 19000)
        cmd = capture.build_cmd(n, "192.168.49.2", 22222, source="raw")
        self.assertNotIn("x11grab", cmd)
        i = cmd.index("rawvideo")
        self.assertEqual(cmd[i:i + 9], ["rawvideo", "-pix_fmt", "yuv420p", "-video_size", "1280x720",
                                        "-framerate", "60", "-i", "pipe:0"])


class Wayland(unittest.TestCase):
    def test_gst_delivers_exact_negotiated_frames(self):
        cmd = wayland.gst_cmd(7, 42, 1920, 1080, 30)
        self.assertEqual(cmd[:4], ["gst-launch-1.0", "-q", "pipewiresrc", "fd=7"])
        self.assertIn("path=42", cmd)
        self.assertIn("video/x-raw,format=I420,width=1920,height=1080,framerate=30/1,pixel-aspect-ratio=1/1", cmd)
        self.assertEqual(cmd[-3:], ["fdsink", "fd=1", "sync=false"])

    def test_session_detection(self):
        self.assertTrue(wayland.is_wayland({"WAYLAND_DISPLAY": "wayland-0", "DISPLAY": ":0"}))   # XWayland present
        self.assertTrue(wayland.is_wayland({"XDG_SESSION_TYPE": "wayland"}))
        self.assertFalse(wayland.is_wayland({"DISPLAY": ":0", "XDG_SESSION_TYPE": "x11"}))


class Name(unittest.TestCase):
    def test_clean_name_limits_to_32_bytes(self):
        self.assertEqual(cli.clean_name("  Casper's Laptop\n"), "Casper's Laptop")
        self.assertEqual(len(cli.clean_name("é" * 40).encode()), 32)   # never splits a UTF-8 character
        self.assertEqual(cli.clean_name("\x00\x07"), "")


if __name__ == "__main__":
    unittest.main()
