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

The module is written with zgx's macros (`#[zgx_module::module]`,
`#[export]`, `#[derive(Record)]`; zgx's `docs/modules.md`): each function
takes typed arguments (`id: &str`, `run: Run` for `{ id, key }`) read where
the runtime holds them, and its types are in the module's manifest, which
`zgx types` reads. An argument of the wrong kind is now an error raised in
the script naming the function (by hand, it read as an empty string).

Measured on Linux (Ryzen 9 5900X), release. Instructions are the calling
thread's, counted by valgrind (callgrind) around each loop of 20,000 calls:
the median of 20 rounds, good to about 10 a call. "By hand" is the same
module written without the macros (registering closures), as it was before.

| | Linked in | Module | Module, by hand |
| --- | --- | --- | --- |
| `ui.call_sync("chat.reply_blocks", id)` | 3,913 instructions | 3,899 | 3,933 |
| `ui.call_sync("chat.reply_run", { id, key })`, the last run | 9,403 | 9,287 | 9,316 |
| The same, the first run | 8,603 | 8,525 | 8,542 |
| `ui.call_sync("chat.reply_run_of", { id, block })` | 8,547 | 8,435 | 8,437 |
| The same loop calling a function that does nothing | 6,693 | 6,686 | 6,680 |
| A 2,001-block reply set aside | 3,404,306 instructions | 1,986,294 | 1,986,512 |
| Rebuild after a one-line edit, dev | 3.13 s (the app) | 0.17 s | 0.18 s |
| Rebuild after a one-line edit, release | 101.1 s (the app) | 1.91 s | 2.21 s |
| Clean build, release | 228.2 s (the app) | 3.90 s | 2.05 s |

(The bench checks in its script that each function answers before it times
a loop. An earlier table's first two rows, 2,347 and about 7,000, were of
calls made after the reply had been let go of, which answer nil at once.)

The module reads the app's values where they are, as the app's own code
does, so the boundary itself costs an indirect call and the argument's
general drop (14 instructions, by zgx's `module_call_bench`). It comes out
under the linked-in code because it reads each block's fields once and
borrows the strings the original clones, not because of where it is built.
The macros cost nothing against the module by hand; a clean build of the
module compiles them (`syn`) once.

Without a counter the kernel allows (`perf_event_paranoid` over 2), count
from outside: `REPLY_BENCH_CASE=<nothing|blocks|run|run_first|run_of|aside>`
and `REPLY_BENCH_ROUNDS=<n>` run that one loop n times and nothing else, so
two runs under valgrind differ by the rounds between them.

zgx's values have types of its own on this pin (`Str`, `List`, `Map`,
`Field`), which is what lets a module read them; `src/` moved with them
(104 lines in 12 files; zgx's `docs/design/runtime.md` appendix has each
pattern). A `Str` is not an `Rc<str>`: what keeps a value's string here
keeps a `Str`, and nothing in `src/` turns one into the other.

## The pin

This branch pins zgx's runtime line (`runtime/stage-2-at-b8f298b`,
888f59b1e): native modules, `zgx run` and `zgx build`, bundles, stage 2
(a module's threads posting to its host, timers, shared state, async
actions) and stage 2b (a shared list patched by key, images by name, a
write to a gone host failing), on the zgx this app was released with.
zgx's main has all of it since 235c37315 (#2414, #2415, #2416, #2440 and
#2446): moving the app's pins
and the modules' `zgx-module` pins there brings everything else main has
gained since, and is Zeron's call.

The sessions module (`sessions/`) is stage 2's proof: the chat list kept
from an engine connection of its own, beside the app's own list, loaded
only where `ZERON_ZGX_SESSIONS_MODULE` names its library.

## Releases without a Rust build

On this branch, `scripts/package-linux.sh` with `ZGX_BUNDLE=1` packages the
app as zgx's runtime and a bundle (zgx's `docs/modules.md`, `zgx build`):

- The binary is built only when its Rust changed. Its key is a hash of
  every tracked file but `ui/`, `plugins-builtin/` and `modules/`, plus the
  compiler; built for a key, it is kept in zgx's cache. The version is no
  longer compiled in: the bundle's manifest carries it, read at launch.
- The scripts ship as bytecode, with a manifest of hashes; each native
  module is taken from zgx's cache, or built for release if its sources
  changed.

A change to the host's Rust still rebuilds the binary, as before. Only
changes to Luau files and to modules skip it.

Measured on Linux (Ryzen 9 5900X), engine given prebuilt in both:

| A beta release where only… | Today | Bundle |
| --- | --- | --- |
| a Luau file changed | 111.5 s, 111.8 s | 10.7 s, 10.7 s |
| the `reply` module changed | (as above: the app rebuilds) | 12.6 s, 12.6 s (module built in 1.9 s) |

The first bundle release on a new key builds the binary once (117.8 s).

Startup, ms from the process's start to zgx's marks, the median of 15
interleaved launches under Xvfb, without the engine:

| | App built | First frame presented |
| --- | --- | --- |
| Today, first launch | 84.5 | 189.9 |
| Bundle, first launch (its scripts hashed: 26 ms) | 71.2 | 176.3 |
| Today, later launches | 84.3 | 145.4 |
| Bundle, later launches (checked by stamps: 5.5 ms) | 71.5 | 133.9 |
