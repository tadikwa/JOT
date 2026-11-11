import json, glob, os, sys, statistics as st, jiwer
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import norm
rows = []
for f in sorted(glob.glob(os.path.join(os.path.dirname(os.path.abspath(__file__)), "results", "asr-*.jsonl"))):
    recs = [json.loads(l) for l in open(f, encoding="utf-8")]
    sm = json.load(open(f.replace(".jsonl", ".summary.json"))) if os.path.exists(f.replace(".jsonl", ".summary.json")) else {}
    for s in ("tts", "noisy", "fleurs"):
        r = [x for x in recs if x["set"] == s]
        if not r: continue
        wer = jiwer.wer([norm(x["ref"]) for x in r], [norm(x["hyp"]) or "_" for x in r])
        terms = sum(x["terms"] for x in r); hits = sum(x["term_hits"] for x in r)
        lat = [x["latency"] for x in r]; dur = sum(x["dur"] for x in r)
        rows.append((r[0]["engine"], s, len(r), wer, f"{hits}/{terms}" if terms else "-", st.mean(lat), max(lat), sum(lat) / dur, sm))
print(f"{'engine':62} {'set':6} {'n':>3} {'WER':>6} {'termes':>7} {'lat moy':>7} {'lat max':>7} {'RTF':>6}")
for e, s, n, w, t, lm, lx, rtf, sm in rows:
    print(f"{e:62} {s:6} {n:3} {w*100:5.1f}% {t:>7} {lm:7.2f} {lx:7.2f} {rtf:6.3f}  {sm if s=='tts' else ''}")
