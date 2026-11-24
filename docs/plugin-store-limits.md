# Plugin store: package size limit and the bounds on a body

The rollout (the hard gate before main: real D1 ids, buckets, the deploy
token's D1 access, the migrations step) is in
`docs/plans/2026-11-18-plugin-store-catalog-rollout.md`.

## The package limit

The store takes a request body (the `.zeron-plugin` file exactly as
uploaded: its files as base64 inside JSON) of at most **20,971,520 bytes**
(20 MiB): `MAX_PUBLISH_BODY_BYTES` (`edge/src/store/identity/body.ts`),
which `STORE_MAX_PACKAGE_BYTES` (`edge/src/store/publish.ts`) is. It is
`limits.max_publish_body_bytes` in `docs/plugin-store-data.json` too, and a
test holds the two equal. There is no
second number for decoded files: base64 makes them at most three quarters of
the body (about 15.7 MB), and the engine's own 50 MB limit on files is
checked as well. A device takes up to 50 MB of files (about
67 MB of package, `crates/engine/src/plugins/package.rs`); a bigger plugin
can still be sideloaded.

## How it was measured

`edge/scripts/store-package-limit.mjs` starts a fresh local workerd per size
(`wrangler dev` on `edge/scripts/store-limit/`, which runs the publish
pipeline's heavy steps: read the body, verify the package as a device does,
read its listing, classify every file, run the checks, write the package and
the version's files object to R2), warms it with a small package, sends the
measured one, and reads from `/proc` the workerd process's resident memory
before, its peak (`VmHWM`) after, and the CPU time the request used.

The packages are the worst case for the edge: half random binary files, half
random text, 256 KB each, signed.

Local R2 runs inside the same workerd process, so each size was measured
with the R2 writes and without them (`--no-r2`): the second is
the pipeline itself; in production R2 is a network service.

| Package | Files | Peak over idle, no R2 | CPU, no R2 | Peak over idle, with local R2 | CPU, with local R2 |
|---:|---:|---:|---:|---:|---:|
| 1 MB | 6 | 5 MB | 40 ms | 12 MB | 70 ms |
| 5 MB | 18 | 16 MB | 90 ms | 48 MB | 240 ms |
| 10 MB | 33 | 30 MB | 180 ms | 93 MB | 610 ms |
| 15 MB | 48 | 45 MB | 250 ms | 132 MB | 950 ms |
| 20 MB | 63 | 59 MB | 300 ms | 170 MB | 1,570 ms |
| 25 MB | 78 | 73 MB | 390 ms | 189 MB | 1,100 ms |
| 30 MB | 93 | 82 MB | 460 ms | 201 MB | 1,430 ms |
| 40 MB | 123 | 106 MB | 580 ms | 256 MB | 1,950 ms |
| 50 MB | 153 | 116 MB | 740 ms | 272 MB | 2,200 ms |

(Linux, 24 cores; 2026-11-18. Resident memory is an upper bound on what the
isolate holds: it includes garbage V8 hadn't collected yet.)

## Why 20 MB

A Worker's isolate has 128 MB. The pipeline peaks at about three times the
package (the request's bytes, the decoded files in one buffer, and
transient copies while hashing), so 20 MB costs about 60 MB, leaving room
for everything else the edge worker holds. 40 MB would not. CPU is far
below the Workers limit at every size.

What keeps it at three times, not more (each measured: the first version
peaked at 280 MB for 20 MB without these):

- File data is decoded from the request's bytes straight into one buffer
  (`json.ts` `RawString`), never copied into JavaScript strings first.
- Files are classified as text or binary in place (`checks.ts` `isText`),
  not decoded.
- A version's files are one R2 object (`files/<id>/<version>/<sha256>.bin`)
  served by ranged reads, not up to 2,000 separate writes.

Re-run the script after changing the pipeline:
`node edge/scripts/store-package-limit.mjs [MB …] [--no-r2]`.

## Through the edge's body reader

The store reads bodies through `readBody` (`edge/src/store/identity/body.ts`).
It keeps every chunk and then joins them, never allocating from
`Content-Length`, so the body is briefly held twice. Measured that way
(`--chunks`):

| Package | Peak over idle | CPU |
|---:|---:|---:|
| 10 MB | 37 MB | 150 ms |
| 20 MB | 72 MB | 280 ms |

20 MB still fits with room; the limit stands.

## Hostile bodies

A body need not be a package. Each shape below is valid-looking JSON at the
size given, sent signed in with a username (`--shape=…`, with `--chunks`).
The first version of the parser built a JavaScript value for every JSON
value; the store now checks the whole document without building it
(`edge/src/store/scan.ts`), builds only the manifest (at most 1 MB of JSON
and 100,000 values, else `too_large`), and decodes file data from the
request's bytes.

| Shape | Size | Before: peak / CPU | Now: peak / CPU | Answer now |
|---|---:|---:|---:|---|
| a package plus `"x":[0,0,0,…]` | 8 MB | 243 MB / 830 ms | 23 MB / 110 ms | accepted (an unknown member, as the engine) |
| | 20 MB | 527 MB / 3,040 ms | 57 MB / 250 ms | accepted |
| a package plus `"x":0.000…001` (one number) | 8 MB | 261 MB / 480 ms | 23 MB / 60 ms | accepted |
| | 20 MB | 536 MB / 1,090 ms | 56 MB / 160 ms | accepted |
| a package plus `"x":[{},{},…]` | 8 MB | 378 MB / 1,010 ms | 23 MB / 100 ms | accepted |
| | 20 MB | 876 MB / 2,660 ms | 57 MB / 200 ms | accepted |
| a file's data of `\/\/\/…` | 8 MB | 155 MB / 270 ms | 29 MB / 100 ms | `bad_file` |
| | 20 MB | 323 MB / 610 ms | 72 MB / 220 ms | `bad_file` |
| a manifest holding `[0,0,0,…]` | 8 MB | 384 MB / 1,890 ms | 23 MB / 100 ms | `too_large` |
| | 20 MB | 837 MB / 4,240 ms | 57 MB / 240 ms | `too_large` |
| `files` with a million empty keys | 22 MB | — | 69 MB / 180 ms | `too_large` (past 2,000) |

Every "before" row is past a Worker's 128 MB, so it would have ended the
isolate and every request on it; every "now" row is the body itself held
twice by the reader, then freed, and linear CPU.

## What else an author's text meets

Every pattern run on text a publisher controls is linear or bounded:

- the package's JSON: one pass (`scan.ts`); a number's range is worked out
  from at most 800 of its digits; the manifest is parsed under the bounds
  above;
- `CHANGELOG.md`: refused past 512 KB before it is read; notes are found by
  a line scan with no regular expression (`listing.ts` `changelogNotes`);
- `README.md`: refused past 512 KB, then decoded once;
- `ui.change` selectors and network hosts: refused past 200 and 253 bytes
  before any parsing, then scanned with simple patterns;
- keywords, URLs, names, descriptions, paths: single-class patterns
  (`/^[a-z0-9 -]+$/`, `/[\p{Cc}\p{Cf}]/u`), each linear, on fields the
  listing rules bound;
- search terms (`?q`, at most 200 characters): `/[\p{L}\p{N}]+/gu`, linear.

## CPU per hostile shape, after the review

The identity lane's review measured the scanner at 0.6–6.3 s of CPU on
some 20 MiB shapes: it allocated a stack per value, an object per span
and a string per key. Now positions are scanner fields, the stack is one
per scanner, top-level keys are compared as bytes, `\u` escapes decode
inline, a repeated `files` key is read once, and a number at 10^308 is
judged against the exact midpoint digit by digit. `src/store/scan.test.ts`
holds each shape, at the 20 MiB cap, to 1.5 s through `verifyPackage`
(Node 22, this box):

| Shape at 20 MiB | Before (review) | Now |
|---|---:|---:|
| `{"a":0,"a":0,…}` | 6.3 s | 171 ms |
| `{"a":[],"a":[],…}` | 4.3 s | 178 ms |
| `files` full of `"a":""` | 2.9–5.5 s | 194 ms |
| `files` of distinct keys | — | 132 ms |
| one 20 MB string of `A` | 0.9–1.8 s | 43 ms |
| `1e308` repeated | 0.6–0.8 s | 120 ms |
| file data of `\/` escapes | — | 127 ms |
| file data of `\u0041` escapes | — | 134 ms |
| `[0,0,0,…]` | — | 177 ms |
| `[{},{},…]` | — | 181 ms |

`limits.cpu_ms` in `edge/wrangler.jsonc` is pinned at 30,000, the paid
plan's default: it applies to the whole worker and its Durable Objects, and
the session rooms' cold replays run near it, so a lower value would cut
them. The store needs far less.

## The scanner against serde_json

Depth: serde_json allows at most 127 containers open at once in a
document, the package's own top-level object among them. The scanner counts
the same (`src/store/units.test.ts`, "store package depth": 127, 128 and
129 at the top level, inside the manifest and inside a file's data).

Differential testing, two tiers:

- `npm test` runs a fixed corpus (`edge/test/fixtures/scan-corpus.*`, 3,175
  inputs with serde_json's own answers: hand-written edges, generated
  packages, byte mutations) through `src/store/scan-corpus.test.ts`, with no
  cargo needed.
- `node edge/scripts/scan-fuzz/run.mjs [--seed N] [--count N] [--valid]`
  generates fresh inputs and compares the scanner with serde_json live
  (`scripts/scan-fuzz/reference`, the engine's serde_json version and
  features). Run it with a few seeds after any change to `scan.ts` or
  `json.ts`; `--write-corpus` refreshes the fixed corpus. Last run: seeds 1–5,
  840,875 inputs, 0 differences.

