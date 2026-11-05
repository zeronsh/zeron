# Development media runtime

Zeron packages do not contain the Codex voice runtime. On macOS, Linux and
Windows, local and remote voice run the `codex-voice-host` helper of the user's
own standalone Codex installation (layout 1, version 0.159 or later), resolved
from the same `codex` executable the harness uses. npm installations do not
include the runtime and report `nativeRuntimeUnavailable`.

`scripts/package-voice-runtime.py` is a development tool only. It projects the
voice subtree of a checksum-pinned official Codex `0.160.0` package into a
`codex-resources/voice` directory (the helper refuses to initialize from any
other directory name), for example to test a machine without Codex:

```sh
python3 scripts/package-voice-runtime.py --download \
  --target x86_64-unknown-linux-gnu --destination /tmp/zeron/codex-resources/voice
```

macOS needs an explicit extracted package (`--package`). The projection keeps
binaries and libraries byte-for-byte (macOS signatures are verified, not
replaced), the dependency notices and sources manifest, and adds
`Codex-LICENSE.txt`, the unmodified OpenAI license at source commit
`a956835d020762cb2b570053af06f643a11c0ecc`:
https://github.com/openai/codex/blob/a956835d020762cb2b570053af06f643a11c0ecc/LICENSE

Point `ZERON_VOICE_MEDIA_DIR` at the projected directory to override the
installed helper for remote voice. Do not ship a projected runtime: its
GStreamer/GLib (LGPL-2.1), Opus, other native libraries and, on Windows, the
Microsoft Visual C++ runtime carry their own redistribution obligations.
