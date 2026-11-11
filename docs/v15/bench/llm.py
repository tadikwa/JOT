"""Reformulation bench: python llm.py <label> <build> <gguf> <ngl> -> results/llm-<label>.jsonl"""
import sys, os, json, time, re, subprocess, requests, jiwer
from common import Sampler, norm
HERE = os.path.dirname(os.path.abspath(__file__)); DL = os.path.join(HERE, "..", "dl")
label, build, gguf, ngl = sys.argv[1:5]
HTTP = requests.Session(); HTTP.trust_env = False
PORT = 18190
VOCAB = "Proxmox, Ansible, Kubernetes, playbook, Claude Code, nginx, NVMe, Ceph, ZFS"
SYSTEM = open(os.path.join(HERE, "prompt.txt"), encoding="utf-8").read().replace("{vocab}", VOCAB)

corpus = {c["id"]: c for c in json.load(open(os.path.join(HERE, "corpus.json"), encoding="utf-8"))}
cases = []
for l in open(os.path.join(HERE, "results", "asr-llama-asr_llama-cuda_17B_99.jsonl"), encoding="utf-8"):
    r = json.loads(l)
    if r["set"] != "tts": continue
    c = corpus[r["id"]]
    must_not = {"s09-injection": ["\n", "vague", "océan"], "s11-question": ["canberra", "sydney"]}.get(r["id"], [])
    cases.append({"id": r["id"], "input": r["hyp"], "expected": c["clean"], "must": [t for t in c["terms"] if t.lower() in r["hyp"].lower()],
                  "fixable": [t for t in c["terms"] if t.lower() not in r["hyp"].lower()], "must_not": must_not})
cases += json.load(open(os.path.join(HERE, "llm_cases.json"), encoding="utf-8"))

def sanitize(t):  # the dictated text can never close our delimiter
    return re.sub(r"</?\s*dictee\s*>", " ", t, flags=re.I).strip()

exe = os.path.join(DL, "x", build, "llama-server.exe")
t0 = time.perf_counter()
p = subprocess.Popen([exe, "-m", os.path.join(DL, "m", gguf), "--host", "127.0.0.1", "--port", str(PORT), "-ngl", ngl, "-t", "8",
                      "-c", "4096", "--jinja", "--no-webui"], stdout=subprocess.DEVNULL, stderr=open(os.path.join(HERE, "results", f"llm-{label}.log"), "w"), cwd=os.path.dirname(exe))
smp = Sampler([p.pid])
while True:
    try:
        if HTTP.get(f"http://127.0.0.1:{PORT}/health", timeout=1).status_code == 200: break
    except Exception: pass
    if p.poll() is not None: raise SystemExit("llama-server died")
    time.sleep(0.1)
load = time.perf_counter() - t0

def ask(text):
    body = {"temperature": 0, "max_tokens": 512, "chat_template_kwargs": {"enable_thinking": False},
            "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": f"<dictee>\n{sanitize(text)}\n</dictee>"}]}
    r = HTTP.post(f"http://127.0.0.1:{PORT}/v1/chat/completions", json=body, timeout=120).json()
    out = r["choices"][0]["message"]["content"] or ""
    out = re.sub(r"<think>.*?</think>", "", out, flags=re.S).strip()
    return out, r.get("usage", {})

ask("bonjour")  # warm-up
out_f = open(os.path.join(HERE, "results", f"llm-{label}.jsonl"), "w", encoding="utf-8")
for c in cases:
    t = time.perf_counter(); out, usage = ask(c["input"]); lat = time.perf_counter() - t
    lo = out.lower(); n_out = norm(out)
    fails = [m for m in c["must"] if m.lower() not in lo]
    fails += [n for n in c.get("must_num", []) if n not in out and norm(n) not in n_out]
    fails += ["NOT:" + m for m in c["must_not"] if m.lower() in lo]
    known = set(norm(c["input"]).split()) | set(norm(c["expected"]).split())
    words = n_out.split(); added = [w for w in words if w not in known]
    rec = {"model": label, "id": c["id"], "latency": round(lat, 3), "fidelity": round(1 - min(1, jiwer.wer(norm(c["expected"]), n_out or "_")), 3),
           "checks_ok": not fails, "fixed": [t for t in c.get("fixable", []) if t.lower() in lo], "fixable": c.get("fixable", []), "fails": fails, "added_words": added, "len_ratio": round(len(out) / max(1, len(c["input"])), 2),
           "out_tokens": usage.get("completion_tokens"), "input": c["input"], "output": out}
    out_f.write(json.dumps(rec, ensure_ascii=False) + "\n"); out_f.flush()
summ = {"model": label, "gguf": gguf, "build": build, "ngl": ngl, "load_s": round(load, 2), "file_gb": round(os.path.getsize(os.path.join(DL, "m", gguf)) / 1e9, 2), **smp.stop()}
json.dump(summ, open(os.path.join(HERE, "results", f"llm-{label}.summary.json"), "w")); print(json.dumps(summ))
p.kill()
