# Parakeet v3 attribution and runtime

NVIDIA Parakeet TDT 0.6B v3 weights © NVIDIA, licensed under Creative Commons Attribution 4.0 International: https://creativecommons.org/licenses/by/4.0/

Original model: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
Source model card inspected at revision `541d1f99c6b0c3cd0b11a95167540bb8edefd82b`.

ONNX conversion by Ivan Stupakov: https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/tree/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce
The conversion card identifies NVIDIA v3 as its base and documents NeMo ASRModel export and vocabulary extraction. Its exact original-weight revision and INT8 conversion tool version are not published. Zeron pins the conversion's immutable revision and SHA-256 of every consumed artifact in model.json; it does not claim independent numerical equivalence to NVIDIA's FP32 weights. The change is an INT8 ONNX conversion; no fine-tuned model is used.

Runtime: parakeet-rs 0.3.8 (MIT OR Apache-2.0), ONNX Runtime 1.28.0 via ort/ort-sys 2.0.0-rc.13, CPU execution. Cargo.lock pins registry checksums and the transitive graph. ONNX Runtime is MIT licensed. Capture: cpal 0.17.3 (Apache-2.0). Sample-rate conversion: rubato 0.16.2 (MIT), using its anti-aliasing FFT resampler to convert device-rate mono audio to 16 kHz. Runtime libraries are linked into the application by ort-sys; model weights are an optional download, never bundled.

TDT v3 is an offline model. Zeron records at most 60 seconds and decodes once on Stop. There are no live partial hypotheses in the production adapter. The editor's partial-result seam exists for deterministic lifecycle tests and future adapter work; it does not imply native streaming support.

Supported languages: Bulgarian, Croatian, Czech, Danish, Dutch, English, Estonian, Finnish, French, German, Greek, Hungarian, Italian, Latvian, Lithuanian, Maltese, Polish, Portuguese, Romanian, Russian, Slovak, Slovenian, Spanish, Swedish, Ukrainian.

Platform baseline: native CPU ONNX and CPAL on macOS, Windows, and Linux. Linux build hosts need ALSA development headers in addition to existing desktop dependencies. macOS requires a packaged app with NSMicrophoneUsageDescription and the audio-input entitlement under hardened runtime. No Apple Speech entitlement, cloud provider, Python installation, or engine-host audio transport is used. Only macOS arm64 is verified locally; other targets require their native CI/package validation.
