#!/usr/bin/env python3
"""Zeron Android interface sounds: tiny, near-subliminal, mono, deterministic.

Standard-library-only synthesis in the same "rounded pressure pulse" language
as the desktop notification family (scripts/generate-notification-sounds.py,
scripts/generate-sound-auditions.py): a smooth Ricker-style pressure pulse for
the physical contact, plus a short, damped, harmonic note for the meaning. No
samples, no random without a fixed seed (the one noise swish uses a private
xorshift generator with a constant seed, so output is bit-identical per run).

Output: apps/android/app/src/main/res/raw/fx_<name>.wav, mono, 16-bit PCM,
48 kHz. With --sync-promoted (default) five desktop auditions are also copied
into the same folder (stereo -> mono, level untouched), see PROMOTED below.

TONAL FAMILY
    Everything is built from the C-major pentatonic scale (C D E G A), which
    cannot clash with itself and also matches the desktop cues (C4/D4/E4/G4,
    F4 only in the Appshot). The detent's fixed reference pitch is A5 = 880 Hz,
    a member of the same scale: the app resamples it with SoundPool playback
    rates 0.5..2.0 (440..1760 Hz) along a pentatonic ladder, so it is a pure,
    narrowband sine (no pulse, no harmonics) and pitch-shifting stays natural.

MEANING IS PITCH DIRECTION
    rising = opening / turning on / adding / refreshing
    falling = closing / turning off / removing / failing
    Rising cues are written with a swell (the energy arrives late, at the
    higher pitch) so that they are also brighter; falling cues decay from their
    high point. Lower and heavier means more destructive (Delete is the lowest).

LOUDNESS
    Three layers, all measured by scripts/audit-android-sounds.py:
      * Slider headroom (BOOST_DB = 6.02 dB). SoundPool volume cannot exceed 1.0,
        so "100% is twice as loud as 50%" has to live in the files: the app
        plays a cue at gain * trim / ASSET_BOOST (CueTable.volume), the default
        slider (gain 1) at half volume, 100% (gain 2) at full volume.
      * The "twice as loud again" lift (DEFAULT_LIFT_DB = 6.0 dB). Every file
        is mastered so that the DEFAULT slider position plays 6 dB above where
        the previous build's 100% played, and its 100% a further 6.02 dB above
        that: interface cues sit at TARGET_RMS_DBFS = -38 + 6.0 + 6.02 = -25.98
        dBFS active RMS; the promoted auditions and the in-app chimes are
        their desktop level + 18.04 dB (the previous build already added 6.02).
        The previous interface peaks were -24 to -30 dBFS, so this fits below
        full scale without compressing; the chimes (crest factor 22 dB) are
        the ones that touch the ceiling.
      * Mastering (master()): a gentle +3.5 dB peaking boost at 2.6 kHz (Q 0.8),
        the band where phone speakers are efficient and hearing is most
        sensitive (everything but NO_EMPHASIS: the pure 880 Hz Detent, which SoundPool
        transposes, and the Zip and Surge, whose direction would suffer), then a soft-knee tanh limiter
        (knee -4 dBFS, ceiling PEAK_LIMIT_DBFS = -0.5 dBFS, so no sample can
        clip), then the gain is iterated until the active RMS after limiting
        equals the target. Peaks above the knee are rounded, not clipped.
    The desktop originals stay untouched (crates/ui/assets); the notification
    channels play the mastered fx_chime_* copies.

LEADING SILENCE
    Latency matters more than tidy tails: every file is trimmed so its onset
    (first sample >= -40 dB re peak) lands LEAD_IN_MS = 0.25 ms in, with a
    0.25 ms raised-sine fade from exact zero in front of it (click-free). The
    audit asserts the onset is within 1 ms for every fx_ file.

CLICK-FREE
    A raised-sine master fade-in and fade-out forces the first and last sample
    to exactly 0 and a zero slope at both ends.

ROUND 2 (thinking power, fast mode, providers)
    Surge        rising power swell with shimmer and a bright bloom (~520 ms)
    Zip          fast falling airy streak (~100 ms)
    Rebound      soft elastic boing (~120 ms)
    FastOn       electric crackle and a short bright zap (~200 ms), quiet
    FastOff      the charge draining: soft falling down-tick (~100 ms)
    Provider*    one distinct motif each, <= 120 ms (see PROVIDERS below)

HAPTIC PAIRING (Feedback.kt Haptic -> Cue)
    Tap          <- Tick / Select   shortest, most neutral
    Select       <- Select          a little warmer/rounder than Tap
    ToggleOn     <- ToggleOn        rising fifth-ish scoop, bright
    ToggleOff    <- ToggleOff       falling, darker, fewer harmonics
    Open         <- Select          rising swell
    Close        <- Select          falling, lower than Open
    Detent       <- Tick            880 Hz pure tick, resampled per step
    Star         <- Pop             bright upward pop with a sparkle
    Unstar       <- Pop             soft downward counterpart
    Pin          <- Pop             firm short thock with a small rise
    Archive      <- Select          soft falling swish + thud
    Delete       <- Heavy           lowest, weightiest falling thud
    Copy         <- Confirm         quick tiny rising double tick
    Error        <- Error           restrained low two-note descent
    Refresh      <- Select          soft two-step rise
"""
import argparse
import hashlib
import math
import struct
import wave
from pathlib import Path

RATE = 48000
TAU = 2 * math.pi
ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'apps/android/app/src/main/res/raw'
AUDITIONS = ROOT / 'docs/sound-design/auditions'
DESKTOP_SOUNDS = ROOT / 'crates/ui/assets/sounds'

# Active-region definition shared with scripts/audit-android-sounds.py.
ACTIVE_FLOOR_DB = -30.0
BOOST_DB = 20 * math.log10(2.0)  # CueTable.ASSET_BOOST: the slider's upper half
PREVIOUS_RMS_DBFS = -38.0        # interface cues of the previous build (its slider 100% played them at this level)
DEFAULT_LIFT_DB = 0.0            # the default slider position vs the previous build's 100%
TARGET_RMS_DBFS = PREVIOUS_RMS_DBFS + DEFAULT_LIFT_DB + BOOST_DB
PEAK_LIMIT_DBFS = -0.5           # hard ceiling of every sample after the limiter
KNEE_DBFS = -4.0                 # the soft limiter starts bending here
EQ_HZ, EQ_Q, EQ_GAIN_DB = 2600.0, 0.8, 3.5  # pre-emphasis where phone speakers and ears are most efficient
# Cues left flat: the Detent is transposed by SoundPool and must stay a pure 880 Hz sine; the Zip is already all
# 1.8-7.5 kHz and its meaning is the fall (the boost would pull its late half up); the Surge's meaning is the climb.
NO_EMPHASIS = {'fx_detent', 'fx_zip', 'fx_surge'}
ONSET_DB = -40.0       # onset = first sample this far under the peak
ONSET_MARGIN_DB = 0.5  # trimming uses a threshold this much higher than the audit's
LEAD_IN_MS = 0.25      # trimmed files start this long before the onset
KEEP_ALIVE_MS = 100

# Pentatonic (C D E G A) pitch table, Hz.
C3, G3 = 130.81, 196.00
E4, C4, G4 = 329.63, 261.63, 392.00
D4, A4 = 293.66, 440.00
C5, D5, E5, G5, A5 = 523.25, 587.33, 659.26, 783.99, 880.00
C6, D6, E6, G6 = 1046.50, 1174.66, 1318.51, 1567.98
A3, D3 = 220.00, 146.83
A6, C7, D7, E7, G7 = 1760.00, 2093.00, 2349.32, 2637.02, 3135.96

# name -> desktop session chime (in-app copy: mono, trimmed, mastered; the originals stay for notifications).
CHIMES = {
    'fx_chime_done': 'done.wav',
    'fx_chime_request': 'request.wav',
    'fx_chime_attention': 'attention.wav',
}

# name -> desktop audition source (copied mono, trimmed, mastered; no re-synthesis).
PROMOTED = {
    'fx_send': '01-send.wav',
    'fx_queued': '02-queued.wav',
    'fx_upload_ready': '03-upload-ready.wav',
    'fx_reconnected': '09-reconnected.wav',
    'fx_undo': '10-undo.wav',
}


# ---------------------------------------------------------------- primitives

def samples(ms):
    return round(RATE * ms / 1000.0)


def add_pulse(buf, centre_ms, width_ms, amp):
    """Rounded pressure pulse, (1 - 2x^2) exp(-x^2): zero net area, no DC."""
    centre, width = centre_ms / 1000.0, width_ms / 1000.0
    first = max(0, int((centre - 5 * width) * RATE))
    last = min(len(buf), int((centre + 5 * width) * RATE) + 1)
    for i in range(first, last):
        x = (i / RATE - centre) / width
        buf[i] += amp * (1 - 2 * x * x) * math.exp(-x * x)


def add_note(buf, start_ms, f0, f1, amp, attack_ms, decay_ms,
             partials=((1, 1.0, 1.0),), glide_ms=0.0):
    """Damped harmonic note whose pitch eases from f0 to f1.

    partials: (ratio, level, damping) like the desktop COLORS; damping scales
    the decay time of that partial. The glide is a smoothstep over glide_ms in
    log-frequency; phase is integrated per sample so there are no jumps.
    """
    start = int(start_ms / 1000.0 * RATE)
    attack = attack_ms / 1000.0
    glide = glide_ms / 1000.0
    decay = decay_ms / 1000.0
    phase = 0.0
    log0, log1 = math.log(f0), math.log(f1)
    for i in range(start, len(buf)):
        u = (i - start) / RATE
        if u > decay * 9:
            break
        if glide > 0:
            s = min(u / glide, 1.0)
            s = s * s * (3 - 2 * s)
            freq = math.exp(log0 + (log1 - log0) * s)
        else:
            freq = f1
        phase += TAU * freq / RATE
        rise = 0.5 - 0.5 * math.cos(math.pi * u / attack) if u < attack else 1.0
        value = 0.0
        for ratio, level, damping in partials:
            value += level * math.exp(-u / (decay * damping)) * math.sin(ratio * phase)
        buf[i] += amp * rise * value


def add_swish(buf, start_ms, length_ms, amp, cutoff_from, cutoff_to, seed=0x2545F491):
    """Soft falling air: xorshift noise through a swept two-stage low-pass."""
    state = seed
    start, length = samples(start_ms), samples(length_ms)
    y1 = y2 = 0.0
    for n in range(length):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        noise = state / 2147483648.0 - 1.0
        s = n / length
        cutoff = cutoff_from * (cutoff_to / cutoff_from) ** s
        a = 1 - math.exp(-TAU * cutoff / RATE)
        y1 += a * (noise - y1)
        y2 += a * (y1 - y2)
        bell = math.sin(math.pi * s) ** 2
        if start + n < len(buf):
            buf[start + n] += amp * bell * y2


def add_streak(buf, start_ms, length_ms, amp, cutoff_from, cutoff_to, peak_at=0.25, seed=0x9E3779B9):
    """Airy band of noise: a swept two-stage low-pass minus a lower one (so no rumble), fast attack, soft tail."""
    state = seed
    start, length = samples(start_ms), samples(length_ms)
    y1 = y2 = l1 = l2 = 0.0
    for n in range(length):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        noise = state / 2147483648.0 - 1.0
        s = n / length
        cutoff = cutoff_from * (cutoff_to / cutoff_from) ** s
        a = 1 - math.exp(-TAU * cutoff / RATE)
        b = 1 - math.exp(-TAU * cutoff * 0.22 / RATE)
        y1 += a * (noise - y1)
        y2 += a * (y1 - y2)
        l1 += b * (noise - l1)
        l2 += b * (l1 - l2)
        if s < peak_at:
            env = math.sin(math.pi / 2 * s / peak_at) ** 2
        else:
            env = math.cos(math.pi / 2 * (s - peak_at) / (1 - peak_at)) ** 2
        if start + n < len(buf):
            buf[start + n] += amp * env * (y2 - l2)


def add_boing(buf, start_ms, f0, amp, decay_ms, dev=0.55, wobble_hz=24.0, wobble_decay_ms=42.0,
              partials=((1, 1.0, 1.0),)):
    """Elastic note: the pitch overshoots high and wobbles around f0 while the level decays (a spring)."""
    start = samples(start_ms)
    phase = 0.0
    for i in range(start, len(buf)):
        u = (i - start) / RATE
        if u > decay_ms / 1000.0 * 9:
            break
        freq = f0 * (1 + dev * math.exp(-u * 1000 / wobble_decay_ms) * math.cos(TAU * wobble_hz * u))
        phase += TAU * freq / RATE
        rise = 0.5 - 0.5 * math.cos(math.pi * u / 0.0015) if u < 0.0015 else 1.0
        value = 0.0
        for ratio, level, damping in partials:
            value += level * math.exp(-u * 1000 / (decay_ms * damping)) * math.sin(ratio * phase)
        buf[i] += amp * rise * value


def add_vibrato_note(buf, start_ms, f0, f1, amp, attack_ms, decay_ms, vib_hz, vib_depth, glide_ms,
                     partials=((1, 1.0, 1.0),)):
    """add_note with a slow pitch wobble (+-vib_depth, a fraction of the frequency) for floating pads."""
    start = int(start_ms / 1000.0 * RATE)
    attack, decay, glide = attack_ms / 1000.0, decay_ms / 1000.0, glide_ms / 1000.0
    log0, log1 = math.log(f0), math.log(f1)
    phase = 0.0
    for i in range(start, len(buf)):
        u = (i - start) / RATE
        if u > decay * 9:
            break
        s = min(u / glide, 1.0) if glide > 0 else 1.0
        s = s * s * (3 - 2 * s)
        freq = math.exp(log0 + (log1 - log0) * s) * (1 + vib_depth * math.sin(TAU * vib_hz * u))
        phase += TAU * freq / RATE
        rise = 0.5 - 0.5 * math.cos(math.pi * u / attack) if u < attack else 1.0
        value = 0.0
        for ratio, level, damping in partials:
            value += level * math.exp(-u / (decay * damping)) * math.sin(ratio * phase)
        buf[i] += amp * rise * value


def master_fade(buf, fade_in_ms, fade_out_ms):
    count = len(buf)
    fi, fo = samples(fade_in_ms), samples(fade_out_ms)
    for i in range(count):
        gain = 1.0
        if i < fi:
            gain = math.sin(math.pi / 2 * i / fi) ** 2
        tail = count - 1 - i
        if tail < fo:
            gain *= math.sin(math.pi / 2 * tail / fo) ** 2
        buf[i] *= gain
    buf[0] = 0.0
    buf[-1] = 0.0


def active_rms(buf):
    peak = max(abs(v) for v in buf)
    floor = peak * 10 ** (ACTIVE_FLOOR_DB / 20)
    hits = [i for i, v in enumerate(buf) if abs(v) >= floor]
    span = buf[hits[0]:hits[-1] + 1]
    return math.sqrt(sum(v * v for v in span) / len(span))


def trim_lead(x):
    """Cut the leading silence: the onset (first sample >= ONSET_DB re peak) ends up LEAD_IN_MS in.

    A raised-sine fade from exact zero over the kept lead-in keeps the start click-free. Works on floats or ints.
    """
    peak = max(abs(v) for v in x)
    # Half a dB above the audit's -40 dB, so rounding to 16 bits (or a slowly swelling sine whose crest just
    # misses the line) can never push the audit's onset behind this one.
    threshold = peak * 10 ** ((ONSET_DB + ONSET_MARGIN_DB) / 20)
    onset = next(i for i, v in enumerate(x) if abs(v) >= threshold)
    start = max(0, onset - max(2, samples(LEAD_IN_MS)))
    out = list(x[start:])
    ramp = onset - start
    for k in range(ramp):
        out[k] = out[k] * math.sin(math.pi / 2 * k / ramp) ** 2
    out[0] = 0 * out[0]
    return out


def peaking_eq(x, f0=EQ_HZ, q=EQ_Q, gain_db=EQ_GAIN_DB):
    """RBJ peaking biquad (direct form I); causal, so the first sample of a signal that starts at 0 stays 0."""
    a_lin = 10 ** (gain_db / 40)
    w0 = TAU * f0 / RATE
    alpha = math.sin(w0) / (2 * q)
    cw = math.cos(w0)
    b0, b1, b2 = 1 + alpha * a_lin, -2 * cw, 1 - alpha * a_lin
    a0, a1, a2 = 1 + alpha / a_lin, -2 * cw, 1 - alpha / a_lin
    b0, b1, b2, a1, a2 = b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0
    out = []
    x1 = x2 = y1 = y2 = 0.0
    for v in x:
        y = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, v, y1, y
        out.append(y)
    return out


def soft_limit(x):
    """Linear below KNEE_DBFS, then tanh into PEAK_LIMIT_DBFS: continuous in value and slope, never above the ceiling."""
    knee, ceil = 10 ** (KNEE_DBFS / 20), 10 ** (PEAK_LIMIT_DBFS / 20)
    out = []
    for v in x:
        a = abs(v)
        if a > knee:
            a = knee + (ceil - knee) * math.tanh((a - knee) / (ceil - knee))
            v = math.copysign(a, v)
        out.append(v)
    return out


def master(x, target_rms_dbfs, eq=True):
    """Pre-emphasis, then gain and soft limiting solved together so the active RMS after limiting hits the target.

    x is in full-scale units (1.0 = 32768) and already faded to zero at both ends. Returns floats."""
    if eq:
        x = peaking_eq(x)
    target = 10 ** (target_rms_dbfs / 20)
    gain = target / active_rms(x)
    for _ in range(60):
        y = soft_limit([v * gain for v in x])
        err = target / active_rms(y)
        gain *= err
        if abs(20 * math.log10(err)) < 0.005:
            break
    y = soft_limit([v * gain for v in x])
    assert abs(20 * math.log10(active_rms(y) / target)) < 0.05, 'limiter could not reach the target level'
    return y


def quantize(y, name):
    pcm = [max(-32768, min(32767, round(v * 32768))) for v in y]
    pcm[0] = pcm[-1] = 0
    peak_db = 20 * math.log10(max(abs(v) for v in pcm) / 32768)
    assert peak_db <= PEAK_LIMIT_DBFS + 0.01, f'{name}: peak {peak_db:.2f} dBFS'
    return pcm


# ---------------------------------------------------------------------- cues
# Each cue: (duration_ms, fade_in_ms, fade_out_ms, builder). Levels inside a
# builder are relative; the whole cue is RMS-normalised afterwards.

def cue_tap(b):
    # Shortest, most neutral: one narrow pulse and a tiny G5 body.
    add_pulse(b, 3.5, 0.40, 1.0)
    add_note(b, 2.5, G5, G5, 0.45, 1.0, 5.0, ((1, 1, 1), (2, .10, .6)))


def cue_select(b):
    # Warmer than Tap: wider (lower) pulse, E5 body with a round 2nd partial.
    add_pulse(b, 4.0, 0.62, 1.0)
    add_note(b, 3.0, E5, E5, 0.70, 1.5, 9.0, ((1, 1, 1), (2, .22, .6)))


def cue_toggle_on(b):
    # Bright rising scoop E5 -> C6, energy arriving late, clear harmonics.
    add_pulse(b, 4.0, 0.34, 0.35)
    add_note(b, 3.0, E5, C6, 1.0, 14.0, 20.0, ((1, 1, 1), (2, .20, .8), (3, .07, .6)), glide_ms=38)


def cue_toggle_off(b):
    # Darker falling counterpart G5 -> D5, fewer harmonics, soft pulse.
    add_pulse(b, 4.0, 0.55, 0.45)
    add_note(b, 3.0, G5, D5, 1.0, 2.0, 15.0, ((1, 1, 1), (2, .05, .6)), glide_ms=34)


def cue_open(b):
    # Airy rising swell D5 -> A5.
    add_note(b, 2.0, D5, A5, 1.0, 34.0, 26.0, ((1, 1, 1), (2, .12, .7)), glide_ms=70)


def cue_close(b):
    # Lower, falling counterpart E5 -> A4 with a soft close-contact pulse.
    add_pulse(b, 4.0, 0.80, 0.25)
    add_note(b, 3.0, E5, A4, 1.0, 4.0, 28.0, ((1, 1, 1), (2, .12, .7)), glide_ms=60)


def cue_detent(b):
    # 880 Hz (A5) reference. Pure sine, 1.5 ms swell, no pulse, no harmonics:
    # narrowband so SoundPool rate 0.5..2.0 transposes it without artefacts.
    add_note(b, 1.0, A5, A5, 1.0, 1.5, 7.0)


def cue_star(b):
    # Bright upward pop: A5 -> E6 scoop, then a G6 sparkle on top.
    add_pulse(b, 4.0, 0.30, 0.30)
    add_note(b, 3.0, A5, E6, 0.8, 12.0, 14.0, ((1, 1, 1), (2, .10, .6)), glide_ms=30)
    add_note(b, 36.0, G6, G6, 1.0, 3.0, 16.0, ((1, 1, 1), (2, .04, .5)))


def cue_unstar(b):
    # Soft downward, no pulse: C6 -> G5.
    add_note(b, 2.0, C6, G5, 1.0, 5.0, 20.0, ((1, 1, 1),), glide_ms=45)


def cue_pin(b):
    # Firm short thock: low rounded pulse and a body that rises E4 -> G4.
    add_pulse(b, 3.5, 0.80, 1.0)
    add_note(b, 2.5, E4, G4, 1.1, 1.5, 14.0, ((1, 1, 1), (2, .55, .7), (3, .18, .5)), glide_ms=22)


def cue_archive(b):
    # Soft falling swish, then a muted thud E4 -> A3.
    add_swish(b, 2.0, 68.0, 0.55, 2400.0, 700.0)
    add_pulse(b, 34.0, 1.00, 0.55)
    add_note(b, 31.0, E4, A3, 1.0, 3.0, 26.0, ((1, 1, 1), (2, .28, .6)), glide_ms=40)


def cue_delete(b):
    # Weighty falling thud G3 -> C3; upper partials keep it audible on a
    # phone speaker while the energy centre stays the lowest of the set.
    add_pulse(b, 5.0, 1.40, 1.0)
    add_note(b, 3.0, G3, C3, 1.4, 3.0, 38.0,
             ((1, 1, 1), (2, .55, .7), (3, .26, .55), (4, .10, .4)), glide_ms=60)


def cue_copy(b):
    # Quick tiny double tick, the second a step up (G5 then C6).
    add_pulse(b, 3.5, 0.30, 0.8)
    add_note(b, 2.5, G5, G5, 0.35, 0.8, 4.0)
    add_pulse(b, 31.5, 0.27, 1.0)
    add_note(b, 30.0, C6, C6, 0.45, 0.8, 4.0)


def cue_error(b):
    # Restrained low two-note descent E4 -> C4, soft attacks, mild 2nd partial.
    add_pulse(b, 4.0, 1.0, 0.25)
    add_note(b, 2.0, E4, E4, 1.0, 6.0, 28.0, ((1, 1, 1), (2, .30, .7), (3, .08, .5)))
    add_pulse(b, 92.0, 1.1, 0.22)
    add_note(b, 88.0, C4, C4, 0.9, 6.0, 34.0, ((1, 1, 1), (2, .30, .7), (3, .06, .5)))


def cue_refresh(b):
    # Soft two-step rise: C5 then E5 (a gentle major third).
    add_pulse(b, 4.0, 0.60, 0.20)
    add_note(b, 2.0, C5, C5, 0.7, 4.0, 18.0, ((1, 1, 1), (2, .10, .6)))
    add_pulse(b, 54.0, 0.55, 0.20)
    add_note(b, 50.0, E5, E5, 1.0, 4.0, 22.0, ((1, 1, 1), (2, .10, .6)))


# ------------------------------------------------------------------- round 2
# Thinking power, fast mode and the provider rail. Same language as above.

def cue_surge(b):
    # The top thinking power: a swell that climbs two octaves and arrives late, two detuned voices for shimmer,
    # a pentatonic run of sparkles climbing with it, and a bright C-major bloom at the top.
    add_note(b, 2.0, G4, C6, 0.80, 300.0, 230.0, ((1, 1, 1), (2, .30, .8), (3, .12, .6)), glide_ms=360)
    add_note(b, 2.0, G4 * 1.006, C6 * 1.006, 0.55, 320.0, 230.0, ((1, 1, 1), (2, .22, .8)), glide_ms=360)
    add_note(b, 120.0, G3, C5, 0.35, 220.0, 200.0, ((1, 1, 1), (2, .5, .7)), glide_ms=250)
    for t, f, a in [(150, E5, .30), (195, G5, .34), (235, A5, .38), (270, C6, .42), (300, D6, .46), (330, E6, .50)]:
        add_note(b, t, f, f, a, 1.5, 20.0, ((1, 1, 1), (2, .14, .5)))
    for f, a in [(C6, .90), (E6, .80), (G6, .70), (C7, .45)]:
        add_note(b, 352.0, f, f, a, 3.0, 85.0, ((1, 1, 1), (2, .10, .6)))
    add_pulse(b, 354.0, 0.50, 0.30)


def cue_zip(b):
    # The lightest power: a fast airy streak that falls away, with a thin pure zing riding it (C6 <- G6).
    add_streak(b, 1.0, 74.0, 1.0, 7500.0, 1800.0, peak_at=0.22)
    add_note(b, 1.0, G6, C6, 0.55, 2.0, 15.0, ((1, 1, 1),), glide_ms=48)


def cue_rebound(b):
    # The thumb snaps back: a soft elastic boing, the pitch overshooting and settling, a low pad under the hit.
    add_pulse(b, 3.0, 0.9, 0.55)
    add_boing(b, 2.0, G4, 1.0, 36.0, dev=0.55, wobble_hz=26.0, wobble_decay_ms=40.0, partials=((1, 1, 1), (2, .30, .6)))


def cue_fast_on(b):
    # Fast mode on: irregular electric crackle, then a short bright zap and a small bloom on top.
    for t, w, a in [(2.0, .14, .50), (8.5, .18, -.80), (13.0, .12, .40), (27.0, .16, .90), (33.5, .12, -.50),
                    (49.0, .15, .70), (55.0, .12, -.45), (62.0, .18, .85)]:
        add_pulse(b, t, w, a)
    add_note(b, 66.0, G6, G7, 1.0, 1.0, 20.0, ((1, 1, 1), (2, .16, .6)), glide_ms=24)
    add_note(b, 66.0, C6, C7, .35, 1.0, 24.0, ((1, 1, 1),), glide_ms=26)
    add_note(b, 96.0, C7, C7, .45, 2.0, 34.0, ((1, 1, 1), (2, .10, .5)))
    add_note(b, 99.0, G6, G6, .25, 2.0, 40.0, ((1, 1, 1),))


def cue_fast_off(b):
    # Fast mode off: the charge draining. A soft tick, then a short falling sigh G6 -> C5.
    add_pulse(b, 3.0, 0.50, 0.30)
    add_note(b, 2.0, G6, C5, 1.0, 1.5, 16.0, ((1, 1, 1), (2, .06, .5)), glide_ms=78)


def cue_provider_claude(b):
    # Warm two-note rising pair, E5 then A5, rounded harmonics and a soft attack.
    for t, f, a in [(2.0, E5, 0.85), (50.0, A5, 1.0)]:
        add_note(b, t, f, f, a, 7.0, 24.0, ((1, 1, 1), (2, .38, .8), (3, .14, .6), (4, .05, .5)))


def cue_provider_codex(b):
    # Crisp bracket-like double tick: two identical hollow clicks, D6, a beat apart.
    for t in (2.5, 44.0):
        add_pulse(b, t, 0.20, 1.0)
        add_note(b, t - 1.0, D6, D6, 0.55, 0.5, 5.0, ((1, 1, 1), (3, .33, .8), (5, .18, .6)))


def cue_provider_cursor(b):
    # One glassy blip with a slight upward bend: inharmonic partials on G6 -> A6.
    add_note(b, 2.0, G6, A6, 1.0, 1.0, 30.0, ((1, 1, 1), (2.76, .34, .5), (5.40, .14, .3)), glide_ms=26)


def cue_provider_devin(b):
    # Soft pad-like minor third: A4 and C5 together, slow bloom, nothing sharp in it.
    add_note(b, 2.0, A4, A4, 0.9, 22.0, 52.0, ((1, 1, 1), (2, .14, .6)))
    add_note(b, 10.0, C5, C5, 1.0, 22.0, 52.0, ((1, 1, 1), (2, .12, .6)))
    add_note(b, 2.0, A3, A3, 0.25, 26.0, 50.0)


def cue_provider_grok(b):
    # Bright quick fifth, C5 then G5, then a little sparkle on top.
    add_note(b, 2.0, C5, C5, 0.9, 2.0, 13.0, ((1, 1, 1), (2, .45, .8), (3, .26, .6), (4, .10, .5)))
    add_note(b, 20.0, G5, G5, 1.0, 2.0, 18.0, ((1, 1, 1), (2, .45, .8), (3, .26, .6), (4, .10, .5)))
    for t, f, a in [(52.0, A6, .38), (64.0, C7, .32), (76.0, E7, .22)]:
        add_note(b, t, f, f, a, 1.0, 9.0)


def cue_provider_hermes(b):
    # Fast flutter up: seven wing-beat steps of the scale, nine milliseconds apart, with a breath of air under them.
    add_streak(b, 2.0, 66.0, 0.22, 3000.0, 6500.0, peak_at=0.6, seed=0x1B873593)
    for k, f in enumerate([C5, D5, E5, G5, A5, C6, D6]):
        add_note(b, 2.0 + 9.0 * k, f, f, 0.50 + 0.08 * k, 1.0, 5.0, ((1, 1, 1), (2, .16, .6)))


def cue_provider_pi(b):
    # Three-note tiny arpeggio on the digits 3, 1, 4 of pi: E5, C5, G5 (third, root, fourth degree).
    for t, f, a in [(2.0, E5, .85), (36.0, C5, .85), (70.0, G5, 1.0)]:
        add_note(b, t, f, f, a, 1.5, 15.0, ((1, 1, 1), (2, .20, .6), (3, .07, .5)))


def cue_provider_opencode(b):
    # Open, hollow tone: an open fifth (D5 + A5) in odd harmonics only, like a wooden pipe.
    add_note(b, 2.0, D5, D5, 1.0, 12.0, 38.0, ((1, 1, 1), (3, .42, .8), (5, .18, .6)))
    add_note(b, 2.0, A5, A5, 0.55, 14.0, 34.0, ((1, 1, 1), (3, .36, .8), (5, .12, .6)))


def cue_provider_antigravity(b):
    # Floaty upward glide with a slow wobble, and a quieter echo of itself a little higher.
    add_vibrato_note(b, 2.0, C5, C6, 1.0, 38.0, 55.0, 17.0, 0.006, 80, ((1, 1, 1), (2, .10, .6)))
    add_vibrato_note(b, 34.0, G5, G6, 0.45, 30.0, 40.0, 19.0, 0.006, 70, ((1, 1, 1),))


def cue_provider_favorites(b):
    # A twinkle: four little bell tones, high and falling in loudness, the last one a fifth up.
    bell = ((1, 1, 1), (2.0, .22, .5), (3.0, .09, .3))
    for t, f, a in [(2.0, C7, 1.0), (27.0, G6, .70), (50.0, C7, .55), (74.0, E7, .38)]:
        add_note(b, t, f, f, a, 0.8, 14.0, bell)


def cue_provider_other(b):
    # Neutral soft pop: a broad rounded pulse with a low A4 body, no pitch story at all.
    add_pulse(b, 3.5, 1.00, 1.0)
    add_note(b, 2.5, A4, A4, 0.60, 1.5, 9.0, ((1, 1, 1), (2, .12, .6)))


CUES = {
    'fx_tap': (34, 2.0, 12, cue_tap),
    'fx_select': (58, 2.5, 20, cue_select),
    'fx_toggle_on': (72, 2.5, 22, cue_toggle_on),
    'fx_toggle_off': (66, 2.5, 24, cue_toggle_off),
    'fx_open': (108, 4.0, 32, cue_open),
    'fx_close': (100, 4.0, 34, cue_close),
    'fx_detent': (38, 1.5, 14, cue_detent),
    'fx_star': (108, 2.5, 36, cue_star),
    'fx_unstar': (92, 2.5, 34, cue_unstar),
    'fx_pin': (84, 2.0, 28, cue_pin),
    'fx_archive': (112, 3.0, 36, cue_archive),
    'fx_delete': (124, 3.0, 44, cue_delete),
    'fx_copy': (76, 2.0, 22, cue_copy),
    'fx_error': (176, 4.0, 52, cue_error),
    'fx_refresh': (118, 3.0, 40, cue_refresh),
    'fx_surge': (520, 3.0, 150, cue_surge),
    'fx_zip': (86, 1.5, 28, cue_zip),
    'fx_rebound': (122, 2.0, 40, cue_rebound),
    'fx_fast_on': (196, 1.0, 70, cue_fast_on),
    'fx_fast_off': (102, 2.0, 40, cue_fast_off),
    'fx_provider_claude': (118, 2.0, 42, cue_provider_claude),
    'fx_provider_codex': (84, 1.0, 30, cue_provider_codex),
    'fx_provider_cursor': (110, 1.5, 40, cue_provider_cursor),
    'fx_provider_devin': (120, 3.0, 46, cue_provider_devin),
    'fx_provider_grok': (118, 2.0, 40, cue_provider_grok),
    'fx_provider_hermes': (96, 1.5, 34, cue_provider_hermes),
    'fx_provider_pi': (112, 2.0, 38, cue_provider_pi),
    'fx_provider_opencode': (120, 3.0, 46, cue_provider_opencode),
    'fx_provider_antigravity': (120, 3.0, 46, cue_provider_antigravity),
    'fx_provider_favorites': (118, 1.5, 40, cue_provider_favorites),
    'fx_provider_other': (60, 1.5, 22, cue_provider_other),
}


def render(name, target_rms_dbfs):
    duration_ms, fade_in, fade_out, builder = CUES[name]
    buf = [0.0] * samples(duration_ms)
    builder(buf)
    if name not in NO_EMPHASIS:
        buf = peaking_eq(buf)
    master_fade(buf, fade_in, fade_out)
    return quantize(trim_lead(master(buf, target_rms_dbfs, eq=False)), name)


def write_mono(path, pcm):
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), 'wb') as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes(struct.pack(f'<{len(pcm)}h', *pcm))


def read_mono(path):
    with wave.open(str(path), 'rb') as wav:
        channels, width, rate = wav.getnchannels(), wav.getsampwidth(), wav.getframerate()
        frames = wav.readframes(wav.getnframes())
    assert width == 2 and rate == RATE, f'{path.name}: unexpected format {width * 8} bit {rate} Hz'
    count = len(frames) // (2 * channels)
    values = struct.unpack(f'<{count * channels}h', frames)
    if channels == 1:
        return list(values), channels
    return [(sum(values[i * channels:(i + 1) * channels]) + channels // 2) // channels
            for i in range(count)], channels


def mastered(pcm, name):
    """A desktop sound as an in-app one: trimmed, short end fade, pre-emphasis and limiter, then
    LIFT (BOOST_DB for the slider + DEFAULT_LIFT_DB) over where the previous build played it."""
    pcm = trim_lead([v / 32768 for v in pcm])
    tail = samples(2.0)
    for k in range(tail):
        pcm[-1 - k] *= math.sin(math.pi / 2 * k / tail) ** 2
    pcm[-1] = 0.0
    # The previous build's file was the desktop sound + BOOST_DB; this one is that + the slider headroom again + the lift.
    target = 20 * math.log10(active_rms(pcm)) + 2 * BOOST_DB + DEFAULT_LIFT_DB
    return quantize(master(pcm, target), name)


def promote(out_dir):
    """Copy desktop auditions and session chimes as mono 16-bit 48 kHz, mastered (also the notification sounds)."""
    for sources, folder in ((PROMOTED, AUDITIONS), (CHIMES, DESKTOP_SOUNDS)):
        for name, source in sources.items():
            pcm, channels = read_mono(folder / source)
            before = len(pcm)
            out = mastered(pcm, name)
            write_mono(out_dir / f'{name}.wav', out)
            peak = 20 * math.log10(max(abs(v) for v in out) / 32768)
            print(f'{name}.wav: from {source} ({channels} ch -> mono, {before / RATE * 1000:.0f} ms -> '
                  f'{len(out) / RATE * 1000:.0f} ms, peak {peak:.1f} dBFS)')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--out', type=Path, default=RAW, help='Output directory (default: res/raw).')
    parser.add_argument('--rms-dbfs', type=float, default=TARGET_RMS_DBFS,
                        help='Active-region RMS shared by all interface cues.')
    parser.add_argument('--sync-promoted', action=argparse.BooleanOptionalAction, default=True,
                        help='Also copy the promoted desktop auditions (default on).')
    args = parser.parse_args()
    for name in CUES:
        pcm = render(name, args.rms_dbfs)
        path = args.out / f'{name}.wav'
        write_mono(path, pcm)
        peak = 20 * math.log10(max(abs(v) for v in pcm) / 32768)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()[:12]
        print(f'{path.name}: {len(pcm) / RATE * 1000:.0f} ms, peak {peak:.1f} dBFS, sha {digest}')
    write_mono(args.out / 'silence_keepalive.wav', [0] * samples(KEEP_ALIVE_MS))  # SoundBank's keep-alive loop
    if args.sync_promoted:
        promote(args.out)


if __name__ == '__main__':
    main()
