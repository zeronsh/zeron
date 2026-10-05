# Permission modes

Every chat has a permission mode; Bypass is the default. The host sends the
policy to the selected harness, which answers the native CLI's approval
requests through Zeron's shared gate.

| Mode | What the gate allows without asking |
| --- | --- |
| Bypass | Everything |
| Auto | Reads, read-only commands, project edits, and everyday development commands; other commands ask, and recognized destructive commands are refused |
| Accept edits | Reads, read-only commands, and project edits; other actions ask |
| Ask | Reads and read-only commands; other actions ask |
| Plan | Reads, searches, and read-only commands; edits and other commands are refused |

Codex sends shell commands through launchers such as `/bin/zsh -lc 'cat a.txt'`.
The harness unwraps an exact `sh`, `bash`, `zsh`, or `dash` `-c` / `-lc` launcher
before evaluating the script or building an approval question and its standing
rule. Both string and argv-array requests are supported. Extra arguments,
outer shell operators, or expansions in the launcher are not stripped. Scripts
with substitutions, file redirects, or unknown commands are not automatically
classified as reads. A shell heredoc used to edit a file still asks in Accept
edits; it is a command, not a file-change approval.

These modes describe Zeron's decisions for requests it receives. Native CLIs
can allow some actions without sending an approval request. For example,
Claude Code can run `git status | head -n 1` in Ask or Accept edits without a
`can_use_tool` request; Codex also has a native safe-command set. Standing rules
only apply to requests that reach the gate. Bypass does not cause every action
to consult those rules. The gate's command checks are conservative lexical
checks, not an OS sandbox.

Claude Plan mode is its native planning mode: project edits are held, but
Claude can write its own plan artifacts under `~/.claude/plans/`. The native
plan-file exception does not grant arbitrary project or outside-project edits.
Codex Plan uses its native read-only sandbox plus Zeron's gate. Cursor and Pi
support Bypass only because they do not expose a permission gate.

Changing the mode restarts the agent process on the next turn. Codex's decline
response has no reason field. A model can also decline a requested action
without making a tool call; that is not evidence of a Zeron policy decision.
