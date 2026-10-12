"""Wi-Fi Direct link to a Roku, with a LAN-verified allowlist.

Safety: a P2P peer is only connectable if a Roku on *our own LAN* (ECP :8060) reports the
same name AND a wifi MAC matching the peer's P2P address (first octet | 0x02). Neighbours'
TVs that are merely in radio range can never be selected.

Link (see playbooks/mirror-laptop-to-roku.md for why each step exists):
  1. NetworkManager's wpa_supplicant (p2p-dev-<wifi>) discovers the TV and sends a PBC provision
     discovery, which opens a WPS push-button window on the TV's always-on group.
  2. A second, patched wpa_supplicant (scripts/build-supplicant) on our own P2P-client iface joins
     that group by WPS, sending a phone-like P2P IE + WFD IE in the assoc request. The Roku only
     opens RTSP to sources that join this way; NM's own `p2p_connect ... join` path never works.
  3. dhcpcd on the group iface; the TV then connects to our :7236 within ~0.5 s.
"""
import os
import re
import subprocess
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor

import paths

CTRL = "/run/wpa_supplicant"          # NetworkManager's wpa_supplicant
OWN = "/run/rokast"              # our patched instance (root-owned)
DEVICE_TYPE = "10-0050F204-5"         # WPS primary device type: smartphone, what the Roku accepts as a source
WFD_SOURCE_IE = "000601101c440032"    # WFD dev-info: source, session available, RTSP 7236, 50 Mbps (as AOSP phones send)
# Vendor IEs a Samsung phone sends in its assoc request; kept verbatim (the TV accepts the join with them).
EXTRA_IES = "dd0b0000f022000104000000eddd0b506f9a23000104010f010f"


def sh(*argv, check=True, timeout=30):
    argv = list(argv)
    if argv[0] == "sudo" and os.environ.get("SUDO_ASKPASS"):   # GUI: no terminal to type a password into
        argv.insert(1, "-A")
    r = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(argv)} -> {r.returncode}: {r.stderr.strip() or r.stdout.strip()}")
    return r.stdout.strip()


def p2p_dev_iface():
    for name in sh("sudo", "ls", CTRL).split():
        if name.startswith("p2p-dev-"):
            return name
    raise RuntimeError("no p2p-dev-* interface (is Wi-Fi P2P enabled in NetworkManager/wpa_supplicant?)")


def wpa(iface, *args, check=True):
    return sh("sudo", "wpa_cli", "-p", CTRL, "-i", iface, *args, check=check)


def own(iface, *args, check=True):
    return sh("sudo", "wpa_cli", "-p", f"{OWN}/ctrl", "-i", iface, *args, check=check)


def p2p_mac_of(wifi_mac):
    b = bytearray(int(x, 16) for x in wifi_mac.split(":"))
    b[0] |= 0x02
    return ":".join(f"{x:02x}" for x in b)


def _ecp(ip):
    try:
        t = urllib.request.urlopen(f"http://{ip}:8060/query/device-info", timeout=1.5).read().decode()
    except OSError:
        return None
    g = lambda k: (re.search(f"<{k}>(.*?)</{k}>", t) or [None, ""])[1]  # noqa: E731
    mac = g("wifi-mac").lower()
    if not mac:
        return None
    return {"ip": ip, "name": g("user-device-name") or g("friendly-device-name"),
            "model": g("model-name"), "wifi_mac": mac, "p2p_mac": p2p_mac_of(mac)}


def wifi_iface():
    return p2p_dev_iface().removeprefix("p2p-dev-")


def group_iface():
    return f"p2p-{wifi_iface()}-mir"[:15]


def lan_rokus(prefix=None):
    """Scan the Wi-Fi interface's /24 for Rokus answering ECP.

    Uses the Wi-Fi iface address, NOT the default route: a VPN may own the default route.
    """
    if not prefix:
        m = re.search(r"inet (\d+\.\d+\.\d+)\.\d+/", sh("ip", "-4", "-o", "addr", "show", "dev", wifi_iface()))
        if not m:
            raise RuntimeError(f"no IPv4 address on {wifi_iface()}")
        prefix = m.group(1) + "."
    with ThreadPoolExecutor(64) as ex:
        found = [r for r in ex.map(_ecp, (f"{prefix}{i}" for i in range(1, 255))) if r]
    return sorted(found, key=lambda r: r["name"])


def pick(rokus, name):
    hits = [r for r in rokus if r["name"].lower() == name.lower()]
    if len(hits) != 1:
        raise RuntimeError(f"target {name!r} matched {len(hits)} LAN Rokus; have: {[r['name'] for r in rokus]}")
    return hits[0]


def parse_peer(text):
    return dict(ln.split("=", 1) for ln in text.splitlines() if "=" in ln)


def find_peer(dev, target, timeout=30):
    """Wait until the allowlisted target is visible as a P2P peer with matching name; return its peer info."""
    wpa(dev, "p2p_find")
    t0 = time.time()
    try:
        while time.time() - t0 < timeout:
            if target["p2p_mac"] in wpa(dev, "p2p_peers", check=False).lower().split():  # empty list -> rc 255
                info = parse_peer(wpa(dev, "p2p_peer", target["p2p_mac"]))
                if (info.get("device_name", "").strip().lower() == target["name"].lower()
                        and info.get("interface_addr", "00:00:00:00:00:00") != "00:00:00:00:00:00"):
                    return info
            time.sleep(1)
    finally:
        wpa(dev, "p2p_stop_find", check=False)
    raise RuntimeError(f"{target['name']} not visible over Wi-Fi Direct in {timeout}s")


def _set_wfd(dev, name):
    """Advertise as a WFD source on NM's p2p-dev (carried in our provision discovery request)."""
    wpa(dev, "set", "device_name", name)
    wpa(dev, "set", "wifi_display", "1")
    wpa(dev, "wfd_subelem_set", "0", WFD_SOURCE_IE)
    wpa(dev, "set", "device_type", DEVICE_TYPE)


def assoc_ies(dev_addr, name):
    """Hex of the vendor IEs for our assoc request: P2P IE (capability + device info), WFD IE, extras."""
    nm = name.encode()[:32]
    info = (bytes.fromhex(dev_addr.replace(":", "")) + bytes.fromhex("0188")         # config methods
            + bytes.fromhex("000a0050f2040005") + b"\x00"                              # 10-0050F204-5, no secondary
            + bytes.fromhex("1011") + len(nm).to_bytes(2, "big") + nm)                 # WPS device name attr
    p2p = bytes.fromhex("506f9a09") + bytes.fromhex("0202002700") + b"\x0d" + len(info).to_bytes(2, "little") + info
    wfd = bytes.fromhex("506f9a0a00") + bytes.fromhex(WFD_SOURCE_IE)                   # subelement 0 (dev info); value is len+data
    return "".join(f"dd{len(e):02x}{e.hex()}" for e in (p2p, wfd)) + EXTRA_IES


def iface_mac(dev_addr):
    """Stable locally-administered address for our group iface, derived from the P2P device address."""
    b = bytearray(int(x, 16) for x in dev_addr.split(":"))
    b[0] |= 0x02
    b[4] ^= 0x80
    return ":".join(f"{x:02x}" for x in b)


def _wait_state(iface, want, timeout):
    t0 = time.time()
    while time.time() - t0 < timeout:
        st = parse_peer(own(iface, "status", check=False))
        if st.get("wpa_state") == want:
            return True
        time.sleep(0.5)
    return False


def _start_own(iface, mac):
    supplicant = paths.find_supplicant()
    if not supplicant:
        raise RuntimeError("patched wpa_supplicant missing: run scripts/build-supplicant")
    wifi = wifi_iface()
    sh("sudo", "iw", "dev", wifi, "interface", "add", iface, "type", "__p2pcl")
    sh("sudo", "iw", "dev", iface, "set", "type", "__p2pcl")   # `interface add` alone leaves it managed
    sh("sudo", "ip", "link", "set", iface, "address", mac)
    sh("nmcli", "device", "set", iface, "managed", "no", check=False)
    sh("sudo", "ip", "link", "set", iface, "up")
    sh("sudo", "rm", "-rf", OWN)
    sh("sudo", "mkdir", "-p", "-m", "0700", OWN)
    conf = f"{OWN}/wpa.conf"
    subprocess.run(["sudo", "tee", conf], input=f"ctrl_interface={OWN}/ctrl\nupdate_config=0\np2p_disabled=1\n",
                   text=True, capture_output=True, check=True)
    # WPA_FORCE_P2P_GROUP: patched behaviour, keep the iface in P2P-client mode at association
    sh("sudo", "env", "WPA_FORCE_P2P_GROUP=1", supplicant, "-B", "-K", "-d", "-D", "nl80211", "-i", iface,
       "-c", conf, "-f", f"{OWN}/supplicant.log", "-P", f"{OWN}/pid")
    t0 = time.time()
    while "wpa_state" not in own(iface, "status", check=False):
        if time.time() - t0 > 5:
            raise RuntimeError(f"patched wpa_supplicant did not start (log: sudo cat {OWN}/supplicant.log)")
        time.sleep(0.2)


def _stop_own(iface):
    own(iface, "terminate", check=False)
    pid = sh("sudo", "cat", f"{OWN}/pid", check=False)
    if pid.isdigit():
        sh("sudo", "kill", pid, check=False)
    sh("sudo", "dhcpcd", "-x", iface, check=False, timeout=10)


def connect(dev, target, name="Laptop", tries=4, timeout=15):
    """Join the TV's Wi-Fi Direct group with the patched supplicant. Returns the group iface (no IP yet)."""
    iface = group_iface()
    for attempt in range(1, tries + 1):
        disconnect(dev, iface)
        _set_wfd(dev, name)
        peer = find_peer(dev, target)
        dev_addr = parse_peer(wpa(dev, "status"))["p2p_device_address"]
        _start_own(iface, iface_mac(dev_addr))
        own(iface, "vendor_elem_add", "13", assoc_ies(dev_addr, name))   # 13 = P2P assoc request
        wpa(dev, "p2p_prov_disc", target["p2p_mac"], "pbc", "join", check=False)   # opens the TV's PBC window
        own(iface, "wps_pbc", peer["interface_addr"])
        if _wait_state(iface, "COMPLETED", timeout):
            return iface
        print(f"p2p: join attempt {attempt}/{tries} failed, retrying", flush=True)
    disconnect(dev, iface)
    raise RuntimeError("could not join the TV's Wi-Fi Direct group (Settings>System>Screen mirroring = Prompt/Always allow?)")


def dhcp(iface, timeout=15):
    """Get a lease from the TV (GO). No default route, no DNS changes. Returns our IP."""
    sh("sudo", "ip", "addr", "flush", "dev", iface)
    sh("sudo", "dhcpcd", "-4", "-1", "-t", str(timeout), "--nogateway", "--noipv4ll", "--nohook", "resolv.conf",
       iface, timeout=timeout + 10)
    m = re.search(r"inet (\d+\.\d+\.\d+\.\d+)/", sh("ip", "-4", "-o", "addr", "show", "dev", iface))
    if not m:
        raise RuntimeError(f"no DHCP lease on {iface}")
    return m.group(1)


def disconnect(dev, iface=None):
    """Stop our supplicant and remove every P2P group netdev. The chip allows ONE P2P-client/GO iface."""
    _stop_own(iface or group_iface())
    prefix = dev.removeprefix("p2p-dev-")
    for line in sh("iw", "dev", check=False).splitlines():
        line = line.strip()
        if line.startswith("Interface p2p-" + prefix + "-"):
            name = line.split()[1]
            wpa(dev, "p2p_group_remove", name, check=False)
            sh("sudo", "dhcpcd", "-x", name, check=False, timeout=10)
            sh("sudo", "iw", "dev", name, "del", check=False)
