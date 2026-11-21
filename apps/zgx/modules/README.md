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

Measured on Linux (Ryzen 9 5900X), release. Instructions are the calling
thread's, counted by valgrind (callgrind) around each loop of 20,000 calls:
the median of 20 rounds, good to about 10 a call.

| | Linked in | Module |
| --- | --- | --- |
| `ui.call_sync("chat.reply_blocks", id)` | 3,911 instructions | 3,921 |
| `ui.call_sync("chat.reply_run", { id, key })`, the last run | 9,391 instructions | 9,287 |
| The same, the first run | 8,594 instructions | 8,510 |
| `ui.call_sync("chat.reply_run_of", { id, block })` | 8,545 instructions | 8,423 |
| The same loop calling a function that does nothing | 6,688 instructions | 6,675 |
| A 2,001-block reply set aside | 3,404,160 instructions, 234 µs | 1,985,702, 151 µs |
| Rebuild after a one-line edit, dev | 3.13 s (the app) | 0.18 s |
| Rebuild after a one-line edit, release | 101.1 s (the app) | 2.21 s |

(The bench checks in its script that each function answers before it times
a loop. An earlier table's first two rows, 2,347 and about 7,000, were of
calls made after the reply had been let go of, which answer nil at once.)

The module reads the app's values where they are, as the app's own code
does, so the boundary itself costs an indirect call and the argument's
general drop (14 instructions, by zgx's `module_call_bench`). It comes out
under the linked-in code on every row but `reply_blocks` because it reads
each block's fields once and borrows the strings the original clones, not
because of where it is built.

Without a counter the kernel allows (`perf_event_paranoid` over 2), count
from outside: `REPLY_BENCH_CASE=<nothing|blocks|run|run_first|run_of|aside>`
and `REPLY_BENCH_ROUNDS=<n>` run that one loop n times and nothing else, so
two runs under valgrind differ by the rounds between them.

zgx's values have types of its own on this pin (`Str`, `List`, `Map`,
`Field`), which is what lets a module read them; `src/` moved with them
(104 lines in 12 files; zgx's `docs/design/runtime.md` appendix has each
pattern). A `Str` is not an `Rc<str>`: what keeps a value's string here
keeps a `Str`, and nothing in `src/` turns one into the other.
