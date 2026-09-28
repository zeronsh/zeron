# Environments

Where a chat runs: which device, which project, which checkout.

## Projects and devices

A chat belongs to a project (a folder on a device) or is project-less.
`zeron chat spawn` inherits your project by default; `--project` picks
another, `--device` picks the host for a project-less chat.

```
zeron chat spawn --prompt "…" --project comet
zeron chat spawn --prompt "…" --device laptop
```

## Checkouts

When the parent's project is a git repo, a spawned chat gets a NEW worktree
of that repo by default — its own branch off the default base, so children
never share a dirty checkout. Override per spawn:

- `--worktree` — explicit new worktree; `--base <ref>` picks the base.
- `--same-checkout` — share the parent's cwd (fast, but you share its working
  tree state).
- `--cwd <path>` — an explicit directory, no worktree.

```
zeron chat spawn --prompt "…" --worktree --base release/1.4
zeron chat spawn --prompt "…" --same-checkout
zeron chat spawn --prompt "…" --cwd /tmp/scratch
```

## Limits

Agent-spawned children may nest to depth 4. One parent may have at most 8
running agent-spawned children; a spawn over the limit fails with exit code 5
and a hint. These limits exist so a fan-out stays reviewable — batch the work
or wait for children before spawning more.
