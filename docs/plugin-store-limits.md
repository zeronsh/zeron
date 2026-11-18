# Plugin store: package size limit

The store takes packages up to **20 MB** (`STORE_MAX_PACKAGE_BYTES`,
`edge/src/store/publish.ts`). A device takes up to 50 MB of files (about
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
- A body with a declared length is read into one buffer of that size
  (`read-capped.ts`), not collected and then joined.
- A version's files are one R2 object (`files/<id>/<version>/<sha256>.bin`)
  served by ranged reads, not up to 2,000 separate writes.

Re-run the script after changing the pipeline:
`node edge/scripts/store-package-limit.mjs [MB …] [--no-r2]`.
