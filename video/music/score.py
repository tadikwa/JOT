"""The demo video's soundtrack: an original piece, synthesised here note by note, so it carries no
third-party rights (adapted from the generator its author wrote for the LocalChords video).
120 BPM, one bar = 2 s = 60 video frames; every section of the edit starts on a bar line
(src/Presentation.tsx, T). Needs numpy, scipy and soundfile:

    python video/music/score.py video/public/audio/score.wav

Sections (seconds): 0-6 intro swell, 6-56 groove (arpeggio from 16 s), 56-62 ending chord.
"""

import sys

import numpy as np
import soundfile
from scipy.signal import butter, fftconvolve, sosfilt

SR = 44100
BPM = 120
BEAT = 60 / BPM
BAR = 4 * BEAT
LENGTH = 62.0
N = int(LENGTH * SR)
rng = np.random.default_rng(7)

# One chord per bar, in C major / A minor: warm, a little open. (MIDI notes, lowest = bass root.)
F7 = [41, 53, 57, 60, 64]  # Fmaj7
CE = [40, 52, 55, 60, 62]  # C/E (add9)
AM = [45, 57, 60, 64, 67]  # Am7
GS = [43, 55, 60, 62, 67]  # Gsus4
G = [43, 55, 59, 62, 67]  # G
C9 = [36, 48, 55, 60, 62, 64]  # Cadd9, the ending
LOOP = [F7, CE, AM, GS]
CHORDS = [F7, CE, AM] + LOOP * 6 + [G]  # bars 0-27 (0-56 s)
CHORDS += [C9]  # the ending chord from 56 s


def hz(m):
    return 440.0 * 2 ** ((np.asarray(m) - 69) / 12)


def lowpass(x, f, order=2):
    return sosfilt(butter(order, f, "low", fs=SR, output="sos"), x, axis=0)


def highpass(x, f, order=2):
    return sosfilt(butter(order, f, "high", fs=SR, output="sos"), x, axis=0)


def bandpass(x, lo, hi):
    return sosfilt(butter(2, [lo, hi], "band", fs=SR, output="sos"), x, axis=0)


def env(n, a, r, sustain=1.0):
    """Attack a seconds, hold, release r seconds at the end."""
    e = np.full(n, sustain)
    na, nr = min(n, int(a * SR)), min(n, int(r * SR))
    if na:
        e[:na] = np.linspace(0, sustain, na) ** 1.5
    if nr:
        e[-nr:] *= np.linspace(1, 0, nr) ** 2
    return e


def place(out, x, t, gain=1.0):
    i = int(t * SR)
    if i >= len(out):
        return
    x = x[: len(out) - i]
    out[i : i + len(x)] += gain * (x if x.ndim == 2 else np.stack([x, x], 1))


def saw(f, n, phase=0.0):
    t = np.arange(n) / SR
    return 2 * ((f * t + phase) % 1.0) - 1


# Instruments --------------------------------------------------------------------------------------
def pad(notes, seconds, bright=1800):
    n = int(seconds * SR)
    out = np.zeros((n, 2))
    for m in notes[1:]:
        for k, (cents, pan) in enumerate(((-7, 0.2), (0, 0.5), (7, 0.8))):
            f = hz(m) * 2 ** (cents / 1200)
            v = saw(f, n, rng.random())
            out[:, 0] += v * (1 - pan)
            out[:, 1] += v * pan
    out = lowpass(out, bright, 2) * 0.05
    return out * env(n, 0.6, 0.9)[:, None]


def sub(m, seconds):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    x = np.sin(2 * np.pi * hz(m) * t) + 0.25 * np.sin(4 * np.pi * hz(m) * t)
    return x * env(n, 0.01, 0.12) * 0.32


def pluck(m, seconds=0.5, damp=0.996):
    """Karplus-Strong string: a soft, glassy pluck for the arpeggio."""
    f = hz(m)
    period = max(2, int(round(SR / f)))
    n = int(seconds * SR)
    buf = lowpass(rng.uniform(-1, 1, period), 5000, 1)
    out = np.zeros(n)
    for i in range(n):
        j = i % period
        out[i] = buf[j]
        buf[j] = damp * 0.5 * (buf[j] + buf[(j + 1) % period])
    return lowpass(out, 3200, 2) * env(n, 0.002, 0.08)


def kick():
    n = int(0.42 * SR)
    t = np.arange(n) / SR
    f = 45 + 110 * np.exp(-t * 28)
    x = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 7.5)
    x += 0.25 * lowpass(rng.uniform(-1, 1, n), 3000) * np.exp(-t * 180)
    return np.tanh(1.6 * x) * 0.55


def hat(open_=False):
    n = int((0.22 if open_ else 0.05) * SR)
    t = np.arange(n) / SR
    x = highpass(rng.uniform(-1, 1, n), 7500, 2) * np.exp(-t * (14 if open_ else 70))
    return x * 0.18


def clap():
    n = int(0.35 * SR)
    t = np.arange(n) / SR
    x = bandpass(rng.uniform(-1, 1, n), 900, 5200)
    e = np.exp(-t * 22)
    for d in (0.0, 0.011, 0.022):  # three hands, a hair apart
        k = int(d * SR)
        e[k:] += 0.6 * np.exp(-t[: n - k] * 140)
    return x * e * 0.16


def bell(m, seconds=2.5, gain=0.12):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    f = hz(m)
    mod = 1.4 * np.exp(-t * 3) * np.sin(2 * np.pi * f * 3.5 * t)
    return np.sin(2 * np.pi * f * t + mod) * np.exp(-t * 2.2) * gain


def swell(seconds, up=True):
    """Filtered noise rising into a cut (or a whoosh falling away from it)."""
    n = int(seconds * SR)
    x = rng.uniform(-1, 1, (n, 2))
    out = np.zeros_like(x)
    steps = 24
    for k in range(steps):  # a cutoff that climbs, in slices with short crossfades
        a, b = k * n // steps, (k + 1) * n // steps
        f = 300 * (40 ** (k / (steps - 1)))
        out[a:b] = bandpass(x[a:b], max(60, f * 0.5), min(18000, f * 1.5))
    shape = np.linspace(0, 1, n) ** 2.5
    if not up:
        shape = shape[::-1]
    return out * shape[:, None] * 0.22


def boom():
    n = int(2.8 * SR)
    t = np.arange(n) / SR
    f = 32 + 60 * np.exp(-t * 6)
    x = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 1.6)
    return np.tanh(2 * x) * 0.45


def reverb(x, seconds=2.6, mix=0.28):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    ir = rng.uniform(-1, 1, (n, 2)) * np.exp(-t * 6.9 / seconds)[:, None]
    ir = lowpass(ir, 6000, 1)
    ir[: int(0.012 * SR)] = 0  # a little pre-delay
    wet = np.stack([fftconvolve(x[:, c], ir[:, c])[: len(x)] for c in (0, 1)], 1)
    wet *= np.sqrt((x**2).sum() / max((wet**2).sum(), 1e-9))
    return x + mix * wet


# Arrangement ------------------------------------------------------------------------------------
def bars(a, b):
    return [i for i in range(a, b)]


def build():
    pads, bass, drums, arp, fx = (np.zeros((N, 2)) for _ in range(5))
    groove = bars(3, 28)  # where the beat plays
    for i, chord in enumerate(CHORDS):
        if chord is None:
            continue
        t0 = i * BAR
        last = i == len(CHORDS) - 1
        place(pads, pad(chord, BAR + (5.0 if last else 1.0), 1400 if i < 3 else 2200), t0)
        if i >= 1 and not last:  # bass: root on 1, the and of 2, and 4
            for b, length in ((0, 0.9), (1.5, 0.45), (3, 0.9)):
                if i in groove or b == 0:
                    place(bass, sub(chord[0] - 12 if chord[0] > 40 else chord[0], length), t0 + b * BEAT)
        if last:
            place(bass, sub(chord[0], 5.5), t0)
    k, h, ho, cl = kick(), hat(), hat(True), clap()
    for i in groove:
        t0 = i * BAR
        light = i in (15, 16)  # a breath in the middle of the edit
        for b in range(4):
            if not light or b == 0:
                place(drums, k, t0 + b * BEAT)
            place(drums, h, t0 + (b + 0.5) * BEAT, 1.0)
            if i >= 8:
                place(drums, h, t0 + (b + 0.25) * BEAT, 0.35)
                place(drums, h, t0 + (b + 0.75) * BEAT, 0.45)
        if not light:
            place(drums, cl, t0 + BEAT)
            place(drums, cl, t0 + 3 * BEAT)
        if i % 4 == 2:
            place(drums, ho, t0 + 3.5 * BEAT, 0.8)
    # The arpeggio: from the learning section on, 16ths over the chord tones.
    for i in bars(8, 28):
        chord = CHORDS[i]
        tones = sorted(set(chord[1:]))
        seq = [tones[j % len(tones)] + 12 * (j // len(tones)) for j in (0, 1, 2, 3, 4, 2, 3, 1)]
        for s in range(16):
            place(arp, pluck(seq[s % 8] + 12, 0.45), i * BAR + s * BEAT / 4, 0.16 if s % 4 == 0 else 0.11)
    # Intro and ending colours.
    place(fx, boom(), 0.25)
    for t, m in ((0.3, 84), (1.8, 79), (3.0, 88)):
        place(fx, bell(m), t)
    place(fx, swell(2.0), 6.0 - 2.0)  # into the groove
    place(fx, swell(2.0), 56.0 - 2.0, 0.7)  # into the ending
    place(fx, boom(), 56.0, 0.9)
    for t, m in ((56.0, 76), (56.6, 79), (57.2, 84), (58.4, 88)):
        place(fx, bell(m, 3.5, 0.1), t)
    # Sidechain: pads, bass and arpeggio duck under each kick.
    duck = np.ones(N)
    shape = 1 - 0.55 * np.exp(-np.arange(int(BEAT * SR)) / SR * 9)
    for i in groove:
        for b in range(4):
            a = int((i * BAR + b * BEAT) * SR)
            duck[a : a + len(shape)] = np.minimum(duck[a : a + len(shape)], shape[: N - a])
    music = (pads + bass * 0.9 + arp) * duck[:, None]
    mix = reverb(music + drums * 0.9, 2.6, 0.22) + reverb(fx, 3.5, 0.5) * 0.9
    mix = highpass(mix, 28, 2)
    end = int(60.5 * SR)
    mix[end:] *= np.linspace(1, 0, N - end)[:, None] ** 2
    mix = np.tanh(1.1 * mix) / np.tanh(1.1)
    return mix / np.abs(mix).max() * 0.89  # about -1 dBFS peak


if __name__ == "__main__":
    out = build()
    soundfile.write(sys.argv[1], out.astype(np.float32), SR, subtype="PCM_24")
    print(sys.argv[1], round(len(out) / SR, 2), "s")
