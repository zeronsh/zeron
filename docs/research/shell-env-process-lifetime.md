# Login-shell probe process lifetime

Investigation and fix, 2026-10-01.

## Evidence and scope

The reported incident involved approximately 7,300 Bash processes arranged in
parent/child chains running the same environment-marker command. Some roots had
already been adopted by systemd. Reported usage was about 52 GiB of 62 GiB RAM
and 6.9 GiB swap; after the chains disappeared, RAM usage returned to about
10 GiB. These are incident observations supplied by the reporter, not a
reproduction performed during this investigation. Adoption by systemd does not
identify the original launcher.

The inspected repository revision was
`42926c802a837097e6a05de89d48ca7ade326658` (workspace version `0.2.100`).
Personal shell configuration, the installed executable, version links, and
services were left unchanged during the investigation.

## Call flow and possible re-entry

1. Engine construction and UI/profile entry points call `default_registry()` in
   `crates/engine/src/registry.rs`, which starts `shell_env::prewarm()`.
2. The prewarm thread calls `login_shell_path()`. Executable discovery
   (`executable::find_on_paths`, adapter npm discovery), child PATH composition,
   and installer PATH configuration can also call it directly.
3. `OnceLock<Option<OsString>>` serializes and caches the result, including
   failures, **within one process**.
4. `capture()` selects an executable shell from `SHELL`, the passwd entry, or
   known defaults. `snapshot_path()` tries the shell-specific flag sets,
   normally interactive login followed by noninteractive login.
5. Each attempt runs `echo __ZERON_SHELL_ENV_BEGIN__; env; echo
   __ZERON_SHELL_ENV_END__` with null stdin/stderr and piped stdout. Startup
   files execute before this payload. The child inherits the environment with
   `ZERON_RESOLVING_ENVIRONMENT=1` and `TERM=dumb` added.
6. Parsing extracts a nonempty PATH between the last begin marker and its end
   marker. An unusable attempt falls back to the next flag set.

The snapshot code has no recursive call into itself: one cache initialization
runs at most two attempts. However, shell startup is external code. If it
launches a new program that reaches this module, that program has a fresh
cache. Previously `capture()` ignored the inherited resolving marker, so this
route could initiate another probe. The new guard closes that route when the
marker is preserved and the descendant uses the corrected code.

This is a possible re-entry route, **not the confirmed incident trigger**.
Direct shell re-execution, wrappers, or code that removes the marker are other
possibilities that this guard cannot rule out. A limited read-only inspection
of the current Bash startup files, their `.bashrc.d` includes, `/etc/profile.d`,
and the Cargo environment script found no explicit Zeron/Comet or probe-marker
reference. This was neither a dynamic execution trace nor an exhaustive audit
of everything startup code might invoke. No unlimited recursion was attempted.

## Installed executable versus source

The links still resolve as follows:

- `~/.local/bin/zeron` -> `~/.zeron/app/current/zeron`
- `~/.zeron/app/current` -> `~/.zeron/app/0.2.98`
- The installed executable reports `zeron 0.2.98` with `--version`.
- SHA-256: `a42d677026dfc8c2dbddb5a0401935f6ab2365e297c77a7298e5ef58f6427cf3`.
- ELF build ID: `a90d7a20dd886145a9def75c3193c0b6bd17046f`.

`--version` was run with both probe-disable indicators set; it exits during CLI
argument parsing. The binary contains the begin marker, both environment
variable names, and `zeron-shell-env-read`. The repository tag `v0.2.98`
(`d2f11d336b0e6f69ca66ffed5aad31bbd8312a63`) has a byte-identical
`crates/harness/src/shell_env.rs` to the inspected HEAD before this change.

These facts support the presence of the same probe implementation in the
installed release, but do not establish its exact build provenance or prove
its control flow. No reproducible-build comparison, build attestation, or
runtime trace of that executable's probe was available. Its behavior has not
been equated conclusively with the checkout. The installed application still
has its original binary; this fix has not been deployed.

## Confirmed defects and correction

Previously cleanup killed/reaped only the direct child, and skipped cleanup
when `try_wait()` had already observed its exit. Descendants could keep running
on timeout, successful capture, or early shell exit. The detached blocking
stdout reader could survive the attempt if a descendant retained the pipe.
`MAX_OUTPUT` bounded captured text, not subprocess count or memory.

Each attempt now calls `setsid()` before exec, creating an isolated session
and process group and detaching from the caller's controlling terminal. This
also prevents normal interactive job control from using that terminal to
separate jobs from the probe's group. A setup failure aborts spawning.

A scope guard sends SIGKILL to the exact owned group on every exit path, then
waits for the direct child. It covers a complete snapshot, EOF, timeout, output
limit, descriptor setup/read errors, and stack unwinding. The leader is never
reaped before group signaling, preventing reuse of its PID for an unrelated
group. No group is discovered through argument matching and no signal targets
the caller's group. Group members can become zombies briefly; their own
parent or the OS reaper collects them. The probe reaps its direct child.

Stdout is read synchronously with `O_NONBLOCK`. There is no reader thread to
join or abandon, and the descriptor closes at scope exit. The loop checks the
five-second deadline even with continuous output, caps stored bytes at exactly
2 MiB, and scans only new bytes plus marker overlap. If the shell exits while a
descendant retains stdout without a complete marker, the attempt now waits up
to that deadline instead of the former 250 ms post-exit grace. Avoiding early
reaping keeps group cleanup safe. EOF and complete snapshots still return
promptly. Shell selection, marker parsing, per-process cache, flag order, and
fallback remain intact.

`capture()` now treats a nonempty inherited `ZERON_RESOLVING_ENVIRONMENT` as a
reason to cache `None`, like `ZERON_NO_LOGIN_SHELL`. Empty values still allow
capture. This is defense against nested library probes, not evidence that the
missing check caused the reported Bash chains.

## Containment limits

A Unix process group is not a process-tree sandbox or a resource quota.
Descendants that deliberately call `setsid()`/`setpgid()`, change credentials,
or launch work through an external service can escape this cleanup. The
nonblocking reader still returns if an escaped writer retains stdout. The
nested guard can also be removed by external code or ignored by older builds.
Rapid process growth within an attempt's deadline is not averted by an output
cap. The original recursion trigger remains unidentified; no claim is made
that the original cause has been resolved.

## Validation and portability

Actual execution platform: Linux x86_64, kernel `7.2.5-200.fc44.x86_64`, Rust
`1.98.0`. Builds were limited to two jobs; no full workspace/application build
was performed.

Targeted commands:

```sh
cargo test -p zeron-harness --lib shell_env::unix::tests --jobs 2 -- --test-threads=1
cargo test -p zeron-harness --test shell_env_resolution --test shell_env_guard --jobs 2 -- --test-threads=1
rustfmt --edition 2024 --check crates/harness/src/shell_env.rs crates/harness/tests/shell_env_resolution.rs crates/harness/tests/shell_env_guard.rs
git diff --check
cargo clippy -p zeron-harness --lib --test shell_env_guard --test shell_env_resolution --jobs 2 -- -D warnings
```

Results: all 12 selected unit tests passed (including the subprocess helper),
as did both integration test binaries (the guard test exercises four isolated
process cases). Formatting and whitespace checks passed. Strict Clippy stopped
on 20 existing diagnostics in untouched files, including
`acp/devin_models.rs:114` (`type_complexity`), `redact.rs:288`
(`manual_pattern_char_comparison`), and existing `collapsible_if` findings in
Codex, Cursor, executable discovery, Pi, and skills code. Those unrelated files
were not changed; strict Clippy is not green.

The unit fixtures use only a fixed shell/wrapper/helper chain, bounded startup
waits, and a ten-second helper timeout. A private Unix socket witnesses helper
death and provides independent cleanup on assertion failure. No broad process
search or PID-based emergency cleanup is used. Tests cover timeout, success,
EOF, output cap, a leader exiting with inherited writers, and an escaped writer
that is explicitly released by the test. They check group isolation and direct
child reaping, plus parsing, normal discovery, and fallback. Separate-process
integration cases cover inherited guards, empty values, prewarm, caching, and
CLI resolution through the captured PATH.

The production code uses Unix `setsid`, `kill`, and `fcntl`, without Linux-only
`/proc`, pidfds, or subreapers. macOS compatibility was reviewed against libc
bindings and Apple's manuals for [setsid](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setsid.2.html),
[kill](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kill.2.html),
and [fcntl](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html).
The child hook follows the restrictions documented for Rust's
[CommandExt::pre_exec](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec).
Only the Linux target is installed locally. macOS was not compiled or executed;
its runtime validation remains pending. Non-Unix behavior is unchanged.
