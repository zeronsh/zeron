#!/usr/bin/env python3
"""Kotlin 2 fixes for UniFFI 0.32 bindings.

- TranscriptView's Rust `close()` clashes with AutoCloseable.close().
- CoreException fields named `message` clash with Throwable.message.
"""
from pathlib import Path
import sys

path = Path(sys.argv[1] if len(sys.argv) > 1 else "apps/android/app/src/main/java/uniffi/zeron_core/zeron_core.kt")
text = path.read_text(encoding="utf-8")
text = text.replace("fun `close`()", "fun `closeEngine`()", 2)
start = text.find("sealed class CoreException")
conv = text.find("public object FfiConverterTypeCoreError")
conv_end = text.find("\npublic object ", conv + 10)
if start < 0 or conv < 0 or conv_end < 0:
    sys.exit("patch-uniffi-kotlin: markers not found")
mid = text[start:conv_end]
mid = mid.replace("val `message`: kotlin.String", "val reason: kotlin.String")
mid = mid.replace('get() = "message=${ `message` }"', "get() = reason")
mid = mid.replace("value.`message`", "value.reason")
path.write_text(text[:start] + mid + text[conv_end:], encoding="utf-8")
print(f"patched {path}")
