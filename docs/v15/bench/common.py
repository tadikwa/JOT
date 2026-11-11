import json, re, threading, time, unicodedata, psutil
from num2words import num2words
try:
    import pynvml; pynvml.nvmlInit(); _H = pynvml.nvmlDeviceGetHandleByIndex(0)
except Exception:
    _H = None

def gpu_used_mib():
    return pynvml.nvmlDeviceGetMemoryInfo(_H).used / 2**20 if _H else 0.0

class Sampler:
    """Peak RSS of pids (+children) and peak global VRAM, every 50 ms."""
    def __init__(self, pids):
        self.pids, self.rss, self.vram, self.cpu = pids, 0.0, 0.0, 0.0
        self.base_vram = gpu_used_mib(); self._stop = False
        self.t = threading.Thread(target=self._run, daemon=True); self.t.start()
    def _procs(self):
        out = []
        for pid in self.pids:
            try:
                p = psutil.Process(pid); out += [p] + p.children(recursive=True)
            except psutil.Error: pass
        return out
    def _run(self):
        while not self._stop:
            rss = 0
            for p in self._procs():
                try: rss += p.memory_info().rss
                except psutil.Error: pass
            self.rss = max(self.rss, rss / 2**20)
            self.vram = max(self.vram, gpu_used_mib() - self.base_vram)
            time.sleep(0.05)
    def stop(self):
        self._stop = True; self.t.join(); return {"peak_rss_mib": round(self.rss), "peak_vram_delta_mib": round(self.vram)}

def norm(text):
    t = unicodedata.normalize("NFC", text.lower())
    t = re.sub(r"(\d+)[ \u202f\u00a0](\d{3})", r"\1\2", t)
    t = re.sub(r"\b(\d+) ?h ?(\d+)\b", r"\1 heures \2", t)
    t = re.sub(r"\b(\d+) ?h\b", r"\1 heures", t)
    t = re.sub(r"\b(\d+) ?go\b", r"\1 gigaoctets", t)
    t = re.sub(r"\b(\d+) ?to\b", r"\1 téraoctets", t)
    t = re.sub(r"\d+", lambda m: " " + num2words(int(m.group()), lang="fr") + " ", t)
    t = t.replace("’", "'").replace("œ", "oe")
    t = re.sub(r"[^\w' -]", " ", t)
    t = re.sub(r"[-']", " ", t)
    return re.sub(r"\s+", " ", t).strip()

def term_hits(hyp, terms):
    h = unicodedata.normalize("NFC", hyp.lower())
    return [t for t in terms if t.lower() in h]
