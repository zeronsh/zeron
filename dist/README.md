# Crew packaging and release

One upstream-based implementation produces the native desktop and headless Crew
runtime. Internal Cargo packages and artwork filenames may remain `zeron`; the
published compatibility executable is **`comet`**. The workspace release is
**0.2.136**. This release is staging-only: do not publish production or
repin production Scaffold. The authoritative lifecycle is
[`skills/comet-work/SKILL.md`](../skills/comet-work/SKILL.md).

## Brand artwork

`dist/zeron.svg` is the Crew mark: the original 46-unit four-piece square in
`#fafafa`, a 10-unit `#09de5e` core, and solid black. Regenerate desktop, launcher,
landing-page and iOS artwork with `python3 scripts/gen-brand-icons.py` (Pillow
required only for regeneration). Packaging uses the existing native icon and DMG
artwork; regeneration does not change installed applications.

## macOS signed distribution

Use an Apple-silicon macOS host with the checked-in Rust toolchain, Xcode command
line tools, Python 3 and network access for dependencies and Apple notarization.
Packaging builds the native upstream `zeron` executable with locked release
settings and copies that exact binary to `Contents/MacOS/comet`; it never launches
another installed app through a wrapper. Cargo metadata determines the build
cache location. Staging uses a separate `staging` build subdirectory.

Provision a Developer ID Application certificate and private key in a keychain,
and authenticate a notarytool profile before distribution:

```sh
COMET_PACKAGE_ENVIRONMENT=staging \
COMET_DEFAULT_ENVIRONMENT=staging \
CODESIGN_IDENTITY='Developer ID Application: Your Organization (ABCDE12345)' \
COMET_MACOS_SIGNING_TEAM_ID=ABCDE12345 \
NOTARYTOOL_KEYCHAIN_PROFILE=crew-distribution \
scripts/package-macos.sh
```

The expected team is independently configured, not inferred from credentials.
`CODESIGN_KEYCHAIN` optionally selects the signing/profile keychain. Missing,
wrong-team or invalid credentials fail before compilation. Signing uses hardened
runtime, secure timestamps and a version-independent designated requirement:
exact bundle identifier, Apple's Developer ID Application trust chain and leaf
team OU. Certificate renewal within that team preserves identity; changing teams
is an identity migration, not credential rotation. No cdhash or certificate serial
is pinned. The executable and app are signed inside-out, notarized and stapled;
the DMG is also signed, notarized and stapled. Read-only verification checks both
the app and the extracted updater archive without repairing signatures.

The hosted macOS packaging and independent verification jobs share one step
that blocks outbound UDP 443 in the dedicated `com.apple/crew-notary` filter
anchor, preserving existing rules and forcing TCP HTTPS for Apple ticket checks:
[runner issue #14924](https://github.com/actions/runner-images/issues/14924).
It fails closed if the system anchor is absent and applies only to these
disposable hosted VMs, never local hosts. Remove the workaround when the upstream
HTTP/3 network failure is resolved; all signing, notarization and Gatekeeper
checks remain required.

| Environment | App / identifier | URL scheme | Artifacts |
| --- | --- | --- | --- |
| staging | `Crew Staging.app` / `ai.ashler.comet.staging` | `comet-staging:` | `target/package/staging/comet-staging-0.2.116-macos-arm64.dmg`, `comet-staging-0.2.116-macos-arm64-app.tar.gz` |
| production identity | `Crew.app` / `ai.ashler.comet` | `comet:` | `target/package/comet-0.2.116-macos-arm64.dmg`, `comet-0.2.116-macos-arm64-app.tar.gz` |
| canary | `Crew Canary.app` / `ai.ashler.crew.upstream-canary` | `crew-upstream-canary:` | `target/package/canary/crew-upstream-canary-0.2.116-macos-arm64.dmg`, `crew-upstream-canary-0.2.116-macos-arm64-app.tar.gz` |

Every release run also signs and notarizes `Crew Canary.app` in its own
`macos-canary` job, uploaded as the private `macos-canary-arm64` Actions
artifact. It is never part of a candidate or any publication. A build-only run
(`promotion_target=none`, plus `reviewed_sha` for an unmerged branch) makes one
from any reviewed source. Install it instead of a local Canary build: its stable
Developer ID identity keeps macOS privacy grants such as Screen Recording and
Accessibility, which every ad-hoc rebuild loses.

Building production-identity candidate bytes does not publish production.
`COMET_PACKAGE_ENVIRONMENT` defaults to `production` for legacy release
compatibility; always specify `staging` for standalone staging builds. A conflicting
`COMET_DEFAULT_ENVIRONMENT` is rejected. The native staging app uses
`~/.crew-staging` and IPC port `27957`, retaining the staging bundle and permission
identity. Legacy `~/.comet-native-staging` / `27655` stays retained, not the new
app's active store. Fresh-only activation requires a new empty native root and
fresh `login`; it does not import legacy profiles or establish a writer barrier.
An occupied native root cannot be reused as fresh: use explicit `login`/`headless`
coordinates for a separate empty root instead. Profile migration requires separate
authorization and its offline import/readback gates. Packaging never imports or
launches a profile.
Packaged headed startup uses the compiled coordinates by default; CLI, environment
and ordinary saved-runtime overrides cannot redirect it. The explicit local opt-in
is `desktop-profile.json` in that compiled native data root (`~/.crew` for
production, `~/.crew-staging` for staging), containing one JSON absolute path string
to an existing **native** profile directory. Its `crew-runtime.json` must supply
Edge, Scaffold, project and nonzero IPC coordinates; deployment is optional but
must be nonblank when supplied. Invalid or incomplete selection aborts startup,
never falls back to a blank profile, and never follows a second profile pointer.
Selection preserves both runtime configuration files, including unknown fields,
and uses the existing attach-first, single-writer engine bootstrap. It does not
copy credentials, migrate legacy stores, or make a retained profile fresh. Remove
only the pointer to restore compiled defaults. `login` and `headless` ignore this
pointer and retain their explicit-coordinate behavior.

Credential-free release verification on macOS:

```sh
COMET_MACOS_SIGNING_TEAM_ID=ABCDE12345 \
  scripts/verify-macos-app.sh '/path/to/Crew Staging.app' ai.ashler.comet.staging
COMET_MACOS_SIGNING_TEAM_ID=ABCDE12345 \
  scripts/verify-macos-release.sh "$CANDIDATE_DIR" 0.2.116
```

The verifier requires one well-formed checksum entry for each expected DMG and
updater archive, then rejects ad-hoc, wrong-team, wrong-identifier,
unstable-requirement, damaged, unstapled and untrusted bundles. A sums file that
omits an artifact or repeats its entry cannot authorize it. Local experimentation
must explicitly set `COMET_MACOS_SIGNING=adhoc`; this is rejected in CI or with a
signing identity and writes only beneath `target/package/local` or
`target/package/staging/local`. Do not use local ad-hoc output to replace a signed
installation.

## Linux and Scaffold runtime

```sh
scripts/package-linux.sh
# PROFILE=debug scripts/package-linux.sh  # local fixture package
```

Produces `target/package/comet-<version>-linux-<arch>.tar.gz` for `x86_64` or `aarch64`,
containing the native `comet` and `zeron-engine` executables, Crew desktop entry/icons, licenses,
installer, and the complete `crew-web/` fixed-session browser assets. The browser
contract sidecar travels with these assets. Scaffold supplies Node and selects
the browser runtime location separately; desktop installation does not launch it.
The binary runs the GUI by default and the engine alone with `comet headless`.
Linux packaging sets both compiled environment selectors to `production`, matching
the shared artifact name, installer data root and release workflow. Any explicit
non-production selector is rejected rather than emitting a staging executable
under that identity. Staging Scaffold uses explicit headless runtime coordinates;
building these production-identity bytes does not publish or deploy production.
The bundled installer preserves immutable digest-qualified versions, reuses a
concurrent install without deleting its live directory, and installs Crew's
`comet` launcher, not a second Canary launcher. Packaging itself never installs,
starts services, writes a live profile or changes Scaffold pins.
The authenticated root installer likewise requires a unique SHA-256 entry for
the selected Linux archive before extracting or replacing the current install.

Platform image assembly extracts the native generation separately to
`/workspace/.scaffold/crew/bin/comet` and `/workspace/.scaffold/crew/web`, with
`/workspace/.scaffold/crew/runtime-ready`, data at
`/workspace/.scaffold/crew-data`, and IPC port `39400`. It must not invoke the
desktop installer or overwrite legacy session `PATH` binaries, `/opt/crew-web`
assets or snapshot data. Staging uses project/deployment `ashler-staging`, Edge
`https://comet-staging.internal.ashler.com`, and control plane
`https://scaffold-staging.internal.ashler.com`; no Canary identity or credential
copy is needed. These headless coordinates are distinct from the standalone
macOS staging profile and port.

Linux builders need the checked-in Rust toolchain, pkg-config, CMake, Wayland/X11,
xkbcommon, fontconfig/freetype, ALSA, Vulkan, WebKitGTK 4.1 and json-glib development
packages. The workflow installs these on Ubuntu 24.04 x86_64 and arm64 runners.

## Immutable candidate and publication

`release.yml` is **workflow_dispatch only**. Merges, pushes and version tags cannot
publish desktop/Linux releases. The repository has no CI test workflows: run the
relevant regression checks locally before release. The 0.2.116
cutover uses two phases. First build both signed macOS identities and Linux, then
publish the verified immutable candidate without advancing channels:

```sh
gh workflow run release.yml --repo Ashler-AI/comet --ref main \
  -f version=0.2.116 -f release_surface=desktop-and-scaffold \
  -f promotion_target=staging-candidate -f scaffold_runtime_deployment=unchanged
```

Wait for success and record that run's ID as `candidate_run_id`. Candidate-only
publication writes `releases/0.2.116/*` in the private staging bucket and stops
before the deployment guard: no moving channel/installer aliases, latest pointers
or Scaffold pins change, and no compatible deployment is claimed. All signing,
checksum, provenance and bucket-privacy gates remain enforced.

This cutover changes the release compatibility marker to
`scaffold.comet-runtime.v2` in both Crew and Platform. Use the verified versioned
manifest and Linux archive tuple to deploy compatible staging Platform support,
verify ordinary successor compatibility and its full rollback baseline, and verify
runtime/admission/browser behavior. Fresh-only activation leaves legacy stores and
writers untouched; it neither requires nor authorizes migration.
This release marker is separate from the persisted `cometRuntimeProfile.version`
wire schema, which remains `scaffold.comet-runtime.v1` in Crew, Platform and Edge.
Do not rewrite retained authority profiles or send the release marker as their version.
Then promote the exact candidate bytes, without rebuilding or re-signing:

```sh
gh workflow run release.yml --repo Ashler-AI/comet --ref main \
  -f version=0.2.116 -f release_surface=desktop-and-scaffold \
  -f promotion_target=staging -f scaffold_runtime_deployment=staging-deployed \
  -f candidate_run_id="${candidate_run_id:?Set the successful immutable candidate run ID}"
```

Wait for exact-candidate verification and channel readback. The release marker is
independent of stored profile/wire schemas, which remain v1; never rename those
schemas to v2 or point legacy snapshots blindly at a new runtime. Future
unchanged-marker releases may acknowledge `unchanged`. Do not falsely acknowledge
deployment to bypass the guard. `promotion_target=none` remains private Actions
artifacts only.

Candidate artifacts contain archive checksums, channel manifests with repository,
commit and workflow-run provenance, `provenance.json`, and Linux
`COMET_SHA256SUMS`. That last file hashes the extracted archive-root `comet`, not
the tarball; use the archive digest for image archive pins and raw digest for
exact executable verification. The candidate tarball has its own SHA-256.
Versioned objects are immutable: mismatched existing bytes or publish races fail.
Publication refuses public/all-authenticated bucket access and backward version
moves, then reads back the exact selected channel objects and pointers. All
selected staging channels pass the forward-version gate before any moving
channel writes.

Staging publishes `desktop-staging-*` for Crew Staging and `desktop-*` for the
production-identity candidate, plus `scaffold-*` when requested, **only in the
staging environment's private bucket**. Installed clients use authenticated Edge
release routes and pinned signed identities. Production promotion is a separate
explicit dispatch, approval environment and bucket; it excludes standalone staging
assets. Reuse `candidate_run_id` to promote the exact successful prior candidate,
without rebuilding or re-signing. Provenance, source ancestry, checksums and both
signed macOS variants must pass verification again. No production action is
authorized by this staging cutover.

The compatibility baseline is the moving `scaffold-manifest.json`, not the last
versioned candidate or the checked-in runtime marker. A moving v1 channel still
requires `staging-deployed` before promotion of verified v2 support.

Version 0.2.120 preserves the native-v2 fixes and hosted TCP ticket transport as a
successor to the reserved 0.2.119 artifacts from release `40216273772`. That run
passed signing and candidate verification but failed the deployment-acknowledgement
gate after immutable uploads. Preserve those bytes; the successful-source-run
reuse requirement remains unchanged. Publish 0.2.120 as `staging-candidate`, deploy
and verify its exact tuple, then promote that successful candidate run with
`staging-deployed` without rebuilding or re-signing.

The update CLI retains the sole local credential owner and its lock through
authenticated update work, resolving current tokens separately for manifest and
artifact requests, then releases both before restarting a managed daemon. Daemon
service files never capture `COMET_EDGE_TOKEN`; systemd values are escaped
literally rather than interpreted as service syntax.

### CI configuration (names only)

Public variables: `MACOS_SIGNING_TEAM_ID`, `GCP_PROJECT_ID`.
Signing secrets: `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`,
`MACOS_CODESIGN_IDENTITY`, `MACOS_NOTARY_APPLE_ID`, `MACOS_NOTARY_APP_PASSWORD`,
`MACOS_NOTARY_TEAM_ID`.
Environment-scoped publishing secrets: `COMET_RELEASES_GCS_BUCKET`,
`GCP_WORKLOAD_IDENTITY_PROVIDER`, `GCP_SERVICE_ACCOUNT`.

The staging publisher requires its effective `COMET_RELEASES_GCS_BUCKET` to equal
the independently verified existing bucket `ashler-comet-releases-staging`
exactly, before any GCS write, including immutable candidate uploads. An empty,
production or other bucket fails closed without disclosing its value. GitHub
environment names alone do not isolate storage; the operator must verify the
actual staging secret binding and bucket privacy before publication. The guard
does not change production configuration or authorize production publication.

Configure distinct `comet-release-staging` and `comet-release-production`
environments and least-privilege identities. Signing credentials are imported
only into a temporary keychain with unconditional cleanup; do not enable shell
tracing around secrets. Storage publication uses GitHub OIDC, checks required
permissions and bucket privacy, and never grants public access. The production
environment should require explicit operator approval.

### Native authority refresh

The 0.2.118 source carries the Canary credential-generation, startup deferral,
directory backpressure and inference-renewal fixes onto the existing native-v2
contract. Rejection feedback invalidates only the sent access lease; WorkOS
refresh-secret rotation remains fenced against sign-out/replacement, not against
rejection of its old access token. Mutations and provider requests are not replayed.
Version 0.2.118 keeps this source distinct from the independently built canonical
0.2.117 handoff-recovery candidate; its immutable objects must not be replaced.

Edge verification coalesces pending work only. Its active envelope is 8 KiB per
UTF-8 token, 64 KiB per JSON stream, 32 combined admissions per isolate and an
8-second caller deadline. At most 16 outbound dispatches reserve headroom for
same-isolate coordinators. Cancelled callers retain admission until underlying
work settles; one waiter cannot cancel another. Fresh batches recheck revocation,
and upstream Retry-After remains authoritative. These are active-work bounds,
not a measured isolate RSS ceiling or a global resident-coordinator count limit.

Original-epoch bootstrap rejects streamed responses over 4 MiB or more than
10,000 outcomes before local writes. Historical pending commands are not current
crash-recovery commands and cannot be reissued while terminal history syncs.
Start requires matching trusted local provenance before dispatch when history is
present; a synced marker alone is insufficient. Existing offline import support
remains separate. The server's retained-history storage has no established total
byte ceiling; the client limit does not bound that server storage.

Canonical publication still requires reviewed `main` source and the immutable
candidate workflow. A separately built Canary controller is not the canonical
Linux release, even when their version labels match. No source check authorizes existing
profile conversion, channel promotion, Edge activation or production changes.

### Checks for the integration owner

```sh
node --test scripts/package-macos.test.mjs scripts/verify-macos-release.test.mjs \
  scripts/release-workflow.test.mjs scripts/scaffold-runtime-release-guard.test.mjs
bash scripts/test-linux-desktop-entry.sh
```

Run the complete installer harness on Linux with GNU utilities. Its build preflight models Linux explicitly and stops at a fake Cargo
boundary; the subsequent installation checks exercise the real Linux utility
contracts rather than substituting macOS equivalents.

No packaging, signing, test or deployment result is implied by these instructions.
