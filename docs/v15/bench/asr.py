"""ASR bench: python asr.py <engine> <set> ... -> results/asr-<engine>.jsonl"""
import sys, os, json, time, glob, subprocess, requests, soundfile as sf
from common import Sampler, norm, term_hits
import jiwer
HTTP = requests.Session(); HTTP.trust_env = False  # no proxy lookup on loopback
HERE = os.path.dirname(os.path.abspath(__file__)); DL = os.path.join(HERE, "..", "dl")
engine, sets = sys.argv[1], sys.argv[2:]
corpus = {c["id"]: c for c in json.load(open(os.path.join(HERE, "corpus.json"), encoding="utf-8"))}
import glob as _g
fleurs = {}
for _f in _g.glob(os.path.join(HERE, "audio-*", "refs.json")): fleurs.update(json.load(open(_f, encoding="utf-8")))
os.makedirs(os.path.join(HERE, "results"), exist_ok=True)

def items():
    for s in sets:
        for p in sorted(glob.glob(os.path.join(HERE, f"audio-{s}", "*.wav"))):
            i = os.path.splitext(os.path.basename(p))[0]
            c = corpus.get(i) or {"ref": fleurs[i], "terms": [], "cat": s}
            yield s, i, p, c

class Sherpa:
    def __init__(self, provider):
        import sherpa_onnx
        d = glob.glob(os.path.join(DL, "x", "parakeet", "*"))[0]
        self.r = sherpa_onnx.OfflineRecognizer.from_transducer(
            encoder=f"{d}/encoder.int8.onnx", decoder=f"{d}/decoder.int8.onnx", joiner=f"{d}/joiner.int8.onnx",
            tokens=f"{d}/tokens.txt", model_type="nemo_transducer", num_threads=8, provider=provider)
    def run(self, p):
        x, sr = sf.read(p, dtype="float32"); s = self.r.create_stream(); s.accept_waveform(sr, x)
        self.r.decode_stream(s); return s.result.text

class WhisperServer:
    def __init__(self, build, model):
        exe = glob.glob(os.path.join(DL, "x", build, "**", "whisper-server.exe"), recursive=True)[0]
        self.p = subprocess.Popen([exe, "-m", os.path.join(DL, "m", model), "-l", "fr", "--host", "127.0.0.1", "--port", "18181", "-t", "8"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=os.path.dirname(exe))
        self.pid = self.p.pid
        while True:
            try:
                HTTP.get("http://127.0.0.1:18181/", timeout=1); break
            except Exception:
                if self.p.poll() is not None: raise SystemExit("whisper-server died")
                time.sleep(0.1)
    def run(self, p):
        with open(p, "rb") as f:
            r = HTTP.post("http://127.0.0.1:18181/inference", files={"file": f}, data={"response_format": "json", "temperature": "0.0", "language": "fr"})
        return r.json()["text"]
    def close(self): self.p.kill()

class Cli:
    """One process per dictation: latency includes model load (cold path)."""
    def __init__(self, build, tool, model, extra):
        self.exe = glob.glob(os.path.join(DL, "x", build, "**", tool), recursive=True)[0]
        self.args = ["-t", "8", "-m", os.path.join(DL, "m", model)] + extra
    def run(self, p):
        r = subprocess.run([self.exe, *self.args, "-f", p], capture_output=True, cwd=os.path.dirname(self.exe))
        out = r.stdout.decode("utf-8", "replace").strip()
        return " ".join(l.strip() for l in out.splitlines() if l.strip())

class LlamaAsr:
    """llama-server (mtmd) with an audio GGUF + mmproj; warm server, one HTTP call per dictation."""
    def __init__(self, build, model, mmproj, ngl):
        exe = os.path.join(DL, "x", build, "llama-server.exe")
        self.p = subprocess.Popen([exe, "-m", os.path.join(DL, "m", model), "--mmproj", os.path.join(DL, "m", mmproj),
                                   "--host", "127.0.0.1", "--port", "18183", "-ngl", ngl, "-t", "8", "-c", "4096"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=os.path.dirname(exe))
        self.pid = self.p.pid
        while True:
            try:
                if HTTP.get("http://127.0.0.1:18183/health", timeout=1).status_code == 200: break
            except Exception: pass
            if self.p.poll() is not None: raise SystemExit("llama-server died")
            time.sleep(0.1)
    def run(self, p):
        import base64
        b = base64.b64encode(open(p, "rb").read()).decode()
        r = HTTP.post("http://127.0.0.1:18183/v1/chat/completions", json={"temperature": 0, "messages": [
            {"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": b, "format": "wav"}}]}]})
        return r.json()["choices"][0]["message"]["content"].split("<asr_text>")[-1]
    def close(self): self.p.kill()

class QwenAsr:
    def __init__(self, name):
        import torch
        from qwen_asr import Qwen3ASRModel
        self.m = Qwen3ASRModel.from_pretrained(name, dtype=torch.bfloat16, device_map="cuda:0", max_new_tokens=512)
    def run(self, p):
        return self.m.transcribe(audio=p, language="French")[0].text

pids = [os.getpid()]
t0 = time.perf_counter(); smp = Sampler(pids)
if engine == "parakeet-cpu": e = Sherpa("cpu")
elif engine == "parakeet-cuda": e = Sherpa("cuda")
elif engine.startswith("ggml-parakeet:"):
    _, build, model, *flags = engine.split(":")
    e = Cli(build, "parakeet-cli.exe", model, ["-ng"] if "cpu" in flags else [])
elif engine.startswith("whisper-cli:"):
    _, build, model, *flags = engine.split(":")
    e = Cli(build, "whisper-cli.exe", model, ["-l", "fr", "-nt", "-np"] + (["-ng"] if "cpu" in flags else []))
elif engine.startswith("whisper-"):
    _, build, model = engine.split(":") if ":" in engine else (None, None, None)
    e = WhisperServer(build, model); smp.pids.append(e.pid)
elif engine.startswith("llama-asr:"):
    _, build, size, ngl = engine.split(":")
    e = LlamaAsr(build, f"Qwen3-ASR-{size}-Q8_0.gguf", f"mmproj-Qwen3-ASR-{size}-Q8_0.gguf", ngl); smp.pids.append(e.pid)
elif engine.startswith("qwen3-asr"): e = QwenAsr("Qwen/Qwen3-ASR-1.7B" if "1.7" in engine else "Qwen/Qwen3-ASR-0.6B")
else: raise SystemExit(engine)
load = time.perf_counter() - t0
# warm-up so the first measured item is not the one paying lazy init
e.run(os.path.join(HERE, "audio-tts", "s01-court.wav"))
first = time.perf_counter() - t0
tag = engine.replace(":", "_").replace(".", "") + os.environ.get("TAG", "")
out = open(os.path.join(HERE, "results", f"asr-{tag}.jsonl"), "w", encoding="utf-8")
for s, i, p, c in items():
    dur = sf.info(p).duration; t = time.perf_counter(); hyp = e.run(p).strip(); lat = time.perf_counter() - t
    w = (jiwer.wer(norm(c["ref"]), norm(hyp)) if norm(hyp) else 1.0) if c["ref"] else float(len(norm(hyp).split()))
    rec = {"engine": engine, "set": s, "id": i, "cat": c["cat"], "dur": round(dur, 2), "latency": round(lat, 3), "wer": round(w, 4),
           "terms": len(c["terms"]), "term_hits": len(term_hits(hyp, c["terms"])), "hyp": hyp, "ref": c["ref"]}
    out.write(json.dumps(rec, ensure_ascii=False) + "\n"); out.flush()
summ = {"engine": engine, "load_s": round(load, 2), "first_result_s": round(first, 2), **smp.stop()}
json.dump(summ, open(os.path.join(HERE, "results", f"asr-{tag}.summary.json"), "w"))
print(json.dumps(summ))
if hasattr(e, "close"): e.close()
