"""Speech engines the intelligibility test can render with.

Each engine renders a batch of (text, wav path) pairs at a numeric speed
setting, whose units are the engine's own; callers find the setting for a
target rate by measuring the output.
"""
import base64
import ctypes
import json
import os
import subprocess
import tempfile
import wave
from concurrent.futures import ThreadPoolExecutor

PARALLEL = 8


def _chunks(items, n):
    return [items[k::n] for k in range(n) if items[k::n]]


def _powershell(exe, script, items, setting):
    def run(chunk):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as f:
            json.dump([{"text": t, "wav": w} for t, w in chunk], f)
        try:
            code = script.replace("$JOBS", f"'{f.name}'").replace("$SETTING", str(setting))
            encoded = base64.b64encode(code.encode("utf-16-le")).decode()
            subprocess.run([exe, "-NoProfile", "-EncodedCommand", encoded], check=True, capture_output=True)
        finally:
            os.remove(f.name)
    with ThreadPoolExecutor(PARALLEL) as pool:
        list(pool.map(run, _chunks(items, PARALLEL)))


class Klatter:
    unit = "wpm"
    native = 220
    bounds = (50, 3000)
    integer = False

    def __init__(self, exe):
        self.exe = os.path.abspath(exe)
        if not os.path.isfile(self.exe):
            raise SystemExit(f"No engine named {exe!r}, and no Klatter executable at {self.exe}")

    def render(self, items, setting):
        def one(item):
            text, wav = item
            txt = wav[:-4] + ".txt"
            with open(txt, "w", encoding="utf-8") as f:
                f.write(text)
            try:
                subprocess.run([self.exe, "-r", str(setting), txt, wav], check=True)
            finally:
                os.remove(txt)
        with ThreadPoolExecutor(PARALLEL) as pool:
            list(pool.map(one, items))


class Sapi:
    unit = "SAPI rate"
    native = 0
    bounds = (-10, 10)
    integer = True
    SCRIPT = """
Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$VOICE
$s.Rate = $SETTING
foreach ($j in (Get-Content -Raw -Encoding UTF8 $JOBS | ConvertFrom-Json)) {
  $s.SetOutputToWaveFile($j.wav); $s.Speak($j.text)
}
$s.Dispose()
"""

    def __init__(self, voice=None):
        self.voice = voice

    def render(self, items, setting):
        select = f"$s.SelectVoice('{self.voice.replace(chr(39), chr(39) * 2)}')" if self.voice else ""
        _powershell("powershell", self.SCRIPT.replace("$VOICE", select), items, setting)


class Espeak:
    unit = "eSpeak wpm"
    native = 175
    bounds = (80, 1000)
    integer = True
    _lib = None

    def render(self, items, setting):
        lib = self._load()
        lib.espeak_SetParameter(1, setting, 0)
        for text, wav in items:
            self._samples.clear()
            data = text.encode()
            lib.espeak_Synth(data, len(data) + 1, 0, 1, 0, 1, None, None)
            lib.espeak_Synchronize()
            with wave.open(wav, "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(self._rate)
                w.writeframes(b"".join(self._samples))

    def _load(self):
        if Espeak._lib is None:
            import espeakng_loader
            lib = ctypes.cdll.LoadLibrary(espeakng_loader.get_library_path())
            Espeak._rate = lib.espeak_Initialize(2, 0, espeakng_loader.get_data_path().encode(), 0)
            Espeak._samples = []
            callback = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.POINTER(ctypes.c_short), ctypes.c_int, ctypes.c_void_p)
            Espeak._callback = callback(lambda w, n, _: (n > 0 and Espeak._samples.append(ctypes.string_at(w, n * 2))) or 0)
            lib.espeak_SetSynthCallback(Espeak._callback)
            lib.espeak_SetVoiceByName(b"en-us")
            Espeak._lib = lib
        return Espeak._lib


def parse(spec):
    """An engine from "sapi", "sapi:Voice Name", "espeak" or a Klatter executable path."""
    # Commands pasted from formatted text can carry non-breaking spaces.
    spec = spec.strip()
    if spec == "sapi" or spec.startswith("sapi:"):
        return Sapi(spec[5:] or None)
    if spec == "espeak":
        return Espeak()
    return Klatter(spec)
