# Dictation models: attribution and runtime

## Parakeet Ultra

Parakeet Ultra weights © Moondream (post-trained from NVIDIA Parakeet TDT 0.6B v3, © NVIDIA), licensed under
Creative Commons Attribution 4.0 International: https://creativecommons.org/licenses/by/4.0/

Original model: https://huggingface.co/moondream/parakeet-ultra at commit `73175eb7aeb0d82f1e2a6b53b3aabc10a90bcd0b`.
Base model: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3

GGUF conversion by handy-computer for transcribe.cpp: https://huggingface.co/handy-computer/parakeet-ultra-gguf/tree/a1539be1b849ea7463b88343e5277b373d0c4ea9
Changes, as its NOTICE states: the conversion repackages the weights (renamed and re-laid-out tensors) and the Q8_0 file
Zeron downloads stores them at reduced numeric precision. The weights were not retrained or fine-tuned. Zeron pins the
conversion's revision and the SHA-256 and size of the file it uses in `models.json`, and verifies them as it downloads.

Languages: Bulgarian, Croatian, Czech, Danish, Dutch, English, Estonian, Finnish, French, German, Greek, Hungarian,
Italian, Latvian, Lithuanian, Maltese, Polish, Portuguese, Romanian, Russian, Slovak, Slovenian, Spanish, Swedish,
Ukrainian.

## Cohere Transcribe

Cohere Transcribe 03-2026 weights © Cohere, licensed under the Apache License 2.0:
https://www.apache.org/licenses/LICENSE-2.0

Original model: https://huggingface.co/CohereLabs/cohere-transcribe-03-2026 at commit `76b8b23` (gated; Zeron never
downloads from it).

GGUF conversion by handy-computer for transcribe.cpp: https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf/tree/7353b6040bdc1be9e5a3823d99d5fe12afe88264
Changes, as its NOTICE states: the conversion repackages the weights (renamed and re-laid-out tensors) and the Q5_K_M file
Zeron downloads stores them at reduced numeric precision. The weights were not retrained or fine-tuned. Pinned in
`models.json` like Parakeet Ultra.

Languages: Arabic, Chinese, Dutch, English, French, German, Greek, Italian, Japanese, Korean, Polish, Portuguese,
Spanish, Vietnamese.

## Runtime

transcribe.cpp 0.4.2 (MIT, © The transcribe.cpp authors, https://github.com/handy-computer/transcribe.cpp) through
the `transcribe-cpp` / `transcribe-cpp-sys` crates, built from source and linked statically. It vendors ggml (MIT,
© The ggml authors), miniz (MIT) and Silero VAD (MIT); their texts are in the crate's `THIRD-PARTY-LICENSES.md`.
Metal on macOS; the CPU (x86-64 with AVX2, or arm64) on Linux and Windows. Capture: cpal 0.17.3 (Apache-2.0).
Sample-rate conversion: rubato 0.16.2 (MIT), its anti-aliasing FFT resampler, from the device's rate to 16 kHz.

Both are offline models: Zeron records at most 60 seconds and decodes once on Stop. Model weights are an
optional download, never bundled. No cloud provider, Apple Speech or engine-host audio transport is used.

## Test clips

`testdata/` holds one LibriSpeech clip and one FLEURS clip, both CC BY 4.0; see `testdata/README.md`.
