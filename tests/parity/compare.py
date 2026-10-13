"""compare.py cases.json py.out rs.out: every case must give the same result on both sides (see make_cases.py)."""
import json
import sys

cases = json.load(open(sys.argv[1]))
py = [json.loads(ln) for ln in open(sys.argv[2])]
rs = [json.loads(ln) for ln in open(sys.argv[3])]
if not len(cases) == len(py) == len(rs):
    sys.exit(f"parity: result counts differ: {len(cases)} cases, {len(py)} python, {len(rs)} rust")
bad = 0
for i, (c, p, r) in enumerate(zip(cases, py, rs)):
    if "err" in p and "err" in r and not c.get("exact_err"):
        continue
    if p != r:
        bad += 1
        print(f"#{i} {c['fn']} {json.dumps(c['args'], ensure_ascii=False)[:200]}\n  python: {p}\n  rust:   {r}")
print(f"parity: {len(cases) - bad}/{len(cases)} cases match")
sys.exit(1 if bad else 0)
