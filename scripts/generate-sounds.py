#!/usr/bin/env python3
"""Synthesize the bundled alert sounds (assets/sounds/*.wav).

The sounds are generated, not recorded, so they carry no third-party license.
Run from the repository root: python3 scripts/generate-sounds.py
"""
import math
import struct
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets" / "sounds"


def bell(freq, start, length, gain, total):
    """A soft bell: a sine with two quiet overtones and a fast exponential decay."""
    samples = [0.0] * total
    first = int(start * RATE)
    for i in range(int(length * RATE)):
        j = first + i
        if j >= total:
            break
        t = i / RATE
        attack = min(1.0, t / 0.004)
        decay = math.exp(-t * 7.0)
        tone = (
            math.sin(2 * math.pi * freq * t)
            + 0.30 * math.sin(2 * math.pi * freq * 2 * t)
            + 0.08 * math.sin(2 * math.pi * freq * 3 * t)
        )
        samples[j] += gain * attack * decay * tone
    return samples


def write(name, notes, seconds):
    total = int(seconds * RATE)
    mix = [0.0] * total
    for freq, start, length, gain in notes:
        for i, value in enumerate(bell(freq, start, length, gain, total)):
            mix[i] += value
    peak = max(abs(v) for v in mix) or 1.0
    scale = 0.6 / peak
    with wave.open(str(OUT / name), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes(
            b"".join(struct.pack("<h", int(max(-1.0, min(1.0, v * scale)) * 32767)) for v in mix)
        )


OUT.mkdir(parents=True, exist_ok=True)
# Done: a rising two-note chime, E5 then A5.
write("done.wav", [(659.25, 0.0, 0.5, 1.0), (880.0, 0.13, 0.55, 1.0)], 0.75)
# Request: two quick, higher pings that ask for attention.
write("request.wav", [(1174.66, 0.0, 0.25, 1.0), (1174.66, 0.17, 0.3, 1.0)], 0.5)
print("wrote", OUT)
