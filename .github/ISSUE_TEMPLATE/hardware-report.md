---
name: Hardware report
about: Tell us whether rokast works (or not) on your TV, distro, desktop, GPU or Wi-Fi chip
title: "Hardware report: <distro> / <desktop> / <TV model>"
labels: hardware-report
---

**Result:** works / partly works / does not work

**TV model** (Settings > System > About):

**What you tried** (`rokast start`, the desktop app, `rokast selftest`, ENCODER=...):

**What happened** (picture on the TV? audio? lag? error message?):

**Report** (paste the output of `rokast doctor --report`; it includes a 5-second capture self-test and no personal data
beyond your computer's name, distro and hardware model names):

```
paste here
```

If mirroring failed after joining, also attach `sudo cat /run/rokast/supplicant.log` from while it was running
(it can contain your TV's Wi-Fi Direct name and MAC address).
