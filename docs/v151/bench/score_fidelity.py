"""Scores `BENCH {...}` lines of `reformulation_bench` against a corpus.
python score_fidelity.py <corpus.json[;more.json]> <bench output.txt> [...]
Metrics kept apart: LLM output correct (checks on the model text, gate or not),
gate refused, gate false positive (refused a correct text), gate false negative
(accepted a wrong one), final text correct (LLM if accepted, raw otherwise),
rewrite actually applied (accepted and different from the raw), latency.
Checks are the ones of docs/v15/bench/reformulation/bench.py.
"""
import sys, json, re

LIST_NUM = re.compile(r"(?m)^\s*\d+[.)]\s+\S")
LIST_BUL = re.compile(r"(?m)^\s*[-•*]\s+\S")


def shape(out):
    if len(LIST_NUM.findall(out)) >= 2: return "numbered"
    if len(LIST_BUL.findall(out)) >= 2: return "bullets"
    return "none"


def order_ok(out, keys):
    lo = out.lower(); pos = -1
    for k in keys:
        i = lo.find(k.lower(), pos + 1)
        if i < 0: return False
        pos = i
    return True


def present(m, lo):
    m = m.lower()
    return bool(re.search(r"(?<!\w)" + re.escape(m) + r"(?!\w)", lo)) if re.fullmatch(r"\w.*\w|\w", m) else m in lo


def checks(c, out):
    lo = out.lower().replace("’", "'"); fails = []
    for m in c.get("must", []):
        if not any(alt.lower() in lo for alt in m.split("|")): fails.append("MISSING:" + m)
    for m in c.get("must_not", []):
        if present(m, lo): fails.append("PRESENT:" + m)
    for m in c.get("once", []):
        if max(lo.count(a.lower()) for a in m.split("|")) > 1: fails.append("REPEATED:" + m)
    if "format" in c and shape(out) != c["format"]: fails.append(f"FORMAT:{shape(out)}!={c['format']}")
    if "order" in c and not order_ok(out, c["order"]): fails.append("ORDER")
    r = len(out.split()) / max(1, len(c["input"].split()))
    if "max_ratio" in c and r > c["max_ratio"]: fails.append(f"RATIO:{r:.2f}>{c['max_ratio']}")
    return fails


def norm(t): return re.sub(r"\W+", " ", t.lower()).strip()


def score(corpus, path):
    cases = {c["id"]: c for f in corpus.split(";") for c in json.load(open(f, encoding="utf-8"))}
    rows = [json.loads(l.split("BENCH ", 1)[1]) for l in open(path, encoding="utf-8", errors="replace") if l.startswith("BENCH ")]
    tot = {k: 0 for k in ("n", "llm_ok", "refused", "fp", "fn", "final_ok", "rewritten")}; lat = []
    print(f"\n## {path}\n\n| cas | LLM correct | garde-fou | final correct | latence s | échecs LLM |\n|---|---|---|---|---:|---|")
    for r in rows:
        c = cases.get(r["id"])
        if not c: continue
        llm_f = checks(c, r["llm"]) if r["llm"] else ["NO_OUTPUT"]
        final = r["llm"] if r["accepted"] else r["input"]
        fin_f = checks(c, final)
        tot["n"] += 1; tot["llm_ok"] += not llm_f; tot["refused"] += not r["accepted"]
        tot["fp"] += (not r["accepted"]) and not llm_f; tot["fn"] += r["accepted"] and bool(llm_f)
        tot["final_ok"] += not fin_f; tot["rewritten"] += r["accepted"] and norm(r["llm"]) != norm(r["input"])
        lat.append(r["latency"])
        gate = "accepté" if r["accepted"] else f"refusé ({r['reason']})"
        print(f"| {r['id']} | {'oui' if not llm_f else 'non'} | {gate} | {'oui' if not fin_f else 'non'} | {r['latency']:.1f} | {', '.join(llm_f)} |")
    n = tot["n"] or 1
    print(f"\nLLM correct {tot['llm_ok']}/{n} · refusés {tot['refused']} (faux positifs {tot['fp']}) · faux négatifs {tot['fn']} · "
          f"final correct {tot['final_ok']}/{n} · reformulés réellement {tot['rewritten']}/{n} · "
          f"latence moy {sum(lat)/len(lat):.1f} s, max {max(lat):.1f} s")
    return tot


if __name__ == "__main__":
    for p in sys.argv[2:]: score(sys.argv[1], p)
