#!/usr/bin/env python3
"""Numeric audit of the Android sound set (standard library only).

Reads apps/android/app/src/main/res/raw/fx_*.wav and the three desktop
references crates/ui/assets/sounds/{done,request,attention}.wav, measures
level, onset, ends, DC, spectrum and pitch direction, runs assertions, writes
docs/sound-design/android-audit.md and exits non-zero if any assertion fails.

Definitions
  active region   first..last sample whose magnitude is >= peak - 30 dB
                  (same rule as scripts/generate-android-sounds.py)
  RMS active      RMS over the active region; RMS all is over the whole file
  centroid        magnitude-weighted mean frequency, 20 Hz..20 kHz, of the whole
                  file (the files fade to zero, so no analysis window is used)
  dominant        magnitude peak (parabolic interpolation), 20 Hz and up
  direction       the active region is split where half of its energy has
                  passed; the dominant frequency (and centroid) of each part
                  is measured with a Hann window; direction is the change in
                  semitones, second part vs first part. Positive = rising.
  onset           time of the first sample >= peak - 40 dB (the leading silence
                  a listener waits through; asserted <= 1 ms for every fx_ file)
  clean ends      first and last sample are 0 (within 1 LSB) and the three
                  samples next to each end stay within 2 LSB (or 0.2% of the peak, the louder the file the more LSBs a fade
                  covers per sample), which with the
                  signal's own slope means the file starts and ends on a
                  zero crossing; "zc" additionally reports whether a sign
                  change (or a zero) occurs in the first/last 8 samples.
Desktop references are stereo; they are analysed as the L/R average.
"""
import io
import math
import re
import struct
import subprocess
import sys
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'apps/android/app/src/main/res/raw'
SOUNDS = ROOT / 'crates/ui/assets/sounds'
REPORT = ROOT / 'docs/sound-design/android-audit.md'

ACTIVE_FLOOR_DB = -30.0
CORE = ['tap', 'select', 'toggle_on', 'toggle_off', 'open', 'close', 'detent', 'star',
        'unstar', 'pin', 'archive', 'delete', 'copy', 'error', 'refresh']
ROUND2 = ['surge', 'zip', 'rebound', 'fast_on', 'fast_off']
PROVIDERS = ['provider_claude', 'provider_codex', 'provider_cursor', 'provider_devin', 'provider_grok',
             'provider_hermes', 'provider_pi', 'provider_opencode', 'provider_antigravity',
             'provider_favorites', 'provider_other']
INTERFACE = CORE + ROUND2 + PROVIDERS
PROMOTED = ['send', 'queued', 'upload_ready', 'reconnected', 'undo']
CHIMES = ['chime_done', 'chime_request', 'chime_attention']  # in-app copies of the desktop chimes (+6 dB)
REFERENCES = ['done', 'request', 'attention']
# Maximum duration in ms per interface cue.
MAX_MS = {'tap': 60, 'select': 90, 'toggle_on': 90, 'toggle_off': 90, 'detent': 60,
          'open': 120, 'close': 120, 'star': 120, 'unstar': 120, 'pin': 120, 'copy': 120,
          'archive': 120, 'delete': 130, 'refresh': 130, 'error': 180,
          'surge': 600, 'zip': 120, 'rebound': 130, 'fast_on': 250, 'fast_off': 120}
MAX_MS.update({name: 120 for name in PROVIDERS})
NOTES = {
    'surge': 'rising power swell G4 to C6, detuned second voice for shimmer, climbing pentatonic sparkles, bright C-major bloom',
    'zip': 'fast airy falling streak (band-passed noise 7.5 to 1.8 kHz) with a thin pure zing G6 to C6',
    'rebound': 'soft elastic boing, G4 with an overshooting, wobbling pitch',
    'fast_on': 'quiet electric crackle (eight signed micro-pulses), bright G6 to G7 zap, small C7 bloom',
    'fast_off': 'soft tick and the charge draining, G6 falling to C5',
    'provider_claude': 'warm two-note rising pair, E5 then A5',
    'provider_codex': 'crisp bracket-like double tick, two identical hollow clicks on D6',
    'provider_cursor': 'one glassy blip, G6 bending up to A6, inharmonic partials',
    'provider_devin': 'soft pad-like minor third, A4 + C5, slow bloom',
    'provider_grok': 'bright quick fifth C5 to G5 with a three-grain sparkle',
    'provider_hermes': 'fast flutter up, seven scale steps 9 ms apart, a breath of air',
    'provider_pi': 'three-note tiny arpeggio on the digits 3-1-4: E5, C5, G5',
    'provider_opencode': 'open hollow tone, an open fifth D5 + A5 in odd harmonics',
    'provider_antigravity': 'floaty upward glide C5 to C6 with slow wobble and a higher echo',
    'provider_favorites': 'twinkle: four inharmonic bell tones C7, G6, C7, E7 fading',
    'provider_other': 'neutral soft pop: broad rounded pulse with a short low A4 body',
}
RMS_BAND_DB = 1.5
DESKTOP_MARGIN_DB = 2.0
BOOST_DB = 20 * math.log10(2.0)  # the slider headroom baked into every fx_ file (CueTable.ASSET_BOOST)
LIFT_DB = 0.0                    # "twice as loud again": default slider position vs the previous build's 100%
MIN_GAIN_DB = -0.5               # asserted for every cue (plain and A-weighted active RMS)
PREVIOUS_COMMIT = 'f1d4eff1'     # the build whose slider 100% is the reference
KOTLIN = ROOT / 'apps/android/app/src/main/java/sh/zeron/android/feedback'
ONSET_DB = -40.0
ONSET_MAX_MS = 1.0
DISTINCT_MIN = 0.20  # minimum fingerprint distance between two provider cues
PEAK_LIMIT_DB = -0.5
SIZE_LIMIT_BYTES = 800 * 1024
DIRECTION_MIN_ST = 0.5
EDGE_REL = 0.002  # neighbours of the first / last sample: 2 LSB or 0.2% of the peak (-54 dB), whichever is larger


def db(x):
    return 20 * math.log10(x) if x > 0 else -999.0


# --------------------------------------------------------------------- io

def read_wav(path):
    with wave.open(str(path), 'rb') as wav:
        channels, width, rate, count = (wav.getnchannels(), wav.getsampwidth(),
                                        wav.getframerate(), wav.getnframes())
        raw = wav.readframes(count)
    assert width == 2, f'{path}: only 16-bit supported'
    values = struct.unpack(f'<{count * channels}h', raw)
    mono = [sum(values[i * channels:(i + 1) * channels]) / channels for i in range(count)]
    return rate, channels, width * 8, mono


# -------------------------------------------------------------------- fft

def fft(values):
    n = len(values)
    j = 0
    data = list(values)
    for i in range(1, n):
        bit = n >> 1
        while j & bit:
            j ^= bit
            bit >>= 1
        j ^= bit
        if i < j:
            data[i], data[j] = data[j], data[i]
    size = 2
    while size <= n:
        step = -2j * math.pi / size
        w_step = complex(math.cos(step.imag), math.sin(step.imag))
        for start in range(0, n, size):
            w = 1 + 0j
            half = size // 2
            for k in range(start, start + half):
                even, odd = data[k], data[k + half] * w
                data[k], data[k + half] = even + odd, even - odd
                w *= w_step
        size <<= 1
    return data


def spectrum(x, rate, hann=False, min_bins=8192):
    n = len(x)
    size = max(min_bins, 1 << (4 * n - 1).bit_length())
    if hann:
        win = [0.5 - 0.5 * math.cos(2 * math.pi * i / max(n - 1, 1)) for i in range(n)]
        padded = [v * w for v, w in zip(x, win)]
    else:
        padded = list(x)
    padded += [0.0] * (size - n)
    mags = [abs(c) for c in fft(padded)[:size // 2 + 1]]
    return rate / size, mags


def centroid_and_dominant(x, rate, hann=False):
    df, mags = spectrum(x, rate, hann)
    lo, hi = max(1, int(20 / df)), min(len(mags) - 1, int(20000 / df))
    total = sum(mags[lo:hi + 1])
    centroid = sum(i * df * m for i, m in enumerate(mags[lo:hi + 1], lo)) / total
    peak = max(range(lo, hi + 1), key=lambda i: mags[i])
    a, b, c = mags[peak - 1], mags[peak], mags[peak + 1]
    denom = a - 2 * b + c
    shift = 0.5 * (a - c) / denom if denom else 0.0
    return centroid, (peak + shift) * df, df, mags


# ---------------------------------------------------------------- measures

def active_span(x):
    peak = max(abs(v) for v in x)
    floor = peak * 10 ** (ACTIVE_FLOOR_DB / 20)
    hits = [i for i, v in enumerate(x) if abs(v) >= floor]
    return hits[0], hits[-1] + 1


def rms(x):
    return math.sqrt(sum(v * v for v in x) / len(x))


def near_zero_crossing(x, at_end):
    edge = x[-8:] if at_end else x[:8]
    if any(abs(v) <= 1 for v in edge):
        return True
    return any(a * b <= 0 for a, b in zip(edge, edge[1:]))


def fingerprint(x, rate, df, mags):
    """A small vector describing how a cue sounds: 12 log-spaced spectral bands (200 Hz..9.6 kHz, log magnitude
    scaled so the strongest is 1), the loudness envelope in 24 slices of the active region (how many hits, how
    spaced), and the dominant pitch in 8 slices (the motif's contour). Used to assert that the provider cues
    are told apart by ear-relevant structure, not just by file name."""
    bands = []
    for k in range(12):
        lo = 200 * 2 ** (k * 5.6 / 12)
        hi = 200 * 2 ** ((k + 1) * 5.6 / 12)
        i0, i1 = max(1, int(lo / df)), max(2, int(hi / df))
        bands.append(math.log10(1e-9 + sum(m * m for m in mags[i0:i1])))
    top = max(bands)
    spec = [max(0.0, (b - (top - 4.0)) / 4.0) for b in bands]  # 0..1, floor 40 dB under the strongest band
    a, b = active_span(x)
    seg = x[a:b]
    size = max(1, len(seg) // 24)
    env = [math.sqrt(sum(v * v for v in seg[i * size:(i + 1) * size]) / size) if seg[i * size:(i + 1) * size] else 0.0
           for i in range(24)]
    peak = max(env) or 1.0
    env = [e / peak for e in env]
    pitches, last = [], 69.0
    psize = max(64, len(seg) // 8)
    for i in range(8):
        part = seg[i * psize:(i + 1) * psize]
        if len(part) < 32 or rms(part) < peak * 0.02:
            pitches.append(last)
            continue
        _, dom, _, _ = centroid_and_dominant(part, rate, hann=True)
        last = 69 + 12 * math.log2(max(dom, 50.0) / 440.0)
        pitches.append(last)
    return spec, env, [p / 24.0 for p in pitches]


def print_distance(p, q):
    """Mean of the root-mean-square gaps of the three parts (spectrum, envelope, pitch contour; 24 semitones = 1)."""
    gaps = [math.sqrt(sum((a - b) ** 2 for a, b in zip(u, v)) / len(u)) for u, v in zip(p, q)]
    return min(1.0, sum(gaps) / len(gaps))


def measure(path):
    rate, channels, bits, x = read_wav(path)
    a, b = active_span(x)
    peak = max(abs(v) for v in x)
    r_all, r_act = rms(x), rms(x[a:b])
    centroid, dominant, df, mags = centroid_and_dominant(x, rate)
    # Equal-energy split of the active region for the direction measure.
    seg = x[a:b]
    energy = sum(v * v for v in seg)
    acc, cut = 0.0, len(seg) // 2
    for i, v in enumerate(seg):
        acc += v * v
        if acc >= energy / 2:
            cut = max(8, min(i, len(seg) - 8))
            break
    onset_floor = peak * 10 ** (ONSET_DB / 20)
    onset = next(i for i, v in enumerate(x) if abs(v) >= onset_floor)
    c1, d1, _, _ = centroid_and_dominant(seg[:cut], rate, hann=True)
    c2, d2, _, _ = centroid_and_dominant(seg[cut:], rate, hann=True)
    # Power fraction within +-150 Hz of the dominant peak (narrowband check).
    power = [m * m for m in mags]
    lo, hi = max(0, int((dominant - 150) / df)), int((dominant + 150) / df) + 1
    return {
        'name': path.stem, 'rate': rate, 'channels': channels, 'bits': bits,
        'bytes': path.stat().st_size, 'ms': len(x) / rate * 1000,
        'peak_db': db(peak / 32768), 'clipped': any(abs(v) >= 32767 for v in x),
        'rms_all_db': db(r_all / 32768), 'rms_act_db': db(r_act / 32768),
        'active_ms': (b - a) / rate * 1000,
        'crest_db': db(peak / r_all), 'first': int(x[0]), 'last': int(x[-1]),
        'edge_ok': (abs(x[0]) <= 1 and abs(x[-1]) <= 1
                    and max(abs(v) for v in x[:3]) <= max(2, EDGE_REL * peak)
                    and max(abs(v) for v in x[-3:]) <= max(2, EDGE_REL * peak)),
        'zc_start': near_zero_crossing(x, False), 'zc_end': near_zero_crossing(x, True),
        'dc': sum(x) / len(x), 'centroid': centroid, 'dominant': dominant,
        'dir_dom_st': 12 * math.log2(d2 / d1), 'dir_cen_st': 12 * math.log2(c2 / c1),
        'dom1': d1, 'dom2': d2,
        'band_fraction': sum(power[lo:hi]) / sum(power[1:]),
        'onset_ms': onset / rate * 1000,
        'arms_act_db': loudness(x)[1],
        'print': fingerprint(x, rate, df, mags),
    }


# --------------------------------------------------------------- loudness

def _bilinear(num, den, fs):
    """Analog biquad (coefficients of s^2, s, 1; highest power first) to digital with s = 2 fs (1 - z^-1) / (1 + z^-1)."""
    k = 2.0 * fs

    def conv(c):
        c2, c1, c0 = c
        return (c2 * k * k + c1 * k + c0, -2 * c2 * k * k + 2 * c0, c2 * k * k - c1 * k + c0)

    n, d = conv(num), conv(den)
    return [v / d[0] for v in n], [v / d[0] for v in d]


def _a_weighting_sections(fs):
    w = [2 * math.pi * f for f in (20.598997, 107.65265, 737.86223, 12194.217)]
    sections = [
        ((1.0, 0.0, 0.0), (1.0, 2 * w[0], w[0] ** 2)),                           # s^2 / (s + w1)^2
        ((1.0, 0.0, 0.0), (1.0, w[1] + w[2], w[1] * w[2])),                      # s^2 / ((s + w2)(s + w3))
        ((0.0, 0.0, w[3] ** 2), (1.0, 2 * w[3], w[3] ** 2)),                     # w4^2 / (s + w4)^2
    ]
    digital = [_bilinear(n, d, fs) for n, d in sections]
    # Normalise to 0 dB at 1 kHz.
    z = complex(math.cos(2 * math.pi * 1000 / fs), math.sin(2 * math.pi * 1000 / fs))
    zi = 1 / z
    gain = 1.0
    for (b, a) in digital:
        gain *= abs((b[0] + b[1] * zi + b[2] * zi * zi) / (a[0] + a[1] * zi + a[2] * zi * zi))
    return digital, 1.0 / gain


def a_weight(x, fs=48000):
    """The A-weighting curve (IEC 61672 poles, bilinear transform, 0 dB at 1 kHz) applied to x."""
    sections, norm = _a_weighting_sections(fs)
    y = list(x)
    for b, a in sections:
        x1 = x2 = y1 = y2 = 0.0
        out = []
        for v in y:
            o = b[0] * v + b[1] * x1 + b[2] * x2 - a[1] * y1 - a[2] * y2
            x2, x1, y2, y1 = x1, v, y1, o
            out.append(o)
        y = out
    return [v * norm for v in y]


def loudness(x):
    """(plain, A-weighted) RMS in dBFS over the active region (the A-weighting runs over the whole file first)."""
    a, b = active_span(x)
    plain = db(rms(x[a:b]) / 32768)
    weighted = db(rms(a_weight(x)[a:b]) / 32768)
    return plain, weighted


def previous_wav(name):
    """A res/raw file of the previous build, straight from git (None when git or the commit is not available)."""
    try:
        data = subprocess.run(['git', 'show', f'{PREVIOUS_COMMIT}:apps/android/app/src/main/res/raw/{name}.wav'],
                              cwd=ROOT, capture_output=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    with wave.open(io.BytesIO(data), 'rb') as wav:
        count, channels = wav.getnframes(), wav.getnchannels()
        values = struct.unpack(f'<{count * channels}h', wav.readframes(count))
    return [sum(values[i * channels:(i + 1) * channels]) / channels for i in range(count)]


def previous_trims():
    """Per-resource trims of the previous build's CueTable (only the non-1.0 ones matter)."""
    try:
        text = subprocess.run(['git', 'show', f'{PREVIOUS_COMMIT}:apps/android/app/src/main/java/sh/zeron/android/feedback/SoundDesign.kt'],
                              cwd=ROOT, capture_output=True, check=True, text=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    return parse_trims(text)


def parse_trims(text):
    trims = {m.group(1): float(m.group(2)) for m in re.finditer(r'CueSpec\(cue, "(fx_\w+)", CueCategory\.\w+, ([\d.]+)f', text)}
    for m in re.finditer(r'provider\(cue, "(\w+)"', text):
        trims[f'fx_provider_{m.group(1)}'] = 1.0
    return trims


def kotlin_const(file, pattern):
    return float(re.search(pattern, (KOTLIN / file).read_text()).group(1))


def sound_pool_volume(slider, trim, asset_boost, max_gain):
    """CueTable.volume(spec, FeedbackSettings(volume = slider).gain): the curve of FeedbackSettings.gainFor."""
    v = 2 * min(max(slider, 0.0), 1.0)
    gain = v * v if v <= 1 else 2 ** (v - 1)
    return min(max(gain * trim / asset_boost, 0.0), 1.0)


# The previous build's files, measured once (plain, A-weighted active RMS in dBFS) so the audit also runs where the
# git history is not available; when it is, the numbers are re-measured and must agree.
PREVIOUS_RMS = {
    'tap': (-37.99, -38.95), 'select': (-37.98, -39.79), 'toggle_on': (-37.98, -38.69),
    'toggle_off': (-37.98, -39.18), 'open': (-37.98, -39.39), 'close': (-37.98, -40.08), 'detent': (-37.98, -38.43),
    'star': (-37.98, -37.18), 'unstar': (-37.98, -38.05), 'pin': (-37.98, -42.80), 'archive': (-37.98, -44.66),
    'delete': (-37.98, -47.48), 'copy': (-37.98, -37.80), 'error': (-37.98, -44.62), 'refresh': (-37.98, -40.00),
    'surge': (-37.98, -37.67), 'zip': (-37.98, -37.47), 'rebound': (-37.98, -42.27), 'fast_on': (-37.98, -36.88),
    'fast_off': (-37.98, -37.14), 'provider_claude': (-37.98, -38.66), 'provider_codex': (-37.98, -37.43),
    'provider_cursor': (-37.98, -36.98), 'provider_devin': (-37.98, -41.49), 'provider_grok': (-37.98, -38.84),
    'provider_hermes': (-37.98, -38.45), 'provider_pi': (-37.98, -39.60), 'provider_opencode': (-37.98, -39.35),
    'provider_antigravity': (-38.00, -38.90), 'provider_favorites': (-37.98, -36.83),
    'provider_other': (-37.98, -42.37), 'send': (-34.62, -38.18), 'queued': (-36.65, -40.77),
    'upload_ready': (-35.72, -40.56), 'reconnected': (-36.34, -41.15), 'undo': (-36.75, -40.94),
    'chime_done': (-34.26, -38.89), 'chime_request': (-35.69, -39.67), 'chime_attention': (-35.82, -41.44),
}
PREVIOUS_TRIMS = {'fx_send': 0.8, 'fx_queued': 0.8, 'fx_upload_ready': 0.8, 'fx_reconnected': 0.8, 'fx_undo': 0.8,
                  'fx_fast_on': 0.7, 'fx_fast_off': 0.8}


# ------------------------------------------------------------------- main

def main():
    results = {}
    for name in INTERFACE + PROMOTED + CHIMES:
        path = RAW / f'fx_{name}.wav'
        if not path.exists():
            print(f'missing {path}', file=sys.stderr)
            return 2
        results[name] = measure(path)
    for name in REFERENCES:
        results[name] = measure(SOUNDS / f'{name}.wav')
    m = results

    checks = []

    def check(label, ok, detail=''):
        checks.append((label, bool(ok), detail))

    fx = INTERFACE + PROMOTED + CHIMES
    for name in fx:
        r = m[name]
        check(f'{name}: mono 16-bit 48 kHz', (r['channels'], r['bits'], r['rate']) == (1, 16, 48000),
              f"{r['channels']} ch, {r['bits']} bit, {r['rate']} Hz")
        check(f'{name}: no clipping', not r['clipped'])
        check(f'{name}: peak <= {PEAK_LIMIT_DB:.1f} dBFS', r['peak_db'] <= PEAK_LIMIT_DB,
              f"{r['peak_db']:.1f}")
        check(f'{name}: onset within {ONSET_MAX_MS:g} ms (first sample >= {ONSET_DB:.0f} dB re peak)',
              r['onset_ms'] <= ONSET_MAX_MS, f"{r['onset_ms']:.2f} ms")
        check(f'{name}: clean ends (0 within 1 LSB, neighbours within 0.2% of peak, near zero crossing)',
              r['edge_ok'] and r['zc_start'] and r['zc_end'], f"first {r['first']}, last {r['last']}")
    for name in INTERFACE:
        r = m[name]
        check(f'{name}: duration <= {MAX_MS[name]} ms', r['ms'] <= MAX_MS[name], f"{r['ms']:.0f} ms")
        check(f'{name}: DC offset < 2% of RMS', abs(r['dc']) < 0.02 * 32768 * 10 ** (r['rms_all_db'] / 20),
              f"{r['dc']:.2f} LSB")

    check('Open centroid > Close centroid', m['open']['centroid'] > m['close']['centroid'],
          f"{m['open']['centroid']:.0f} vs {m['close']['centroid']:.0f} Hz")
    check('ToggleOn centroid > ToggleOff centroid', m['toggle_on']['centroid'] > m['toggle_off']['centroid'],
          f"{m['toggle_on']['centroid']:.0f} vs {m['toggle_off']['centroid']:.0f} Hz")
    check('Star centroid > Unstar centroid', m['star']['centroid'] > m['unstar']['centroid'],
          f"{m['star']['centroid']:.0f} vs {m['unstar']['centroid']:.0f} Hz")
    check('Select warmer (lower centroid) than Tap', m['select']['centroid'] < m['tap']['centroid'],
          f"{m['select']['centroid']:.0f} vs {m['tap']['centroid']:.0f} Hz")
    check('Tap is the shortest interface cue', all(m['tap']['ms'] <= m[n]['ms'] for n in INTERFACE),
          f"{m['tap']['ms']:.0f} ms")
    check('Surge is the longest interface cue and swells for 400+ ms',
          m['surge']['ms'] >= 400 and m['surge']['ms'] == max(m[n]['ms'] for n in INTERFACE),
          f"{m['surge']['ms']:.0f} ms")
    check('Every provider cue and Zip is 120 ms or less and FastOn 250 ms or less',
          all(m[n]['ms'] <= 120 for n in PROVIDERS + ['zip']) and m['fast_on']['ms'] <= 250, '')
    lowest = min(INTERFACE, key=lambda n: m[n]['centroid'])
    check('Delete has the lowest centroid of the interface set', lowest == 'delete',
          f"lowest is {lowest} ({m[lowest]['centroid']:.0f} Hz); delete {m['delete']['centroid']:.0f} Hz")
    for name, sign in [('toggle_on', 1), ('toggle_off', -1), ('star', 1), ('unstar', -1),
                       ('open', 1), ('close', -1), ('pin', 1), ('refresh', 1), ('error', -1),
                       ('archive', -1), ('delete', -1), ('surge', 1), ('zip', -1), ('fast_off', -1),
                       ('provider_claude', 1), ('provider_hermes', 1), ('provider_antigravity', 1),
                       ('provider_grok', 1)]:
        d = m[name]['dir_dom_st']
        check(f"{name} {'rises' if sign > 0 else 'falls'} (dominant, equal-energy halves)",
              sign * d >= DIRECTION_MIN_ST, f"{d:+.1f} st ({m[name]['dom1']:.0f} -> {m[name]['dom2']:.0f} Hz)")
    det = m['detent']
    check('Detent dominant frequency is 880 Hz (+-10)', abs(det['dominant'] - 880) <= 10,
          f"{det['dominant']:.1f} Hz")
    check('Detent is narrowband (>= 90% of power within +-150 Hz)', det['band_fraction'] >= 0.9,
          f"{det['band_fraction'] * 100:.1f}%")

    check('FastOn is bright (centroid above 2 kHz) and Zip is airy (centroid above 2.5 kHz)',
          m['fast_on']['centroid'] > 2000 and m['zip']['centroid'] > 2500,
          f"{m['fast_on']['centroid']:.0f} / {m['zip']['centroid']:.0f} Hz")
    check('Surge climbs at least an octave (dominant first half vs second half or centroid)',
          m['surge']['dir_dom_st'] >= 7 or m['surge']['dir_cen_st'] >= 7,
          f"{m['surge']['dir_dom_st']:+.1f} / {m['surge']['dir_cen_st']:+.1f} st")
    worst = None
    for i, a in enumerate(PROVIDERS):
        for b in PROVIDERS[i + 1:]:
            d = print_distance(m[a]['print'], m[b]['print'])
            if worst is None or d < worst[0]:
                worst = (d, a, b)
            check(f'{a} and {b} sound different (fingerprint distance >= {DISTINCT_MIN})', d >= DISTINCT_MIN, f'{d:.2f}')

    levels = sorted(m[n]['rms_act_db'] for n in INTERFACE)
    median = levels[len(levels) // 2]
    check(f'Interface active RMS within +-{RMS_BAND_DB} dB of the set median',
          all(abs(m[n]['rms_act_db'] - median) <= RMS_BAND_DB for n in INTERFACE),
          f'median {median:.2f} dBFS, spread {levels[-1] - levels[0]:.2f} dB')
    for ref, chime in zip(REFERENCES, CHIMES):
        margin = min(m[chime]['rms_act_db'] - m[n]['rms_act_db'] for n in INTERFACE)
        check(f'Every interface cue >= {DESKTOP_MARGIN_DB:g} dB quieter (active RMS) than in-app {chime}',
              margin >= DESKTOP_MARGIN_DB, f'smallest margin {margin:.1f} dB')
        gain = m[chime]['rms_act_db'] - m[ref]['rms_act_db']
        expected = 2 * BOOST_DB + LIFT_DB
        check(f'{chime} is the desktop {ref} plus {expected:.2f} dB: previous headroom {BOOST_DB:.2f} + slider headroom '
              f'{BOOST_DB:.2f} + lift {LIFT_DB:g} (+-0.3 dB)', abs(gain - expected) <= 0.3, f'{gain:+.2f} dB')
    total = sum(m[n]['bytes'] for n in fx)
    check('fx_*.wav total size < 800 KB', total < SIZE_LIMIT_BYTES, f'{total / 1024:.0f} KB')

    # ------------------------------------------- loudness against the previous build
    sd = (KOTLIN / 'SoundDesign.kt').read_text()
    asset_boost = kotlin_const('SoundDesign.kt', r'ASSET_BOOST = ([\d.]+)f')
    max_gain = kotlin_const('FeedbackSettings.kt', r'MAX_GAIN = ([\d.]+)f')
    default_volume = kotlin_const('FeedbackSettings.kt', r'DEFAULT_VOLUME = ([\d.]+)f')
    trims = parse_trims(sd)
    old_trims = previous_trims() or PREVIOUS_TRIMS
    check('SoundPool volume stays <= 1.0 at every slider position for every cue',
          all(sound_pool_volume(i / 100, trims[f'fx_{n}'], asset_boost, max_gain) <= 1.0
              for n in fx for i in range(101)), f'ASSET_BOOST {asset_boost:g}, MAX_GAIN {max_gain:g}')
    check('The default slider position is 50%', default_volume == 0.5, f'{default_volume:g}')
    loud = {}
    for n in fx:
        old_plain, old_a = PREVIOUS_RMS[n]
        wav = previous_wav(f'fx_{n}')
        if wav is not None:
            p, a = loudness(wav)
            check(f'{n}: stored baseline of the previous build still matches {PREVIOUS_COMMIT}',
                  abs(p - old_plain) < 0.05 and abs(a - old_a) < 0.05, f'{p:.2f}/{a:.2f} vs {old_plain:.2f}/{old_a:.2f}')
        trim_old = old_trims.get(f'fx_{n}', 1.0)
        old100 = 20 * math.log10(sound_pool_volume(1.0, trim_old, 2.0, 2.0))  # the previous build: ASSET_BOOST 2, MAX_GAIN 2
        trim = trims[f'fx_{n}']
        d = 20 * math.log10(sound_pool_volume(default_volume, trim, asset_boost, max_gain))
        t = 20 * math.log10(sound_pool_volume(1.0, trim, asset_boost, max_gain))
        r = m[n]
        loud[n] = {
            'old100': old_plain + old100, 'old100_a': old_a + old100,
            'new50': r['rms_act_db'] + d, 'new50_a': r['arms_act_db'] + d,
            'new100': r['rms_act_db'] + t, 'new100_a': r['arms_act_db'] + t,
        }
        l = loud[n]
        l['gain'], l['gain_a'] = l['new50'] - l['old100'], l['new50_a'] - l['old100_a']
        check(f'{n}: default slider (50%) is >= {MIN_GAIN_DB:g} dB louder than the previous build at 100% '
              f'(plain and A-weighted active RMS, SoundPool volume included)',
              l['gain'] >= MIN_GAIN_DB and l['gain_a'] >= MIN_GAIN_DB,
              f"{l['gain']:+.2f} dB plain, {l['gain_a']:+.2f} dB A-weighted")
        check(f'{n}: slider 100% is 6.02 dB above the default', abs(l['new100'] - l['new50'] - BOOST_DB) < 0.02,
              f"{l['new100'] - l['new50']:+.2f} dB")
    notify = {}
    for ref, chime in zip(REFERENCES, CHIMES):
        # Notification channels play fx_chime_* at file level (the system notification volume is the user's);
        # they used to play the desktop original.
        notify[ref] = (m[chime]['rms_act_db'] - m[ref]['rms_act_db'], m[chime]['arms_act_db'] - m[ref]['arms_act_db'])
        check(f'notification {ref}: channel sound (fx_{chime}) >= {MIN_GAIN_DB:g} dB louder than the desktop original',
              min(notify[ref]) >= MIN_GAIN_DB, f'{notify[ref][0]:+.1f} dB plain, {notify[ref][1]:+.1f} dB A-weighted')

    # ------------------------------------------------------------- report
    out = ['<!-- generated by scripts/audit-android-sounds.py, do not edit -->', '',
           '# Android sound audit', '',
           'Generated by `scripts/audit-android-sounds.py`, do not edit. Regenerate with',
           '`python3 scripts/generate-android-sounds.py && python3 scripts/audit-android-sounds.py`.',
           '', f'Result: **{"PASS" if all(ok for _, ok, _ in checks) else "FAIL"}** '
           f'({sum(ok for _, ok, _ in checks)}/{len(checks)} assertions).', '',
           'Definitions: active region = first to last sample within 30 dB of the peak; centroid = '
           'magnitude-weighted mean frequency of the whole file; dir = change of dominant frequency, '
           'second vs first equal-energy half of the active region, in semitones (positive rises). '
           'Desktop references are stereo and analysed as the L/R average.', '',
           '## Levels, ends and format', '',
           '| file | set | fmt | ms | active ms | onset ms | peak dBFS | RMS all | RMS active | crest dB | first | last | zc start/end | DC (LSB) | bytes |',
           '|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---:|']
    groups = [(CORE, 'interface', 'fx_'), (ROUND2, 'round 2', 'fx_'), (PROVIDERS, 'provider', 'fx_'),
              (PROMOTED, 'promoted', 'fx_'), (CHIMES, 'in-app chime', 'fx_'), (REFERENCES, 'desktop ref', '')]
    for names, label, prefix in groups:
        for n in names:
            r = m[n]
            fmt = f"{r['channels']}ch/{r['bits']}b/{r['rate'] // 1000}k"
            out.append(f"| {prefix}{n} | {label} | {fmt} | {r['ms']:.1f} | {r['active_ms']:.1f} | "
                       f"{r['onset_ms']:.2f} | {r['peak_db']:.1f} | {r['rms_all_db']:.1f} | {r['rms_act_db']:.1f} | "
                       f"{r['crest_db']:.1f} | {r['first']} | {r['last']} | "
                       f"{'yes' if r['zc_start'] else 'NO'}/{'yes' if r['zc_end'] else 'NO'} | "
                       f"{r['dc']:+.2f} | {r['bytes']} |")
    classes = [('interface (generated)', CORE + ROUND2), ('provider motifs', PROVIDERS),
               ('promoted auditions', PROMOTED), ('session chimes (in-app)', CHIMES)]
    out += ['', '## Loudness against the previous build', '',
            f'Effective level = file active RMS + SoundPool volume (`CueTable.volume`, slider curve `gainFor`, trim, '
            f'`ASSET_BOOST` {asset_boost:g}, volume capped at 1.0). Previous build = `{PREVIOUS_COMMIT}` at slider 100% '
            f'(its loudest setting; the user still found it too quiet). Every cue must gain at least {MIN_GAIN_DB:g} dB '
            'at the new default (50%), in plain active RMS and in A-weighted active RMS (IEC A-curve through bilinear '
            'biquads, so about right to 10 kHz). The new 100% is another 6.02 dB above the new default. dBFS.', '',
            '| class | previous 100% | new 50% (default) | new 100% | gain at default | gain at default, A-weighted | A-weighted new 100% |',
            '|---|---:|---:|---:|---:|---:|---:|']
    for label, names in classes:
        avg = lambda key: sum(loud[n][key] for n in names) / len(names)
        out.append(f"| {label} | {avg('old100'):.1f} | {avg('new50'):.1f} | {avg('new100'):.1f} | "
                   f"{avg('gain'):+.1f} dB (min {min(loud[n]['gain'] for n in names):+.1f}) | "
                   f"{avg('gain_a'):+.1f} dB (min {min(loud[n]['gain_a'] for n in names):+.1f}) | {avg('new100_a'):.1f} |")
    out += ['', 'Notification channels (system notification volume applies on top; file level only):', '',
            '| channel | desktop original (RMS / A-weighted) | new channel file (RMS / A-weighted) | gain |', '|---|---:|---:|---:|']
    for ref, chime in zip(REFERENCES, CHIMES):
        out.append(f"| {ref} | {m[ref]['rms_act_db']:.1f} / {m[ref]['arms_act_db']:.1f} | "
                   f"{m[chime]['rms_act_db']:.1f} / {m[chime]['arms_act_db']:.1f} | "
                   f"{notify[ref][0]:+.1f} / {notify[ref][1]:+.1f} dB |")
    out += ['', '| cue | trim | previous 100% | new 50% | new 100% | gain at default | A-weighted | peak dBFS | crest dB |',
            '|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    for label, names in classes:
        for n in names:
            l = loud[n]
            out.append(f"| {n} | {trims['fx_' + n]:g} | {l['old100']:.1f} | {l['new50']:.1f} | {l['new100']:.1f} | "
                       f"{l['gain']:+.1f} | {l['gain_a']:+.1f} | {m[n]['peak_db']:.1f} | {m[n]['crest_db']:.1f} |")
    out += ['', '## Round 2 cues', '',
            '| file | what it is | ms | onset ms | centroid Hz | dir (dominant) st |', '|---|---|---:|---:|---:|---:|']
    for n in ROUND2 + PROVIDERS:
        r = m[n]
        out.append(f"| fx_{n} | {NOTES[n]} | {r['ms']:.0f} | {r['onset_ms']:.2f} | {r['centroid']:.0f} | {r['dir_dom_st']:+.1f} |")
    out += ['', '## Spectrum and pitch direction', '',
            '| file | centroid Hz | dominant Hz | first half Hz | second half Hz | dir (dominant) st | dir (centroid) st |',
            '|---|---:|---:|---:|---:|---:|---:|']
    for names, _, prefix in groups:
        for n in names:
            r = m[n]
            out.append(f"| {prefix}{n} | {r['centroid']:.0f} | {r['dominant']:.0f} | {r['dom1']:.0f} | "
                       f"{r['dom2']:.0f} | {r['dir_dom_st']:+.1f} | {r['dir_cen_st']:+.1f} |")
    ref_act = {ref: m[ref]['rms_act_db'] for ref in REFERENCES}
    mean_if = sum(m[n]['rms_act_db'] for n in INTERFACE) / len(INTERFACE)
    out += ['', '## Loudness relative to the desktop cues', '',
            f'Interface set mean active RMS: {mean_if:.2f} dBFS (spread '
            f'{levels[-1] - levels[0]:.2f} dB).', '',
            '| desktop cue | RMS active | RMS all | interface is quieter by (active) |',
            '|---|---:|---:|---:|']
    for ref in REFERENCES:
        out.append(f"| {ref} | {ref_act[ref]:.1f} | {m[ref]['rms_all_db']:.1f} | {ref_act[ref] - mean_if:.1f} dB |")
    mean_p = sum(m[n]['rms_act_db'] for n in PROMOTED) / len(PROMOTED)
    prov = sorted((print_distance(m[a]['print'], m[b]['print']), a, b)
                  for i, a in enumerate(PROVIDERS) for b in PROVIDERS[i + 1:])
    out += ['', f'Level model. Every `fx_` file carries {BOOST_DB:.2f} dB of slider headroom: SoundPool volume '
            'cannot exceed 1.0, so "100% is twice as loud as 50%" has to live in the files. The app '
            'plays them at half volume by default (`CueTable.ASSET_BOOST`) and the slider\'s upper half spends the '
            f'headroom. On top of that the files are mastered {LIFT_DB:g} dB hotter than the previous build\'s '
            'loudest setting (pre-emphasis around 2.6 kHz, soft limiter at -0.5 dBFS, see '
            '`scripts/generate-android-sounds.py`). The in-app chime copies (`fx_chime_*`, also the notification '
            f'channel sounds) are the desktop chimes plus {2 * BOOST_DB + LIFT_DB:.2f} dB (checked above); the '
            'desktop originals stay untouched.', '',
            f'Why only {DESKTOP_MARGIN_DB:g} dB under the chimes: the margin is an audibility floor, not a target. '
            'The interface cues are also far shorter than the chimes (tens of milliseconds against several '
            'hundred), so at equal RMS they are perceptually quieter still. Measured on active-region RMS.', '',
            f'Promoted desktop auditions (boosted, otherwise untouched) average {mean_p:.1f} dBFS active RMS, '
            f'{mean_p - mean_if:+.1f} dB relative to the interface set.', '',
            'Scope: peak, onset, clean-end, format and clipping assertions apply to every `fx_` file; the '
            'duration, direction, centroid-order and chime-margin assertions apply to the synthesised interface '
            'cues only, because the promoted desktop cues are intentionally at desktop level and multi-click.', '',
            '## Provider cues are told apart', '',
            'Fingerprint = 12 log-spaced spectral bands (200 Hz to 9.6 kHz), the loudness envelope in 24 slices '
            'of the active region (how many hits, how spaced) and the dominant pitch in 8 slices (the contour); '
            f'distance = mean of the three RMS gaps (24 semitones = 1), minimum allowed {DISTINCT_MIN}. '
            f'Closest pairs: ' + '; '.join(f'{a[9:]} / {b[9:]} {d:.2f}' for d, a, b in prov[:4]) + '.', '',
            '## Assertions', '', '| result | assertion | measured |', '|---|---|---|']
    for label, ok, detail in checks:
        out.append(f"| {'pass' if ok else '**FAIL**'} | {label} | {detail} |")
    REPORT.write_text('\n'.join(out) + '\n')

    failed = [c for c in checks if not c[1]]
    for label, ok, detail in checks:
        if not ok:
            print(f'FAIL: {label} [{detail}]')
    print(f'{len(checks) - len(failed)}/{len(checks)} assertions passed; wrote {REPORT.relative_to(ROOT)}')
    for n in INTERFACE + PROMOTED + CHIMES + REFERENCES:
        r = m[n]
        print(f"{n:>13}: {r['ms']:6.1f} ms  peak {r['peak_db']:6.1f}  rms {r['rms_act_db']:6.1f}  "
              f"cent {r['centroid']:6.0f}  dom {r['dominant']:6.0f}  dir {r['dir_dom_st']:+5.1f}/{r['dir_cen_st']:+5.1f}")
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
