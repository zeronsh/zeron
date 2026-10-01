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
    harness, model?, reasoning?, options,
    policy: AgentPolicy,              // mode, sandbox, network, rules
    instructions?: String,            // added to the agent's system prompt
    tools?: { allow?: [..], deny?: [..] }, // MCP servers/tools to offer
    target?: { device? | cloud? },    // where it prefers to run
    worktree: bool,                   // start in a fresh worktree
    may_spawn: bool,                  // may create chats itself
    fallbacks: [{harness, model}],    // when the harness isn't on the target
}
```

## Where presets live

- **User presets** are synced registry rows (kind `presets`), so every device
  has them.
- **Project presets** are `.zeron/agents/<id>.md` files: YAML frontmatter with
  the fields above, and the body as `instructions`. They are checked in with
  the project. A project preset with the same id as a user preset wins inside
  that project.
- **Imported presets.** `.claude/agents/*.md` files are offered read-only as
  presets with harness Claude Code (`name`, `description`, `model`, and
  `tools` as the allow list).

## Using a preset

- **A chat records the preset** as `ChatConfig.preset = {id, digest}`, along
  with the resolved harness, model, policy and instructions. Editing the
  preset later never silently changes a running chat. The chat shows "preset
  changed — update?".
- **Instructions are delivered** through each harness's own system prompt
  where it has one:
  - Claude: `--append-system-prompt`.
  - Codex: `developer_instructions`.
  - OpenCode: an agent prompt.
  - Elsewhere: prepended once to the first prompt, like the move note.
- **Tools.** The allow/deny lists filter which MCP servers the run is given.
- **Spawned chats are capped.** A chat started by another chat's agent gets
  the lower of its preset's policy and its spawner's (`AgentPolicy::capped_by`).
  Going higher needs the user.

## Surfaces

- **Desktop:**
  - a preset picker on the new-session canvas and in the composer, next to
    the model;
  - Settings → Agents to create, edit, duplicate and delete user presets,
    open a project's preset file, and import from `.claude/agents`.
- **MCP:**
  - `list_agents` returns id, name, description, harness, model and mode;
  - `create_chat { agent: "<id or name>" }` starts a chat with that preset.
- **Plan mode:** a plan card's **Hand off to…** lists presets.
