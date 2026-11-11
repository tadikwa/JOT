import json, glob, os, statistics as st
H = os.path.dirname(os.path.abspath(__file__))
print(f"{'modèle':16} {'fidél.':>6} {'checks':>7} {'ajouts':>6} {'termes corrigés':>15} {'lat moy':>7} {'lat max':>7}  échecs")
for f in sorted(glob.glob(os.path.join(H, "results", "llm-*.jsonl"))):
    r = [json.loads(l) for l in open(f, encoding="utf-8")]
    sm = json.load(open(f.replace(".jsonl", ".summary.json")))
    fixable = sum(len(x.get("fixable", [])) for x in r); fixed = sum(len(x.get("fixed", [])) for x in r)
    added = sum(len(x["added_words"]) for x in r)
    fails = [f"{x['id']}:{','.join(x['fails'])}" for x in r if not x["checks_ok"]]
    print(f"{sm['model']:16} {st.mean(x['fidelity'] for x in r):6.3f} {sum(x['checks_ok'] for x in r):3}/{len(r):<3} {added:6} {fixed:>7}/{fixable:<7} {st.mean(x['latency'] for x in r):7.2f} {max(x['latency'] for x in r):7.2f}  {' '.join(fails)}")
    print(f"{'':16} load {sm['load_s']} s, VRAM +{sm['peak_vram_delta_mib']} Mio, RSS {sm['peak_rss_mib']} Mio, fichier {sm['file_gb']} Go")
