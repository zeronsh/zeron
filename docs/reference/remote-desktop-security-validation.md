# PR 387 security remediation validation

Validated on Fedora 44 on 2026-09-16. This record covers the auditor's three
comments on commit `276b54f7`, not a new full-repository security audit.

## Clipboard reassembly — fixed

An authenticated RDP peer could send SVC fragments without LAST indefinitely.
The old accumulator exceeded the application's clipboard limit before the
CLIPRDR callback could validate the text. A bounded regression reproduction
failed against the original SVC source; the valid exact-limit control passed.

The locally patched SVC accumulator checks both the declared size and actual
cumulative bytes before appending, drops partial data on overflow, and returns
an error. The connector sets CLIPRDR's limit to 1 MiB plus its eight-byte message
header before negotiation. The session propagates that error and drops the
connection. Existing clipboard text, focus, and generation checks remain intact.

Five SVC tests cover endless non-LAST fragments, huge advertised lengths, false
small declarations, repeated FIRST flags, oversized standalone messages, memory
release, and exact-limit fragmentation followed by another valid message.

## Display Control reassembly — fixed

The original DVC accumulator accepted a tiny DataFirst announcing `u32::MAX`
bytes, allowing continued growth before the 20-byte application check. That
reproduction failed against the original DVC source while valid fragmentation
passed.

The patched DVC accumulator captures the processor's limit at channel creation.
Display Control configures 20 bytes; standalone and fragmented input is checked
before retention or growth. The outer drdynvc SVC is independently limited to
64 KiB before the DVC decoder copies data. This accommodates channel management
messages while bounding the outer accumulator too. Overflow errors terminate
the session. Outbound monitor layout encoding is unchanged.

Five DVC tests cover huge totals, oversized actual payloads, continuations past
the cap or declared length, and replacement/following-message state. Three new
RDP tests exercise real nested SVC/DVC decoding, including headers split across
outer fragments, channel IDs encoded in one/two/four bytes, valid 20-byte CAPS,
and attacks against either accumulator. Independent read-only candidate reviews
found no concrete surviving bypass or compatibility regression in either fix.

Both crates retain their original MIT/Apache-2.0 notices. See
[`vendor/README.md`](../../vendor/README.md) for source archive checksums,
local patch details, and the upgrade/removal conditions.

## Verification commands and results

1. Syntax and integration: `git diff --check` and `cargo fmt --check -p zeron-rdp`
   passed. `cargo check --locked -p zeron-ui --example remote-desktop-fixture
   --features remote-desktop-fixture` passed, with existing unrelated warnings.
2. Security and legitimate controls: `cargo test --locked -p ironrdp-svc
   -p ironrdp-dvc -p zeron-rdp` passed all 30 tests (5 SVC, 5 DVC, 16 RDP unit,
   4 lifecycle). Both original reproductions now reject before unbounded growth;
   valid fragmentation remains accepted. These tests are wired into RDP CI.
3. Interoperability: `cargo build --release --locked -p zeron-rdp --example probe`
   passed. A live xrdp run after the clipboard fix passed the standard lab's
   Unicode input, bidirectional clipboard, resize, and disconnect assertions.
   With both fixes, a controlled xrdp run passed bidirectional Unicode clipboard,
   CRLF conversion, confirmed 1280x800 to 1024x768 resize, and clean disconnect.
   The controlled run initialized xclip directly on the server before the probe's
   request and read back the offered local text independently of xterm input.

The standard xterm-driven lab also intermittently failed with "Remote clipboard
does not offer Unicode text", including when using the previously built client
without these patches. The successful controlled run establishes protocol
interoperability but does not resolve that existing fixture instability.
Local evidence: `/tmp/pr387-lab-inspect.pPs2oN` (standard lab success),
`/tmp/pr387-controlled-clipboard.QoJJ1s` (both fixes, controlled server selection),
and `/tmp/pr387-clipboard-baseline.DtWeds` (unpatched-client lab failure).

No new native macOS/Windows validation or full repository audit was performed.

## Rustls advisory — already corrected (`no_change`)

The auditor's original head resolved Rustls 0.23.43. The later rebase onto main
already retained Rustls 0.23.45, the patched version for
[RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html).
No additional dependency or runtime change is necessary for this finding.

At the remediation head, `cargo tree --locked -p zeron-rdp -i rustls` reports:

```text
rustls v0.23.45
└── tokio-rustls v0.26.4
    └── zeron-rdp v0.2.71
```

The committed Cargo.lock contains that same version. Locked builds and tests
use this resolved dependency rather than the older version in the audited head.
The existing certificate-pin/handshake-signature test was also rerun:

```sh
cargo test --locked -p zeron-rdp tls::tests::certificate_pins_still_require_handshake_signatures_and_changed_pins_prompt
```

Result: passed (1 test). That test protects the application's certificate and
signature behavior; it is not a reproduction of the upstream TLS encryption-level
bug. Closure of this specific dependency finding rests on resolving the patched
version. This verification does not claim a fresh full-tree `cargo audit` run.
