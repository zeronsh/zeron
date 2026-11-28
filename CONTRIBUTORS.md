# Contributing to Zeron

Thanks for helping build Zeron. This guide covers how to set up, what a good
pull request looks like, and the handful of rules that keep a local-first,
multi-device app working across versions.

## Before you start

- For anything larger than a focused fix, open an issue or draft PR first so
  the approach can be agreed before you invest in it.
- Keep a pull request to one concern. A bug fix, a refactor, and a new feature
  are three PRs.
- Read [ARCHITECTURE.md](ARCHITECTURE.md) for the crate map and data model, and
  [CONTEXT.md](CONTEXT.md) for domain vocabulary. `docs/` holds design notes for
  most subsystems (sync, previews, performance, themes); check there before
  redesigning something.

## Setup

The toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml)
(stable, with `rustfmt` and `clippy`).

**Linux** needs the desktop system libraries CI installs:

```sh
sudo apt-get install -y libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libxcb1-dev libx11-xcb-dev libfontconfig1-dev libfreetype-dev \
  libasound2-dev libvulkan-dev pkg-config cmake libwebkit2gtk-4.1-dev libjson-glib-dev
```

(Add `mesa-vulkan-drivers` if you run GPU visual tests without a hardware GPU.)

**macOS** needs Xcode and **Zig 0.16** (`brew install zig@0.16`, used to build
Ghostty for the panel's terminals in `apps/zgx`; cached in
`~/Library/Caches/zgx-ghostty` after the first build). **Windows** is covered in
[docs/reference/windows-development.md](docs/reference/windows-development.md).

### Running the desktop app (`apps/zgx`)

The shipping desktop app lives in [`apps/zgx`](apps/zgx/README.md) (its own
Cargo workspace) and talks to `zeron-engine` built from the root workspace:

```sh
cargo build -p zeron --bin zeron-engine
cd apps/zgx
cargo run --release              # intro / onboarding / main window
cargo run --release -- main      # straight to the main window
cargo run --release -- gallery   # design-system workbench (light + dark)
```

Edits under `apps/zgx/ui/` hot-reload in place without rebuilding Rust.
Dev builds default to the staging engine (`~/.zeron-staging`, port `27871`) and
refuse the production data dir and port so they never touch your installed
Zeron. Override with `ZERON_ZGX_ENGINE_PORT`, `ZERON_ZGX_ENGINE_DATA`, and
`ZERON_ZGX_ENGINE_HOME` for a throwaway engine (see
[`apps/zgx/README.md`](apps/zgx/README.md)).

### Running the engine or headless CLI (`apps/zeron`)

An installed daemon holds the default data directory and IPC port. Give a
standalone engine or headless CLI run its own so the two never share state:

```sh
ZERON_DATA_DIR=~/.zeron-dev ZERON_IPC_PORT=27700 cargo run -p zeron --bin zeron-engine
# Or via the CLI binary (`apps/zeron` opens no window; use `apps/zgx` for desktop):
ZERON_DATA_DIR=~/.zeron-dev ZERON_IPC_PORT=27700 cargo run -p zeron -- headless
```

Useful knobs for exercising the UI without a real agent or account:

| Variable | Effect |
| --- | --- |
| `ZERON_ZGX_MOCK=1` | Offers the engine's mock agent in `apps/zgx`'s model picker |
| `ZERON_HARNESS=mock` | Offers the mock harness on the engine, streaming canned turns |
| `ZERON_MOCK_SUBAGENT=1` | Mock turns spawn subagents |
| `ZERON_MOCK_THINKING=1` | Mock turns include markdown-heavy thinking |
| `ZERON_MOCK_MERMAID=1` | Mock turns include Mermaid diagrams (simple and large flowcharts, sequence, state, class) |
| `ZERON_MOCK_TODO=1` | Mock turns work through an 8-item checklist (pair with `ZERON_MOCK_DELAY_MS=900` to watch the todo panel) |
| `ZERON_MOCK_BACKGROUND=1` | Mock turns leave three background processes running (printing output, stoppable from the composer's background tray) |

## Tests

Run the suites for the areas you touched before opening a PR. These mirror CI:

```sh
# ZGX desktop app (Rust host unit tests, integration tests, and Luau UI tests):
scripts/ci/test-zgx.sh rust
scripts/ci/test-zgx.sh tests
scripts/ci/test-zgx.sh ui        # requires `zgx` CLI; see apps/zgx/README.md

# Root workspace crates:
cargo test --locked -p zeron-engine --lib
cargo test --locked -p zeron-harness            # includes tests/ fixtures, not just --lib
cargo test --locked -p zeron-sync --lib
cargo test --locked -p zeron-preview
```

- CI does not cover every crate on every platform. For engine, harness, and sync
  changes, the local suite is the gate, so say in the PR what you ran.
- Every bug fix comes with a test that fails without the fix. Every behavior
  change updates or adds the tests that pin it.
- Tests must be deterministic and offline: no real network, no real agent
  CLIs, no reliance on your home directory. Use the in-memory RPC client,
  fixtures under `tests/`, and temp dirs.
- A catalog or protocol change often touches both `--lib` tests and
  integration fixtures under `crates/*/tests/`. Run the whole crate.

## Code

- Match the surrounding code: its naming, its idioms, and its comment density.
  Comments explain *why*, not what.
- `cargo clippy --all-targets` should introduce no new warnings in code you
  touched.
- Format only your own changes. Parts of the tree are not rustfmt-clean, and
  whole-file reformatting buries the real diff. Drop formatting-only hunks in
  code you didn't otherwise change.
- Don't add a dependency without saying why in the PR. Dependencies ship to
  every user's machine.
- Don't bump the version. Releases are cut by maintainers; the version lives
  only in `[workspace.package]` in `Cargo.toml`.

### Compatibility across versions

Zeron runs on several devices at once, and they update independently. A UI may
talk to an older local daemon, and a chat may be hosted on a remote device
running an older engine. So:

- New fields on wire and document types (`zeron-proto`, `zeron-doc`) must be
  optional or `#[serde(default)]`. Older peers must still parse newer frames,
  and newer code must accept frames without the field.
- New behavior that needs the other side's cooperation is gated on a
  capability (`zeron_proto::capabilities`) or a device version check. Features
  degrade when a peer lacks them; they don't fail.
- Never rename or repurpose persisted keys, RPC method names, or document
  fields. Add new ones instead.
- Some infrastructure (Cloudflare worker, R2 buckets) keeps its old `comet`
  name on purpose. Don't rename it.

### Agent harnesses

- New harness integrations follow the existing drivers in `crates/harness`.
  Installs are explicit user actions and are documented in
  [crates/harness/README.md](crates/harness/README.md).
- Only drive agent CLIs in ways their terms of service allow.

## UI changes

- Desktop UI work lives in `apps/zgx/ui/` (Luau) and `apps/zgx/src/` (Rust
  host). Follow the design system rules and patterns in
  [`apps/zgx/README.md`](apps/zgx/README.md): theme tokens only (no hard-coded
  colors/spacing/radii), concentric corners, even container insets, and
  in-place state updates (`ui.each`, live functions/states — never rebuild
  subtrees on data change).
- Reuse the app's existing `@ds` components and idioms exactly: rows, menus,
  seams, hover treatments, empty states. Prototype new shared components in
  `gallery` first. Run `zgx lint ui` and `zgx check ui` on Luau changes.
- Do what the change calls for and nothing more. Unrequested restyling of
  nearby UI gets reverted in review.
- Check light and dark mode, frosted and opaque surfaces. Hover and selection
  on glass lift toward white, never a dark wash.
- Menus mark the selected row with a wash, not a check glyph. Dropdowns open
  below their trigger.
- The bundled Geist font lacks most exotic whitespace (em space, thin space,
  and so on). Reserve space with characters it actually has.
- Keep user-visible strings localized via `ui.t(...)` (`apps/zgx/docs/i18n.md`).

## Pull requests

**Title:** a short imperative summary, for example
`Explorer: newest subagents first`.

**Description:** two sections.

- **Summary:** what changed and why, in a few bullets. Call out anything
  reviewers should look at closely, such as compatibility, migrations, or
  behavior that changed on purpose.
- **Test plan:** the tests you added and the suites you ran, as a checklist.
  List manual checks you did, and any you didn't do.

**Screenshots are required for every visible change.**

- Show the new state, and a before/after for changes to existing UI. Include
  light and dark mode when colors or surfaces are involved. Use a short
  recording for animation, drag, or scrolling changes.
- Drag the images into the PR description so GitHub hosts them as attachments.
  **Never commit screenshots to the repository** for a PR.
- Redact personal data before uploading: email addresses, account names,
  tokens, private repo names, and home-directory paths. Pixelate them; don't
  just crop around them.

**Keep it reviewable:**

- Keep the branch mergeable with `main`. PRs are squash-merged, so the PR
  title and description become the commit.
- Respond to review by pushing new commits rather than force-pushing over
  history the reviewer already read.
- CI must pass, or the failure must be a known flake that you name in the PR.

## Security

- Never commit secrets, tokens, or real account data, including in fixtures,
  screenshots, and logs.
- Code that spawns processes, handles credentials, or opens network listeners
  gets extra scrutiny in review. Explain in the PR why it needs to exist.
- To report a vulnerability, contact a maintainer privately. Please don't open
  a public issue.

## License

Zeron is [MIT licensed](LICENSE). By contributing, you agree that your
contributions are licensed under the same terms.
