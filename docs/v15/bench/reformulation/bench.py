"""Prompt iteration bench: mirrors the request of crates/jot-core/src/local.rs and its gate.
JOT_BENCH_ROOT=<data root holding local/runtime/b12289 and local/models> [CASES=<corpus.json>]
python bench.py <gguf file name> <variant.json> [...]   -> out/<model>__<variant>.jsonl
Fictional texts only; never point it at a personal profile.
"""
import sys, os, json, re, time, subprocess, urllib.request, socket

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.environ["JOT_BENCH_ROOT"]  # isolated bench root, never %LOCALAPPDATA%\Jot
EXE = os.path.join(ROOT, "local", "runtime", "b12289", "llama-server.exe")
CASES = json.load(open(os.environ.get("CASES", os.path.join(HERE, "..", "reformulation_cases.json")), encoding="utf-8"))
OUT = os.environ.get("BENCH_OUT", os.path.join(HERE, "out")); os.makedirs(OUT, exist_ok=True)

def post(port, body, timeout):
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(req, timeout=timeout) as r:
        return json.loads(r.read())

def start(gguf):
    s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
    args = [EXE, "-m", os.path.join(ROOT, "local", "models", gguf), "--host", "127.0.0.1", "--port", str(port),
            "--no-webui", "--offline", "--jinja", "-c", "8192", "--device", "Vulkan0",
            "--log-file", os.path.join(OUT, f"llama-{gguf}.log")]
    t = time.perf_counter()
    p = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=os.path.dirname(EXE))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    while True:
        if p.poll() is not None: raise SystemExit("server died")
        try:
            if opener.open(f"http://127.0.0.1:{port}/health", timeout=2).status == 200: break
        except Exception: pass
        time.sleep(0.25)
    print(f"[{gguf}] pid {p.pid} port {port} ready in {time.perf_counter()-t:.1f}s", flush=True)
    return p, port, time.perf_counter() - t

# ---- validation.rs port ----
NUM = [("zero","0"),("one","1"),("two","2"),("three","3"),("four","4"),("five","5"),("six","6"),("seven","7"),("eight","8"),("nine","9"),("ten","10"),("eleven","11"),("twelve","12"),("twenty","20"),("thirty","30"),("forty","40"),("fifty","50"),("hundred","100")]
def normalize(t):
    s = t.lower()
    for w, d in NUM: s = re.sub(rf"\b{w}\b", d, s)
    return s
def words(t): return [w for w in re.split(r"[^\w]|_", normalize(t)) if w]
def tri(s):
    c = [ch for ch in s if not ch.isspace()]
    if len(c) < 3: return {"".join(c)} if c else set()
    return {"".join(c[i:i+3]) for i in range(len(c)-2)}
def trisim(a, b):
    ta, tb = tri(a), tri(b)
    if not ta or not tb: return 1.0 if a == b else 0.0
    return len(ta & tb) / min(len(ta), len(tb))
ANSWER = re.compile(r"(?i)^(sure|okay|certainly|of course|great question|here('s| is)|i can('|no)t|as an ai|i'm (sorry|an ai))\b")
def validate(raw, clean):
    rw, cw = words(raw), words(clean)
    if not cw: return (not rw, "empty_output")
    if ANSWER.match(clean) and cw[:1] != rw[:1]: return (False, "answer_pattern")
    if "as an ai" in clean.lower() or "language model" in clean.lower(): return (False, "ai_self")
    if not rw: return (True, None)
    ratio = len(cw) / len(rw)
    if len(rw) >= 3 and ratio > 1.6: return (False, f"expansion_{ratio:.2f}")
    if len(rw) >= 6 and ratio < 0.2: return (False, f"shrink_{ratio:.2f}")
    rs = set(rw); cont = sum(w in rs for w in cw) / len(cw); t = trisim(normalize(raw), normalize(clean))
    if cont < 0.5 and t < 0.55: return (False, f"divergence_c{cont:.2f}_t{t:.2f}")
    if "?" in raw and "?" not in clean: return (False, "question_lost")
    return (True, None)
def strip_artifacts(t):
    s = t.strip()
    if s.startswith("```"): s = re.sub(r"(?i)```[a-z]*\n?", "", s).replace("```", "").strip()
    for l in ["CLEAN:", "Clean:", "Transcript:", "TRANSCRIPT:"]:
        if s.startswith(l): s = s[len(l):].strip()
    if len(s) > 1 and s.startswith('"') and s.endswith('"'): s = s[1:-1]
    return s
def sanitize(t): return re.sub(r"(?i)<\s*/?\s*dictee\s*>", " ", t)

def checks(c, out):
    lo = out.lower(); fails = []
    for m in c.get("must", []):
        if not any(alt.lower() in lo for alt in m.split("|")): fails.append("MISSING:" + m)
    for m in c.get("must_not", []):
        if m.lower() in lo: fails.append("PRESENT:" + m)
    for m in c.get("once", []):
        if lo.count(m.lower()) > 1: fails.append("REPEATED:" + m)
    if out.strip() in c.get("must_not_exact", []): fails.append("ANSWERED")
    r = len(out.split()) / max(1, len(c["input"].split()))
    if "max_ratio" in c and r > c["max_ratio"]: fails.append(f"RATIO:{r:.2f}>{c['max_ratio']}")
    return fails, round(r, 2)

def run(port, gguf, vpath):
    v = json.load(open(vpath, encoding="utf-8")); name = os.path.splitext(os.path.basename(vpath))[0]
    path = os.path.join(OUT, f"{gguf.split('-Q')[0]}__{name}.jsonl"); f = open(path, "w", encoding="utf-8")
    ok = 0; lat = []; toks = 0
    for c in CASES:
        raw = c["input"]; n = len(raw.split())
        msgs = [{"role": "system", "content": v["system"]}]
        for u, a in v.get("shots", []):
            msgs += [{"role": "user", "content": f"<dictee>{u}</dictee>"}, {"role": "assistant", "content": a}]
        msgs.append({"role": "user", "content": v.get("user", "<dictee>{}</dictee>").replace("{}", sanitize(raw))})
        body = {"messages": msgs, "temperature": 0, "max_tokens": n * 3 + 64, "chat_template_kwargs": {"enable_thinking": False}}
        body.update(v.get("params", {}))
        t = time.perf_counter(); r = post(port, body, 90); dt = time.perf_counter() - t
        content = r["choices"][0]["message"]["content"] or ""; fin = r["choices"][0].get("finish_reason")
        clean = strip_artifacts(content); acc, why = validate(raw, clean)
        if "dictee" in clean.lower(): acc, why = False, "tag"
        final = clean if acc else raw
        fails, ratio = checks(c, final)
        if fin != "stop": fails.append("TRUNCATED")
        if not acc: fails.append("REJECTED:" + str(why))
        ok += not fails; lat.append(dt); toks += r.get("usage", {}).get("prompt_tokens", 0)
        rec = {"id": c["id"], "latency": round(dt, 3), "prompt_tokens": r.get("usage", {}).get("prompt_tokens"), "out_tokens": r.get("usage", {}).get("completion_tokens"),
               "finish": fin, "accepted": acc, "reason": why, "ratio": ratio, "fails": fails, "input": raw, "llm": content, "final": final}
        f.write(json.dumps(rec, ensure_ascii=False) + "\n"); f.flush()
        print(f"  {'OK ' if not fails else 'BAD'} {c['id']:24s} {dt:5.2f}s r={ratio:<5} {';'.join(fails)}\n      -> {final}", flush=True)
    lat.sort()
    print(f"== {gguf} / {name}: {ok}/{len(CASES)} pass, median {lat[len(lat)//2]:.2f}s, max {lat[-1]:.2f}s, avg prompt tok {toks/len(CASES):.0f}", flush=True)

if __name__ == "__main__":
    gguf = sys.argv[1]
    p, port, load = start(gguf)
    try:
        post(port, {"messages": [{"role": "user", "content": "bonjour"}], "max_tokens": 4, "chat_template_kwargs": {"enable_thinking": False}}, 60)
        for v in sys.argv[2:]: run(port, gguf, v)
    finally:
        p.kill(); p.wait(); print(f"killed pid {p.pid}")
