"""Score `BENCH {json}` lines from the Rust reformulation_bench with the same checks as bench.py."""
import sys, json, os
B = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..") + os.sep
cases = {}
for f in ["reformulation_cases.json", "reformulation_holdout.json", "reformulation_holdout2.json"]:
    for c in json.load(open(B + f, encoding="utf-8")): cases[c["id"]] = (f, c)
def checks(c, out):
    lo = out.lower(); fails = []
    for m in c.get("must", []):
        if not any(a.lower() in lo for a in m.split("|")): fails.append("MISSING:" + m)
    for m in c.get("must_not", []):
        if m.lower() in lo: fails.append("PRESENT:" + m)
    for m in c.get("once", []):
        if lo.count(m.lower()) > 1: fails.append("REPEATED:" + m)
    if out.strip() in c.get("must_not_exact", []): fails.append("ANSWERED")
    r = len(out.split()) / max(1, len(c["input"].split()))
    if "max_ratio" in c and r > c["max_ratio"]: fails.append(f"RATIO:{r:.2f}>{c['max_ratio']}")
    return fails
path = sys.argv[1]; recs = []
for line in open(path, encoding="utf-8", errors="replace"):
    if line.startswith("BENCH "): recs.append(json.loads(line[6:]))
tot = {}; lat = []
out = open(path.replace(".txt", ".scored.jsonl"), "w", encoding="utf-8")
for r in recs:
    f, c = cases[r["id"]]; final = r["llm"] if r["accepted"] else r["input"]
    fails = checks(c, final) + ([] if r["accepted"] else ["REJECTED:" + str(r["reason"])])
    ok, n = tot.get(f, (0, 0)); tot[f] = (ok + (not fails), n + 1); lat.append(r["latency"])
    r.update(final=final, fails=fails); out.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(("OK  " if not fails else "BAD ") + f"{r['id']:24s} {r['latency']:.2f}s {';'.join(fails)}\n     -> {final}")
lat.sort()
print(recs[0]["model"], {k: f"{a}/{b}" for k, (a, b) in tot.items()}, f"median {lat[len(lat)//2]:.2f}s max {lat[-1]:.2f}s")
