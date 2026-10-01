# Plan mode

In plan mode the agent can investigate but cannot change anything. Before it
edits a file or runs a command it presents a plan, and the user approves it,
sends it back with feedback, or hands it to another agent.

It is built on the permission policy (`PermissionMode::Plan`, see
[`plans/2026-09-30-agent-mobility-and-policy.md`](plans/2026-09-30-agent-mobility-and-policy.md),
Part 5). Under it, the shared gate allows reading, searching and read-only
commands, and refuses edits and other commands with a reason telling the
agent to present its plan.

## The plan is part of the transcript

`MessagePart::PlanProposal { id, revision, markdown, status, decided_mode? }`. It is named so it isn't confused with the agent's checklist, which the todo panel shows: ACP plans and Codex `turn/plan/updated` feed that panel ([`todo-panel.md`](todo-panel.md)):

- `status` is one of `proposed`, `approved`, `revised` or `rejected`.
- The plan renders as a card, with its markdown and, while `proposed`, these
  actions:
  - **Approve**, with a mode to continue in (Auto by default, or Accept
    edits, Ask or Bypass);
  - **Keep planning**, with feedback text;
  - **Hand off to…** another agent preset, once presets exist.
- Every revision is a new part; the earlier one becomes `revised`. That keeps
  the history of how the plan changed.

## How a plan reaches Zeron

| Harness | How it presents a plan |
| --- | --- |
| Claude Code | Native: `--permission-mode plan`; the plan arrives as the `ExitPlanMode` tool call's `plan` input |
| OpenCode | Native `plan` agent; the plan is its final message of the turn, submitted through `submit_plan` when instructed |
| Every other harness | Emulated: the gate holds Plan semantics, the run gets a one-time instruction (like the move note) to present the plan by calling the Zeron MCP tool `submit_plan {plan}` |

Every run already has the Zeron MCP server, so `submit_plan` works for
any harness.

## The decision

A plan is a pending input on the chat (question id prefix `plan:`, carrying
the plan's part id). It reuses the existing question bridge, so the decision
works from any device and phone, and the session shows *Awaiting input*.

- **Approve (mode M):**
  - The chat's `ChatConfig.policy.mode` becomes M.
  - Claude: `ExitPlanMode` is allowed and the run's gate switches to M, so
    the same turn continues and starts executing.
  - Emulated harnesses: `submit_plan` returns "approved; the user switched to
    M", the turn ends, and the host queues *"Execute the approved plan."* as
    the next turn under M. That is a fresh runtime, since the mode changed.
- **Keep planning (feedback):** `ExitPlanMode` is denied, or `submit_plan`
  returns, with the feedback as the message. The agent revises and presents
  again.
- **Hand off to preset P:** approve, then start a new chat (or switch this
  one) with preset P, giving it the plan as its first prompt.

## With goal mode

A goal set while the chat is planning waits, `paused (readOnly)`
([`goal-mode.md`](goal-mode.md)). Approving the plan resumes it. The goal's
rounds then run in the approved mode. Its verifier always runs read-only and
unattended.

## With native agent modes

Claude Code, Cursor and OpenCode advertise their own Build/Plan modes. If
those are exposed as model options (see the open native-interactions work),
Zeron's mode decides them: Plan selects the agent's native plan mode where it
has one, and the option is not offered separately.

## Desktop

- The composer's mode picker offers **Plan**. Shift+Tab cycles to it.
- The plan card lives in the transcript, and the question panel shows the
  same three actions.
- While a plan is pending, the composer's send turns into **Keep planning**,
  which sends the text typed as feedback.
