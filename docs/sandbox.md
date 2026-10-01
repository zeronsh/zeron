# Sandboxing agent processes

Crate: `crates/sandbox` (`zeron-sandbox`). Plan:
`docs/plans/2026-09-30-agent-mobility-and-policy.md`, Part 4.

A chat's `AgentPolicy` carries a sandbox mode and a network switch. The same
policy holds on every OS. A WASM sandbox can't host the native agent CLIs,
so the layer that stays the same everywhere is the *policy*, not the
mechanism. Each OS enforces it with its own backend around the agent CLI
process. The agent's tools, subagents and MCP servers are its children, so
they inherit the confinement.

## Modes

| Mode | Writes allowed | Reads |
| --- | --- | --- |
| `Off` | anywhere (no sandbox) | anywhere |
| `WorkspaceWrite` | workspace, temp folders, the agent's own state | anywhere except hidden paths |
| `ReadOnly` | temp folders, the agent's own state | anywhere except hidden paths |

With **network off**, the agent can still reach loopback. It has to, because
the `zeron mcp` server it spawns dials the engine at
`ws://127.0.0.1:$ZERON_IPC_PORT`. How much of loopback stays open depends on
the OS (see below). Pass the port as `SandboxSpec.loopback_ports`.

When entries overlap, `hidden` wins over `read_only`, and `read_only` wins
over writable.

## What the spec contains

`SandboxSpec::for_agent(harness, policy, workspace, home)` builds the spec a
spawn site needs:

- **Writable state** (`default_agent_paths`). Each harness gets only its own
  state:
  - Claude Code: `~/.claude` (or `$CLAUDE_CONFIG_DIR`), the
    `~/.claude.json*` prefix, and its `claude-cli-nodejs` cache.
  - Codex: `$CODEX_HOME`.
  - Cursor: `~/.cursor`, `~/.config/cursor`, `~/.zeron/cursor-state`.
  - OpenCode: its XDG data, config, cache and state folders, plus
    `~/.opencode`.
  - Pi: `~/.pi`.
  - Grok: `$GROK_HOME`.
  - Hermes: `$HERMES_HOME` and the XDG `hermes` folder.
  - Devin: the XDG and Application Support `devin` folders.
  - Antigravity: `$GEMINI_HOME`.
- **Temp folders**, which the backend always adds: `/tmp`, `/var/tmp` and
  `$TMPDIR`. On macOS this means `/private/tmp`, `/private/var/tmp`, and the
  per-user `confstr(_CS_DARWIN_USER_TEMP_DIR)` and cache folders. On Linux it
  also includes `/dev/shm`.
- **Read-only carve-outs**. These are places where an agent's write would run
  code *outside* the sandbox later:
  - The repository's `.git/hooks` and `.git/config` (a hook, or
    `core.fsmonitor`, runs the next time Zeron or the user runs git). For a
    linked worktree, the common git dir is included as well.
  - Claude's `settings.json`, `commands` and `agents`.
  - Codex's `config.toml` (MCP servers, `notify`) and `packages` (the CLI
    binary).
- **Hidden** paths:
  - The common secret stores:
    - Keys: `~/.ssh`, `~/.gnupg`.
    - Cloud and cluster credentials: `~/.aws`, `~/.azure`, gcloud,
      `~/.kube`, `~/.docker/config.json`.
    - Plaintext token stores: `.netrc`, `.git-credentials`.
    - Registry publish tokens: `.npmrc`, `.pypirc`, cargo credentials.
    - Password managers: `pass`, `op`, 1Password.
  - Browser profiles and cookies.
  - The macOS Keychain folder, except for harnesses that keep their login
    there (Claude Code, Cursor, Antigravity, and the unverified ACP agents).
  - Linux keyrings.
  - Zeron's own account vault (`~/.zeron/agent-accounts`,
    `~/.zeron/session.json`).
  - **Every other agent's credentials**: a sandboxed Codex can't read
    Claude's login, and the reverse.
- **Git paths** (`git_paths`). If the workspace is a linked worktree or a
  subfolder of a repository, its git dirs are made writable in
  `WorkspaceWrite`, so commits keep working.

`toolchain_cache_paths` lists package-manager caches (`~/.npm`,
`~/.cargo/registry`, …). It is opt-in only: a writable cache that holds code
lets a confined agent plant code that other projects build unconfined.

## Backends

`wrap(spec, program, args)` returns the command to spawn and an
`Enforcement` report. The report says which parts of the spec actually hold
on this machine; `is_complete()` is true when all of them do, and `notes`
lists the caveats.

### macOS: Seatbelt

`/usr/bin/sandbox-exec -p <profile> -- <exe> <args…>`. `sandbox-exec` is
always taken from that absolute path, never from `PATH`.

The profile is closed by default. Rules are written broad to narrow, because
Seatbelt applies the last matching rule:

1. Allow reads everywhere.
2. Allow writes to the writable roots and prefixes.
3. Deny writes to read-only carve-outs. Folders above a carve-out can't be
   renamed, so the carve-out can't be moved away.
4. Deny reads and writes on hidden paths.

On top of that:

- It allows the mach services a Node, Bun or Rust CLI needs to start, log in
  and use TLS: the Keychain, trustd, DNS configuration and cfprefs.
- Launch Services, `lsopen` and Apple Events stay closed. Without that,
  `open` and `osascript` could start programs outside the sandbox.
- With network off, outbound traffic is limited to `localhost:*`, plus Unix
  sockets inside the writable roots. Bind and inbound are limited to
  loopback.

Every path is canonicalised (`/tmp` → `/private/tmp`, symlinks resolved) and
written as an SBPL string literal with `\` and `"` escaped. Paths that
contain control characters are refused.

The profile was adapted from two sources. Both are Apache-2.0 and both derive
from Chromium's sandbox policy:

- OpenAI Codex, `codex-rs/sandboxing/src/seatbelt_base_policy.sbpl` and
  `seatbelt_network_policy.sbpl`. Taken from it: the process, sysctl, pty,
  device and platform rules; the `system-fcntl` deny; ancestor-rename
  protection.
- Anthropic `sandbox-runtime`, `src/sandbox/macos-sandbox-utils.ts`. Taken
  from it: the mach-lookup and IOKit list; the loopback rules, and why
  bind/inbound use `local ip`; string escaping.

Tested against real processes on macOS 27 (`cargo test -p zeron-sandbox`):

- Workspace writes succeed. Writes outside it fail with `EPERM`.
- Hidden files and folders can't be read, listed or written. Other reads
  work.
- `ReadOnly` refuses workspace writes. Temp folders stay writable in both
  modes.
- `.git/hooks` and `.git/config` can't be written, and `.git` can't be
  renamed. `git commit` still works.
- Atomic replace via a prefix sibling works.
- A path that tries to inject SBPL stays a single literal.
- With network off:
  - Loopback and the agent's own loopback servers stay reachable.
  - `curl`, TCP by IP and UDP to the internet fail.
- With network on, `curl https` works.
- `launchctl submit` can't start an unconfined job.
- `node` starts, spawns children and does `fetch` over TLS.
- The installed CLIs work under the spec their spawn site would build:
  - `claude --version` and `claude auth status`: still logged in through
    the Keychain.
  - `codex --version` and `codex login status`.
  - `pi --version`.
  - Claude in stream-json mode and `codex app-server` both answer their
    initialize handshake with network off.

**Nested Seatbelt is refused.** macOS won't apply a *different* profile
inside a sandboxed process (`sandbox_apply: Operation not permitted`). While
Zeron's sandbox is on, a harness's own tool sandbox has to stay off: Codex
`danger-full-access`, Claude Code without `sandbox.enabled`. The outer
profile still confines those tools.

### Linux: bubblewrap (preferred when installed and working)

`bwrap` builds a private mount namespace:

- `--ro-bind / /`.
- A minimal `--dev`, plus the host's `/dev/shm`.
- A fresh pid and ipc namespace with its own `/proc`.
- Writable roots bound back read-write.
- Carve-outs bound read-only on top.
- Hidden folders covered by an empty read-only tmpfs; hidden files covered
  by `/dev/null`.
- `--new-session`, `--die-with-parent`, `--cap-drop ALL`.

Unlike Landlock, it can deny below a grant, so the whole filesystem spec
holds for paths that exist at spawn time. Bind mounts need a target that
exists, so writable folders are created first.

Network off works one of two ways:

- **No loopback port needed:** `--unshare-net`. The agent only has its own
  loopback.
- **Loopback ports needed:** `--unshare-net` would cut the agent off from the
  engine's port. Instead the namespace is shared, and the Landlock helper
  runs inside bwrap with `--net-only` (see below).

Availability is probed once by running `bwrap` with exactly these namespace
flags. Unprivileged user namespaces may be disabled, or restricted by
AppArmor.

### Linux: Landlock + seccomp (fallback)

The spawn becomes `<zeron> sandbox-exec --spec <json> -- <exe> <args…>`. The
helper mode does the following, then `execve`s the agent:

1. Sets `no_new_privs`.
2. Applies a best-effort Landlock ruleset sized to the kernel's ABI.
3. Installs the seccomp filter when the network is off.

The pid stays the same, and the restrictions carry across `exec`. If the
helper can't confine the process, it exits 126 rather than run the agent
unconfined.

**Filesystem.** Landlock is allow-list only, so hidden paths are handled by
*splitting*: walk from `/` towards each hidden path and grant every sibling
on the way instead of the parent. Writable roots are split the same way.
Directory listing is granted on `/`. Symlinks are skipped, because Landlock
checks the resolved path, and the split already covers it at its real
location.

**Network off.**

- seccomp refuses every socket that isn't Unix or netlink, and refuses
  io_uring (it creates sockets without `socket(2)`).
- If loopback ports are needed, TCP stream sockets are allowed, and Landlock
  ABI v4 (Linux 6.7+) limits `connect` to those ports and `bind` to nothing.
  UDP stays refused.
- ABI v6 scopes signals, and with network off also abstract Unix sockets, to
  the sandbox.
- ABI v9 limits pathname Unix-socket connects to the writable roots when the
  network is off.

The seccomp filter is adapted from Codex `codex-rs/linux-sandbox/src/landlock.rs`
(Apache-2.0). Dependencies: `landlock` (MIT/Apache-2.0) and `seccompiler`
(Apache-2.0/BSD-3-Clause).

The Linux tests live in `tests/linux.rs`. That binary is its own helper. It
compiles from macOS with `cargo check --target x86_64-unknown-linux-gnu`, and
runs only on Linux.

## Limitations

- **Windows is unsupported for now.** With a mode other than `Off`, `wrap`
  returns `SandboxError::Unsupported("Windows")`. AppContainer is the
  planned backend.
- **Linux, loopback ports:** Landlock filters TCP by port, not by address. A
  listed port is reachable on *any* host. Agents can't listen on TCP when the
  network is off.
- **Linux, older kernels:** below Landlock ABI 4, network off can't be
  enforced when loopback ports are needed, and the report says so. Below
  ABI 3, truncation outside the writable areas isn't restricted. With ABI 1,
  cross-folder renames are refused even inside the workspace.
- **Landlock, carve-outs and hidden paths:**
  - Read-only carve-outs aren't enforced (reported).
  - Names inside hidden folders can be listed.
  - New entries created next to a hidden path after spawn (new files
    directly in `$HOME`) are unreadable until the next start.
  - A hard link to a hidden file isn't hidden.
- **Atomic-replace files** such as `~/.claude.json`: on Linux they can only
  be written in place. Claude Code's temp-file-plus-rename update of
  `~/.claude.json` is refused there. Setting `CLAUDE_CONFIG_DIR` moves the
  file into a writable folder.
- **Writable agent state** can still be used to plant config that runs on
  the next *unsandboxed* launch of the same CLI, for example MCP servers in
  `~/.claude.json`. The read-only carve-outs only cover the most direct
  hooks.
- **Hidden-path trade-offs:**
  - With `~/.ssh` hidden, git over SSH fails. HTTPS through `gh` or a
    credential helper still works.
  - `gh`'s token (`~/.config/gh/hosts.yml`) is *not* hidden, because hiding
    it breaks `gh` entirely.
  - Change `hidden` per user if needed.
- **Not protected:** the `.git` carve-outs don't cover a repo-local
  `core.hooksPath` (such as `.husky`), and they don't cover editor or tool
  configs that run code when the user opens the repo (`.vscode`, `.envrc`).
  `git init` in a new folder works; there's nothing to protect yet.
- **Bubblewrap:** the agent sees only its own processes. Stopping `bwrap`
  kills the agent with SIGKILL, with no graceful SIGTERM; closing stdin
  remains the clean shutdown path.
- **Future: a Linux microVM backend** (Virtualization.framework on macOS,
  WSL2 on Windows, namespaces on Linux) would behave identically on every OS,
  because the agent always runs on the same Linux. A network namespace with a
  loopback relay for the engine port would also remove the port-only caveat.

## Wiring

- **Picking it.** The composer's permissions menu has a **Sandbox** section
  below the modes: *No sandbox* (the default), *Workspace write* and
  *Read-only*. The choice is stored on the chat (`ChatConfig.policy.sandbox`)
  and the mode chip shows a small icon while it's on. Choices this device
  can't provide are greyed with the reason.
- **What each harness offers.** `Harness::policy_caps().sandboxes` is
  `zeron_harness::sandboxing::os_sandboxes()`: all three where
  `best_backend()` finds one, otherwise only *No sandbox*. Codex keeps its
  native sandbox as the fallback, so it offers all three everywhere.
- **Dispatch.** The host refuses an attended run that asks for a sandbox its
  harness can't have, with the reason in the transcript. An unattended run (a
  goal's verifier, a child ask) drops the sandbox instead; Plan's rules still
  keep it read-only.
- **Spawn.** Every agent spawn site for a run asks
  `zeron_harness::sandboxing::agent_command` (or `policy_command`) for its
  `Command` instead of `Command::new(exe)`:
  - Claude Code, Codex (`app-server`), the OpenCode server, ACP agents,
    the Cursor shim and Pi.
  - It builds `SandboxSpec::for_agent`, opens the run's `ZERON_IPC_PORT` on
    loopback, wraps the executable and adds the wrapper's and the agent's
    environment. Whatever the site adds afterwards (arguments, env, folder,
    stdio) applies to the agent unchanged.
  - Probes, model listing and sign-in run unconfined.
  - A wrap failure refuses the run ("This chat asks for a … sandbox, but it
    couldn't be applied: …"). It is never run unconfined.
- **Codex.** While Zeron's sandbox confines it, Codex's own sandbox is
  `danger-full-access` (nested Seatbelt is refused). Its approvals still go
  through the policy.
- **The Landlock helper.** `apps/zeron` calls
  `zeron_sandbox::run_helper_if_requested()` first thing in `main`, so the
  app binary is its own `sandbox-exec` helper.
