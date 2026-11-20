# Native modules

The app's own Rust, built apart from the app as libraries zgx loads
(zgx's `docs/design/runtime.md`, zgx-script's `modules` feature). A change to
a module rebuilds that module alone: no build of the app, no build of zgx.

One so far, as the proof: `reply`, the long replies' blocks set aside from
their rows (`src/chat.rs` `set_aside` and the three `chat.reply_*` functions
the transcript's scripts call). The app still has its own copy of that code
and uses it unless a launch names the module:

```sh
(cd modules/reply && cargo build --release)
ZERON_ZGX_REPLY_MODULE=$PWD/modules/reply/target/release/libzeron_reply.so cargo run --release -- main
```

Both ways pass the same tests (`cargo test --bin zeron-zgx`, with and without
the variable), and `reply_module_costs` in `src/long_thread_tests.rs` measures
one against the other.

Measured on Linux (Ryzen 9 5900X), release, medians of 5 runs of 21 rounds:

| | Linked in | Module |
| --- | --- | --- |
| `ui.call_sync("chat.reply_blocks", id)` | 2,359 instructions, 176 ns | 2,407, 179 ns |
| `ui.call_sync("chat.reply_run", { id, key })` | 7,138 instructions, 567 ns | 7,301, 593 ns |
| A 2,001-block reply set aside | 3.44 M instructions, 234 µs | 3.81 M, 277 µs |
| Rebuild after a one-line edit, dev | 3.13 s (the app) | 0.17 s |
| Rebuild after a one-line edit, release | 101.1 s (the app) | 1.93 s |
