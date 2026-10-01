# Agent presets

A preset is a named agent: a harness, model and permission policy, plus
instructions and tools. "Reviewer" is Codex with reasoning set high, in Ask
mode, with "only report, never edit". "Fast fixer" is Claude Sonnet in Auto.
Pick one when starting a chat, or let an orchestrating agent spawn one by
name.

Plan: [`plans/2026-09-30-agent-mobility-and-policy.md`](plans/2026-09-30-agent-mobility-and-policy.md),
Part 3.

## Shape

```rust
AgentPreset {
    id, name, description,            // the description tells orchestrators when to use it
    icon?, color?,
    harness, model?, reasoning?, model_options,
    policy: AgentPolicy,              // mode, sandbox, network, rules
    instructions?: String,            // added to the agent's system prompt
    tools: { allow, deny },           // which of Zeron's own MCP tools it is offered
    worktree: bool,                   // start in a fresh worktree
    may_spawn: bool,                  // may create chats itself
    fallbacks: [{harness, model?}],   // when the harness isn't on the device
    source: user | project | imported,
}
```

There is no `target` (device or cloud) yet: it arrives with continuing in the
cloud, since nothing else could use it.

## Where presets live

- **User presets** are `{data_dir}/presets.json` on the device, edited in
  Settings → General → Agent presets. They are **per device for now**. The plan was a
  synced registry row, which needs a new row kind on the edge as well as in
  the engine; that is the follow-up.
- **Project presets** are `.zeron/agents/<id>.md` files, checked in with the
  project: flat frontmatter, then the instructions as the body.

  ```text
  ---
  name: Reviewer
  description: Reviews a diff and reports; never edits.
  harness: codex
  model: gpt-5
  reasoning: high
  mode: ask               # bypass | auto | accept-edits | ask | plan
  sandbox: read-only      # off | workspace-write | read-only
  network: false
  tools: read_chat, get_chat    # allow list of Zeron MCP tools; deny: for the rest
  may_spawn: false
  worktree: no
  fallbacks: claude-code/sonnet, opencode
  ---
  Only report what you find. Never edit a file.
  ```

  A project preset with the same id as a user preset wins inside that
  project. Keys it doesn't know are ignored.
- **Imported presets.** `.claude/agents/*.md` files are offered read-only as
  presets with harness Claude Code. Only the name, description, model and
  prompt carry over: Claude's `tools` names are Claude's, not Zeron's.

## Using a preset

- **A chat records the preset** as `ChatConfig.preset = {id, name, digest,
  instructions, may_spawn, tools}` along with the resolved harness, model and
  policy. Editing the preset later never changes a running chat. The digest
  says whether it has been edited since; the UI doesn't yet offer "update".
- **Instructions are delivered** through the harness's own system prompt where
  it has one (`Harness::delivers_instructions`):
  - Claude Code: `--append-system-prompt`.
  - Codex: `developerInstructions` on `thread/start`.
  - Everywhere else (OpenCode, ACP agents, Cursor, Pi): prepended once to the
    first prompt of a session.
- **Tools.** `allow`/`deny`/`may_spawn` limit what the agent is offered
  *through Zeron's own MCP server* (the engine sets them in its environment;
  `create_chat` and `create_chats` go when `may_spawn` is false). Other MCP
  servers are the agent's own business.
- **Spawned chats are capped.** A chat started by another chat's agent gets
  the lower of its preset's policy and its spawner's (`AgentPolicy::capped_by`).
  Going higher needs the user.
- **Fallbacks.** When the preset's harness isn't offered on the device, the
  first fallback that is offered is used, with its own model.
- `worktree` is honoured on the desktop's new-chat canvas (it selects "New
  worktree"). Chats created through MCP use the project folder.

## Surfaces

- **Desktop:**
  - an **Agent** chip on the new-session canvas, beside the device and project
    chips: picking a preset sets the harness, model, reasoning and mode;
  - Settings → General → **Agent presets** to create, edit, duplicate and delete user
    presets, and to see the project's and imported ones read-only.
- **MCP:**
  - `list_agents` returns id, name, description, source, harness, model and mode;
  - `create_chat { agent: "<id or name>" }` starts a chat with that preset;
    `harness`, `model`, `reasoning` and `mode` given alongside override it.
