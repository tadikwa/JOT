"""Terminology audit, ASR side: does Qwen3-ASR on llama-server b12289 take a context?
JOT_BENCH_ROOT=<isolated root holding local/runtime/b12289 and local/models>
python -I context.py <wav dir made by tts.ps1 from context_cases.json> <out dir>
Writes asr.jsonl (raw with and without a system context), template.txt (the GGUF
chat template, to see whether a system turn reaches the model) and cases.json
(the raw transcripts, for the Rust reformulation_bench). Fictional texts only.
"""
import sys, os, json, time, socket, subprocess, base64, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.environ["JOT_BENCH_ROOT"]  # never %LOCALAPPDATA%\Jot
EXE = os.path.join(ROOT, "local", "runtime", "b12289", "llama-server.exe")
MODELS = os.path.join(ROOT, "local", "models")
WAVS, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
CASES = json.load(open(os.path.join(HERE, "context_cases.json"), encoding="utf-8"))
CONTEXT = "Jot, Ctrl, GPU, Proxmox, Ansible, Claude Code"
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def call(port, path, body=None, timeout=60):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=json.dumps(body).encode() if body else None,
                                 headers={"Content-Type": "application/json"})
    with HTTP.open(req, timeout=timeout) as r:
        return json.loads(r.read())

s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
# Same arguments as LocalEngine::spawn_and_wait for the transcription pack.
p = subprocess.Popen([EXE, "-m", os.path.join(MODELS, "Qwen3-ASR-1.7B-Q8_0.gguf"),
                      "--mmproj", os.path.join(MODELS, "mmproj-Qwen3-ASR-1.7B-Q8_0.gguf"),
                      "--host", "127.0.0.1", "--port", str(port), "--no-webui", "--offline", "--jinja",
                      "-c", "16384", "--device", "Vulkan0"],
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=os.path.dirname(EXE))
try:
    while True:
        if p.poll() is not None: raise SystemExit("llama-server died")
        try:
            if HTTP.open(f"http://127.0.0.1:{port}/health", timeout=2).status == 200: break
        except Exception: pass
        time.sleep(0.25)
    props = call(port, "/props")
    open(os.path.join(OUT, "template.txt"), "w", encoding="utf-8").write(props.get("chat_template", ""))
    rows, cases = [], []
    for c in CASES:
        audio = base64.b64encode(open(os.path.join(WAVS, c["id"] + ".wav"), "rb").read()).decode()
        user = {"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]}
        row = {"id": c["id"]}
        for mode, msgs in [("prod", [user]), ("context", [{"role": "system", "content": CONTEXT}, user])]:
            t = time.perf_counter()
            r = call(port, "/v1/chat/completions", {"messages": msgs, "temperature": 0, "max_tokens": 200}, 120)
            text = (r["choices"][0]["message"]["content"] or "").split("<asr_text>")[-1].strip()
            row[mode] = text; row[mode + "_s"] = round(time.perf_counter() - t, 2)
            row[mode + "_prompt_tokens"] = r.get("usage", {}).get("prompt_tokens")
        rows.append(row); cases.append({"id": c["id"], "input": row["prod"]})
        print(json.dumps(row, ensure_ascii=False), flush=True)
    with open(os.path.join(OUT, "asr.jsonl"), "w", encoding="utf-8") as f:
        for row in rows: f.write(json.dumps(row, ensure_ascii=False) + "\n")
    json.dump(cases, open(os.path.join(OUT, "cases.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)
finally:
    p.kill(); p.wait(); print(f"killed pid {p.pid}")
