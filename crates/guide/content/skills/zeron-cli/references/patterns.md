# Orchestration patterns

## Fan-out / fan-in

Split independent work into children, collect at the end.

```
zeron chat spawn --prompt "Review crates/doc" --title "doc review"
zeron chat spawn --prompt "Review crates/engine" --title "engine review"
zeron chat spawn --prompt "Review crates/ui" --title "ui review"
```

Let them run — you get a notification per child (possibly batched). When you
need all results:

```
zeron chat wait <id1> <id2> <id3> --timeout 20m
zeron chat output <id1>
```

Each child gets its own worktree by default, so parallel edits never collide.
Stay under 8 running children; queue the rest behind `--wait` waves.

Children report through the notification, not through `tell`: a child's
final reply arrives as the completion notification, so a `tell` from a
child is a blocker — it needs a decision before it can continue.

## Reviewer loop

Spawn a fresh-eyes reviewer on your own diff, then act on its findings:

```
zeron chat spawn --prompt "Review the diff in this worktree. Report only real defects." --title "reviewer" --wait
zeron chat tell <reviewer> "Finding 2 is a false positive, skip it"
```

## Answering a child that needs help

A `needs help` notification names the blocker. Reply with guidance:

```
zeron chat tell <id> "Use the existing tokenizer in syntax/src."
```

or resolve a pending question directly:

```
zeron chat show <id>            # see the pending input
zeron chat answer <id> "Option B"
```

## Recovering a failed or interrupted child

Read what happened first:

```
zeron chat log <id> --limit 40
zeron chat show <id>
```

Then send corrective context and let the same chat retry (its session
resumes):

```
zeron chat tell <id> "The build failed on X; fix it by …"
```

An interrupted child may have been stopped by the user on purpose — do not
resume it unless the user asks. Prefer `tell` over respawning so the child's
context survives.
