# Glitch Flow prototype

This first iteration is a native Rust desktop app built with GPUI. It starts
from [Zeron](https://github.com/zeronsh/zeron) at commit `a9a78d7`, then applies
a restrained graphite palette and a minimal Glitch Flow identity. Zeron's MIT
license remains in `LICENSE`.

The reference video informed the visible shell: a compact project and session
sidebar, a conversation with activity rows, a pinned composer, a file explorer,
an editor, and anchored menus. Text shown inside the video is example content,
not a command or product requirement.

## Windows demo

Build requirements are the same as Zeron's
[Windows development guide](docs/reference/windows-development.md): stable MSVC
Rust, Visual Studio C++ build tools, Windows SDK, CMake, and Git for Windows.

Run `scripts/dev-demo.ps1` from PowerShell. It builds the app and RPC probe,
starts a local mock engine, seeds a small Git workspace in
`ignore/prototype-demo`, and opens the headed app. Closing the app stops the
daemon started by the script. The demo requires no account or agent credentials.

For normal local use, run `cargo run --locked -p glitch-flow`. The executable,
package names, UI copy, and window title say Glitch Flow. Shared Rust library
crate identifiers still follow the upstream source.

## Scope of this iteration

- Native desktop UI with real pane, menu, sidebar, composer, and editor code.
- Native Tasks with persistent boards, epics, issues, subissues, comments,
  list/board views, and links to agent chats and their child threads.
- Agent-facing task operations through the app's MCP server for creating,
  searching, updating, commenting, and linking tickets.
- Dark neutral default palette; semantic error and status colors remain.
- Offline sample conversations for visual and interaction review.
- A new-session branch picker that can create a branch through Git on the
  selected device. Local mode checks out the branch; New worktree mode leaves
  the shared checkout alone and creates the isolated checkout when the run
  starts.
- A selected project determines which device hosts the chat and agent run.
  The composer names that host before sending.
- GitHub pull requests discovered from a checkout through the host's `gh`
  installation, plus persistent PR URLs attached to a conversation. A
  conversation can show multiple PRs across branch changes.

To attach PRs, right-click a conversation and choose **Manage pull requests…**.
Enter one GitHub PR URL per line; use Shift+Enter for another line. The same
dialog shows whether discovery can use `gh` on that conversation's host.

The sample agent is scripted. GitHub discovery uses the host's existing `gh`
authentication; run `gh auth login` on that host to connect it. Cloud sync and
remote-device execution require an explicitly configured edge and
auth (`ZERON_EDGE_URL` plus `ZERON_WORKOS_CLIENT_ID` or a development
`ZERON_EDGE_TOKEN`); this prototype
does not point at Zeron's production account service. Managed cloud compute
is not implemented. Distribution packaging and macOS/Linux visual matching
have not been validated for this prototype.
