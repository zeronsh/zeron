# Plan mode

In plan mode the agent can investigate but cannot change anything. Before it
edits a file or runs a command it presents a plan, and the user approves it,
sends it back with feedback, or hands it to another agent.

It is built on the permission policy (`PermissionMode::Plan`, see
[`plans/2026-09-30-agent-mobility-and-policy.md`](plans/2026-09-30-agent-mobility-and-policy.md),
Part 5). Under it, the shared gate allows reading, searching and read-only
commands, and refuses edits and other commands with a reason telling the
agent to present its plan.

## The plan is a question

A plan reaches the user as a question on the chat (id prefix `plan:`), so it
reuses everything questions already have: it works from any device and the
phones, the session shows *Awaiting input*, and the answer is durable in the
transcript.

- The question's text is the plan, as markdown. The desktop shows it as a
  scrolling card (headings, numbered and bulleted steps, code blocks) above
  the options; other clients show it as the question's text.
- The options are **Approve · Auto**, **Approve · Accept edits**,
  **Approve · Ask**, **Approve · Bypass** and **Keep planning**.
- A typed answer is feedback: it sends the plan back with that text.
- Each revision is a new question, so the transcript keeps how the plan
  changed.

A separate `PlanProposal` transcript part (revision history, a "hand off to
another agent" action) was the first design. It isn't needed to approve a
plan, and presets (`agent-presets.md`) are the place for hand-off, so it's left
out until something needs it.

## How a plan reaches Zeron

| Harness | How it presents a plan |
| --- | --- |
| Claude Code | Native: `--permission-mode plan`; the plan arrives as the `ExitPlanMode` tool call's `plan` input |
| Every other harness that can plan (Codex, OpenCode, the ACP agents) | The run's Zeron MCP server carries a `submit_plan {plan}` tool, and its instructions tell the agent to use it |
| Cursor, Pi | Can't plan: they never ask before acting, so the menu doesn't offer Plan |

`submit_plan` only exists in a Plan-mode chat. The engine marks the server it
hands such a run with `ZERON_PLAN_MODE=1`.

## The decision

- **Approve (mode M):**
  - The chat's saved mode becomes M (`ChatConfig.policy.mode`), on every
    device, as soon as the answer lands.
  - Claude: `ExitPlanMode` is allowed and the run's gate switches to M, so
    the same turn continues and starts executing.
  - Other harnesses: `submit_plan` returns "approved, now in M mode; end your
    turn" and the engine queues *"The plan was approved. Carry it out."* behind
    the turn. It runs as a fresh turn, because the mode changed and the
    agent that asked is still running under Plan.
- **Keep planning (feedback):** `ExitPlanMode` is denied, or `submit_plan`
  returns an error, with the feedback as the message. The agent revises and
  presents again, still in Plan mode.

## With goal mode

A goal set while the chat is planning waits, `paused (readOnly)`
([`goal-mode.md`](goal-mode.md)). Approving a plan resumes it, and its rounds
then run in the approved mode. Its verifier always runs read-only and
unattended. A goal paused for another reason stays paused.

## With native agent modes

Claude Code, Cursor and OpenCode advertise their own Build/Plan modes. If
those are exposed as model options (see the open native-interactions work),
Zeron's mode decides them: Plan selects the agent's native plan mode where it
has one, and the option is not offered separately.

## Desktop

- The composer's mode picker offers **Plan**. Shift+Tab cycles to it.
- The plan card sits in the question panel, above its options. Typing in the
  panel's text box and submitting sends the text as feedback ("Keep
  planning").
