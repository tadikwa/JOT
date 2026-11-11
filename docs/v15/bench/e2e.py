"""ASR + LLM resident together: python e2e.py <build> <asr size> <llm gguf> <ngl> <label>"""
import sys, os, json, time, glob, base64, re, subprocess, requests
from common import Sampler, gpu_used_mib
HERE = os.path.dirname(os.path.abspath(__file__)); DL = os.path.join(HERE, "..", "dl")
build, size, gguf, ngl, label = sys.argv[1:6]
H = requests.Session(); H.trust_env = False
exe = os.path.join(DL, "x", build, "llama-server.exe")
base = gpu_used_mib(); t0 = time.perf_counter()
def start(args, port):
    return subprocess.Popen([exe, *args, "--host", "127.0.0.1", "--port", str(port), "-ngl", ngl, "-t", "8", "-c", "4096", "--no-webui"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=os.path.dirname(exe))
def wait(port, p):
    while True:
        try:
            if H.get(f"http://127.0.0.1:{port}/health", timeout=1).status_code == 200: return
        except Exception: pass
        if p.poll() is not None: raise SystemExit("server died")
        time.sleep(0.1)
a = start(["-m", f"{DL}/m/Qwen3-ASR-{size}-Q8_0.gguf", "--mmproj", f"{DL}/m/mmproj-Qwen3-ASR-{size}-Q8_0.gguf"], 18201)
l = start(["-m", f"{DL}/m/{gguf}", "--jinja"], 18202)
smp = Sampler([a.pid, l.pid]); smp.base_vram = base
wait(18201, a); wait(18202, l); load = time.perf_counter() - t0
SYSTEM = open(os.path.join(HERE, "prompt.txt"), encoding="utf-8").read().replace("{vocab}", "Proxmox, Ansible, Kubernetes, playbook, Claude Code, nginx, NVMe, Ceph, ZFS")
def asr(p):
    b = base64.b64encode(open(p, "rb").read()).decode()
    r = H.post("http://127.0.0.1:18201/v1/chat/completions", json={"temperature": 0, "messages": [{"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": b, "format": "wav"}}]}]}).json()
    return r["choices"][0]["message"]["content"].split("<asr_text>")[-1]
def llm(t):
    r = H.post("http://127.0.0.1:18202/v1/chat/completions", json={"temperature": 0, "max_tokens": 512, "chat_template_kwargs": {"enable_thinking": False},
        "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": f"<dictee>\n{t}\n</dictee>"}]}).json()
    return r["choices"][0]["message"]["content"].strip()
llm(asr(f"{HERE}/audio-tts/s01-court.wav"))
first = time.perf_counter() - t0
rows = []
for p in sorted(glob.glob(f"{HERE}/audio-tts/*.wav")):
    t = time.perf_counter(); raw = asr(p); ta = time.perf_counter() - t; out = llm(raw); tl = time.perf_counter() - t - ta
    rows.append({"id": os.path.basename(p)[:-4], "asr_s": round(ta, 3), "llm_s": round(tl, 3), "total_s": round(ta + tl, 3), "out": out})
sm = {"label": label, "load_both_s": round(load, 2), "first_result_s": round(first, 2), **smp.stop(),
      "mean_total_s": round(sum(r["total_s"] for r in rows) / len(rows), 3), "max_total_s": max(r["total_s"] for r in rows)}
json.dump({"summary": sm, "rows": rows}, open(f"{HERE}/results/e2e-{label}.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print(json.dumps(sm)); a.kill(); l.kill()
