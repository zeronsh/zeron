# Glitch Flow

Glitch Flow is a native Rust desktop prototype for working with coding agents. It
uses GPUI for the conversation, workspace, file, editor, and Git views. The
default appearance is a restrained graphite theme.

## Try the Windows demo

From PowerShell in this repository:

```powershell
.\scripts\dev-demo.ps1
```

The script builds the app, starts an isolated local mock engine, seeds a small
Git project, sample conversations, and a task board, then opens Glitch Flow. Closing the app stops
the demo engine. No agent account or cloud service is needed for this demo.

For development outside the seeded demo, run `cargo run --locked -p glitch-flow`.
The local app command and packages use `glitch-flow`. Shared Rust library crate
names retain their upstream identifiers for now.

## Current scope

- Local conversations, agent selection, and agent-supplied model choices.
- Native Tasks with boards, epics, issues, subissues, comments, list/board views,
  and links to agent conversations. Agents can manage the same records through
  the app's MCP tools.
- Branch and worktree selection for a conversation's Git workspace.
- Multiple GitHub pull request links on one conversation. Discovery uses `gh`
  authenticated on the selected device; links can also be attached manually.
- Optional sync with connected devices through an explicitly configured edge.
  This prototype does not connect to Zeron's production account service or
  provide managed cloud compute.

For GitHub sign-in and multi-device sync, see [Glitch Flow cloud setup](docs/production-cloud.md).
The service address is packaged with a configured release; users sign in on
each computer with their own GitHub account.

See [PROTOTYPE.md](PROTOTYPE.md) for the demo workflow and current limitations.

## Origin and license

Glitch Flow currently builds on [Zeron](https://github.com/zeronsh/zeron) commit
`a9a78d7`. The existing source and its changes retain Zeron's
[MIT license](LICENSE) and [third-party notices](THIRD_PARTY_NOTICES.md).
Glitch Flow's name and new icon identify this prototype; they do not change the
provenance of the underlying code.
