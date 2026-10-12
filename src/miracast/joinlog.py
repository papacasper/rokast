"""Evidence from Wi-Fi Direct join attempts that failed (why does the first join after the TV sits idle fail?).

The next attempt wipes a failed attempt's supplicant log (p2p._start_own clears /run/rokast), so p2p.connect hands
every failed attempt to a JoinLog straight away. A run with any failed attempt is saved, whether it joined in the
end or not, to $XDG_CACHE_HOME/rokast/join-failures/<UTC time>/ (the newest KEEP are kept):
  summary.json    result, target, TV state over the LAN, and per attempt: timings (s since the join began),
                  the TV's P2P peer entry before and after, both supplicants' status
  attempt-N.log   our patched supplicant's debug log for attempt N (also the attempt that joined, to compare)
`rokast doctor --report` prints the newest summary.
"""
import json
import os
import shutil
import time

import paths

KEEP = 5


def failures_dir(env=os.environ):
    return os.path.join(paths._xdg(env, "XDG_CACHE_HOME", ".cache"), "rokast", "join-failures")


def runs(env=os.environ):
    """Saved runs, oldest first (names are UTC times, so they sort)."""
    base = failures_dir(env)
    try:
        return [os.path.join(base, n) for n in sorted(os.listdir(base)) if os.path.isdir(os.path.join(base, n))]
    except FileNotFoundError:
        return []


def latest(env=os.environ):
    r = runs(env)
    return r[-1] if r else None


class JoinLog:
    def __init__(self, target, clock=time.monotonic):
        self.clock, self.t0 = clock, clock()
        self.target = {k: target.get(k, "") for k in ("name", "ip", "model", "p2p_mac")}
        self.attempts, self.logs, self.tv = [], {}, {}

    def t(self):
        return round(self.clock() - self.t0, 2)

    def failed(self, info, log):
        """Record one failed attempt: info (timings, peer, states) and our supplicant's log text."""
        n = len(self.attempts) + 1
        self.attempts.append(dict(info, attempt=n, failed_at=self.t()))
        self.logs[n] = log

    def joined(self, info, log):
        """Record the attempt that joined; kept only when an earlier one failed."""
        n = len(self.attempts) + 1
        self.attempts.append(dict(info, attempt=n, joined_at=self.t()))
        self.logs[n] = log

    def save(self, result, env=os.environ, now=None, keep=KEEP):
        """Write the run if any attempt failed; returns its folder, else None. Never raises: evidence is best effort."""
        if not any("failed_at" in a for a in self.attempts):
            return None
        try:
            base = failures_dir(env)
            os.makedirs(base, mode=0o700, exist_ok=True)
            stamp = time.strftime("%Y%m%d-%H%M%S", time.gmtime(now))
            d, i = os.path.join(base, stamp), 1
            while os.path.exists(d):
                i += 1
                d = os.path.join(base, f"{stamp}-{i}")
            os.makedirs(d, mode=0o700)
            summary = {"result": result, "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now)),
                       "target": self.target, "tv": self.tv, "attempts": self.attempts}
            with open(os.path.join(d, "summary.json"), "w") as f:
                json.dump(summary, f, indent=1)
            for n, log in self.logs.items():
                with open(os.path.join(d, f"attempt-{n}.log"), "w") as f:
                    f.write(log)
            for old in runs(env)[:-keep]:
                shutil.rmtree(old, ignore_errors=True)
            return d
        except OSError:
            return None
