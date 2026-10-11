"""Wi-Fi Display (Miracast) RTSP *source* side, M1..M7 + keepalive.

The SOURCE (us) listens on TCP 7236 (advertised in our WFD IE); the sink (TV) connects in.
We send M1, answer the sink's M2, query its capabilities (M3), pick a format (M4), trigger
SETUP (M5), answer SETUP/PLAY (M6/M7), then the caller streams RTP/MPEG-TS to the negotiated
port. Pure asyncio, no I/O besides the one socket -> testable against a fake sink.
"""
import asyncio
import random
import re
import socket
from dataclasses import dataclass, field

CRLF = "\r\n"
PUBLIC = "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER, SETUP, PLAY, PAUSE, TEARDOWN"

# CEA resolution bit -> (w, h, fps)   (Wi-Fi Display spec table 5-10)
CEA = {0: (640, 480, 60), 1: (720, 480, 60), 3: (720, 576, 50), 5: (1280, 720, 30),
       6: (1280, 720, 60), 7: (1920, 1080, 30), 8: (1920, 1080, 60), 10: (1280, 720, 25),
       11: (1280, 720, 50), 12: (1920, 1080, 25), 13: (1920, 1080, 50),
       15: (1280, 720, 24), 16: (1920, 1080, 24)}
LEVEL_FOR = {(1920, 1080, 60): 0x10, (1920, 1080, 50): 0x10}  # else 4.0 is enough
DEFAULT_PREF = ["1080p30", "720p60", "720p30", "480p60"]
PREF_BIT = {"1080p60": 8, "1080p30": 7, "720p60": 6, "720p30": 5, "480p60": 0}


@dataclass
class Negotiated:
    width: int
    height: int
    fps: int
    profile: str            # "baseline" | "high"
    level: str              # e.g. "4.0"
    audio: str              # "AAC" | "LPCM"
    rtp_port: int
    video_param: str = ""   # exact wfd_video_formats string we sent in M4


def parse_video_formats(s):
    """Parse the sink's wfd_video_formats value into masks."""
    t = s.split()
    if len(t) < 11 or t[0] == "none":
        raise ValueError(f"unparseable wfd_video_formats: {s!r}")
    return {"profile": int(t[2], 16), "level": int(t[3], 16), "cea": int(t[4], 16),
            "vesa": int(t[5], 16), "hh": int(t[6], 16)}


def choose_video(sink_caps, prefs=None):
    """Pick (bit, (w,h,fps), profile_bit, level_bit) from sink caps. Pure; unit-tested."""
    prefs = prefs or DEFAULT_PREF
    order = [PREF_BIT[p] for p in prefs if p in PREF_BIT] + [8, 7, 6, 5, 1, 0]
    for bit in order:
        if sink_caps["cea"] & (1 << bit):
            res = CEA[bit]
            need = LEVEL_FOR.get(res, 0x04)       # 0x04 = L4.0, 0x10 = L4.2
            lv = sink_caps["level"]
            level = need if lv >= need else (lv & -lv if lv else 0x01)
            # constrained baseline if offered (most compatible), else high
            prof = 0x01 if sink_caps["profile"] & 0x01 else 0x02
            return bit, res, prof, level
    raise ValueError("sink offers no CEA resolution we can use")


LEVEL_NAME = {0x01: "3.1", 0x02: "3.2", 0x04: "4.0", 0x08: "4.1", 0x10: "4.2"}


def format_video_param(bit, prof, level):
    return f"00 00 {prof:02x} {level:02x} {1 << bit:08x} 00000000 00000000 00 0000 0000 00 none none"


@dataclass
class Msg:
    start: str
    headers: dict = field(default_factory=dict)
    body: str = ""

    @property
    def is_request(self):
        return not self.start.startswith("RTSP/")

    @property
    def method(self):
        return self.start.split()[0]

    @property
    def cseq(self):
        return int(self.headers.get("cseq", "0"))


async def read_msg(reader):
    head = await reader.readuntil(b"\r\n\r\n")
    lines = head.decode(errors="replace").split(CRLF)
    h = {}
    for ln in lines[1:]:
        if ":" in ln:
            k, v = ln.split(":", 1)
            h[k.strip().lower()] = v.strip()
    n = int(h.get("content-length", "0"))
    body = (await reader.readexactly(n)).decode(errors="replace") if n else ""
    return Msg(lines[0], h, body)


def parse_params(body):
    out = {}
    for ln in body.splitlines():
        if ":" in ln:
            k, v = ln.split(":", 1)
            out[k.strip()] = v.strip()
    return out


class Source:
    def __init__(self, local_ip, prefs=None, log=print):
        self.local_ip, self.prefs, self.log = local_ip, prefs, log
        self.cseq = 0
        self.pending = {}
        self.session = f"{random.randrange(10**7, 10**8)}"
        self.server_port = random.randrange(20000, 30000)
        self.negotiated = None
        self.sink_ip = None
        self.on_play = None    # async callable(Negotiated, sink_ip, server_port)
        self.on_stop = None
        self.playing = asyncio.Event()
        self.listening = asyncio.Event()

    async def _send(self, w, text):
        w.write(text.encode())
        await w.drain()

    async def _request(self, w, method, uri, headers=None, body=""):
        self.cseq += 1
        h = {"CSeq": str(self.cseq), **(headers or {})}
        if body:
            h["Content-Type"] = "text/parameters"
            h["Content-Length"] = str(len(body.encode()))
        fut = asyncio.get_running_loop().create_future()
        self.pending[self.cseq] = fut
        msg = f"{method} {uri} RTSP/1.0{CRLF}" + "".join(f"{k}: {v}{CRLF}" for k, v in h.items()) + CRLF + body
        await self._send(w, msg)
        resp = await asyncio.wait_for(fut, 10)
        if not resp.start.split()[1] == "200":
            raise RuntimeError(f"{method} -> {resp.start}")
        return resp

    async def _respond(self, w, req, status="200 OK", headers=None, body=""):
        h = {"CSeq": str(req.cseq), **(headers or {})}
        if body:
            h["Content-Type"] = "text/parameters"
            h["Content-Length"] = str(len(body.encode()))
        await self._send(w, f"RTSP/1.0 {status}{CRLF}" + "".join(f"{k}: {v}{CRLF}" for k, v in h.items()) + CRLF + body)

    async def serve(self, port=7236, device=None):
        """Listen on :port, accept the sink's one connection, run the session until it ends.

        device: bind to that netdev (SO_BINDTODEVICE) instead of local_ip, so the listener can be up
        before DHCP finishes (the Roku connects ~0.5 s after its lease and gives up after ~4 s) without
        being reachable from the LAN. `self.listening` is set once accepting."""
        done = asyncio.get_running_loop().create_future()

        async def handler(r, w):
            if done.done():
                w.close()
                return
            try:
                self.local_ip = w.get_extra_info("sockname")[0]
                await self._session(r, w, w.get_extra_info("peername")[0])
            finally:
                if not done.done():
                    done.set_result(None)

        if device:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_BINDTODEVICE, device.encode())
            sock.bind(("0.0.0.0", port))
            server = await asyncio.start_server(handler, sock=sock)
        else:
            server = await asyncio.start_server(handler, self.local_ip, port)
        self.log(f"wfd: listening on {device or self.local_ip}:{port} for the sink")
        self.listening.set()
        try:
            await asyncio.wait_for(asyncio.shield(done), None)
        finally:
            server.close()

    async def _session(self, r, w, sink_ip):
        self.sink_ip = sink_ip
        self.log(f"wfd: sink {sink_ip} connected")
        self.m2_seen = asyncio.Event()
        negotiate = asyncio.create_task(self._negotiate(w))
        try:
            while True:
                msg = await read_msg(r)
                if not msg.is_request:
                    fut = self.pending.pop(msg.cseq, None)
                    if fut and not fut.done():
                        fut.set_result(msg)
                    continue
                m = msg.method
                if m == "OPTIONS":                                   # M2 (sink -> source)
                    await self._respond(w, msg, headers={"Public": PUBLIC})
                    self.m2_seen.set()
                elif m == "SETUP":                                   # M6
                    t = msg.headers.get("transport", "")
                    mt = re.search(r"client_port=(\d+)", t)
                    if mt and self.negotiated:
                        self.negotiated.rtp_port = int(mt.group(1))
                    await self._respond(w, msg, headers={
                        "Session": f"{self.session};timeout=30",
                        "Transport": f"RTP/AVP/UDP;unicast;client_port={self.negotiated.rtp_port};server_port={self.server_port}"})
                elif m == "PLAY":                                    # M7
                    await self._respond(w, msg, headers={"Session": self.session})
                    self.playing.set()
                    if self.on_play:
                        await self.on_play(self.negotiated, sink_ip, self.server_port)
                    asyncio.create_task(self._keepalive(w))
                elif m == "TEARDOWN":
                    await self._respond(w, msg, headers={"Session": self.session})
                    break
                else:                                                # GET/SET_PARAMETER (keepalive, idr request)
                    await self._respond(w, msg)
        except (asyncio.IncompleteReadError, ConnectionError):
            self.log("wfd: sink closed the connection")
        finally:
            negotiate.cancel()
            w.close()
            if self.on_stop:
                await self.on_stop()

    async def _negotiate(self, w):
        await self._request(w, "OPTIONS", "*", {"Require": "org.wfa.wfd1.0"})                  # M1
        await asyncio.wait_for(self.m2_seen.wait(), 10)                                        # M2 from sink
        resp = await self._request(w, "GET_PARAMETER", "rtsp://localhost/wfd1.0", body=         # M3
                                   "wfd_video_formats\r\nwfd_audio_codecs\r\nwfd_client_rtp_ports\r\n")
        caps = parse_params(resp.body)
        self.log(f"wfd: sink caps {caps}")
        bit, (w_, h_, fps), prof, level = choose_video(parse_video_formats(caps["wfd_video_formats"]), self.prefs)
        audio = "AAC" if "AAC" in caps.get("wfd_audio_codecs", "") else "LPCM"
        audio_param = "AAC 00000001 00" if audio == "AAC" else "LPCM 00000002 00"
        mp = re.search(r"(\d+)\s+\d+", caps.get("wfd_client_rtp_ports", ""))
        port = int(mp.group(1)) if mp else 1028
        vp = format_video_param(bit, prof, level)
        self.negotiated = Negotiated(w_, h_, fps, "baseline" if prof == 1 else "high",
                                     LEVEL_NAME.get(level, "4.0"), audio, port, vp)
        body = (f"wfd_video_formats: {vp}\r\nwfd_audio_codecs: {audio_param}\r\n"                 # M4
                f"wfd_presentation_URL: rtsp://{self.local_ip}/wfd1.0/streamid=0 none\r\n"
                f"wfd_client_rtp_ports: RTP/AVP/UDP;unicast {port} 0 mode=play\r\n")
        await self._request(w, "SET_PARAMETER", "rtsp://localhost/wfd1.0", body=body)
        await self._request(w, "SET_PARAMETER", "rtsp://localhost/wfd1.0", body="wfd_trigger_method: SETUP\r\n")  # M5
        self.log(f"wfd: negotiated {w_}x{h_}@{fps} {self.negotiated.profile} L{self.negotiated.level} {audio}")

    async def _keepalive(self, w):
        while not w.is_closing():
            await asyncio.sleep(20)
            try:
                await self._request(w, "GET_PARAMETER", "rtsp://localhost/wfd1.0")
            except Exception as e:  # noqa: BLE001
                self.log(f"wfd: keepalive failed: {e}")
                w.close()
                return
