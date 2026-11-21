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

Measured on Linux (Ryzen 9 5900X), release, medians of 5 runs of 21 rounds,
the two ways interleaved. Instructions are the calling thread's, in user mode:

| | Linked in | Module |
| --- | --- | --- |
| `ui.call_sync("chat.reply_blocks", id)` | 2,347.3 instructions, 175 ns | 2,347.0, 166 ns |
| `ui.call_sync("chat.reply_run", { id, key })` | 7,010 instructions, 574 ns | 6,909, 559 ns |
| A 2,001-block reply set aside | 3.42 M instructions, 234 µs | 1.99 M, 151 µs |
| Rebuild after a one-line edit, dev | 3.13 s (the app) | 0.18 s |
| Rebuild after a one-line edit, release | 101.1 s (the app) | 2.21 s |

The module reads the app's values where they are, as the app's own code
does, so the boundary itself costs an indirect call and the argument's
general drop (10 instructions, by zgx's `module_call_bench`). It comes out
under the linked-in code because it reads each block's fields once and
borrows the strings the original clones, not because of where it is built.

zgx's values have types of its own on this pin (`Str`, `List`, `Map`,
`Field`), which is what lets a module read them; `src/` moved with them
(104 lines in 12 files; zgx's `docs/design/runtime.md` appendix has each
pattern). A `Str` is not an `Rc<str>`: what keeps a value's string here
keeps a `Str`, and nothing in `src/` turns one into the other.
