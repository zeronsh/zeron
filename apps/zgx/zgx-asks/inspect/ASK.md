# Ask: `zgx inspect` frames that measure a whole interaction

**Who needs it:** anyone measuring frame cost in Zeron's app, for example the panel's drag-resize work (Nov 1 2026): p95 per move, before and after a change, read from `inspect frames`.

**The problems**, three, found while doing that:

1. **`zgx inspect --json frames n=300` returns 10 frames.** The CLI sends `n` as the string `"300"`, so `number("n")` in `inspect.rs` doesn't read it and the default (10) applies. The text form has the same limit. Over the socket, `{"cmd":"frames","n":300}` works. Ten frames is one or two drags, so p95 from the CLI is silently a sample of the last few frames.
2. **`zgx inspect mark name=drag` sets a mark named `name=drag`.** The CLI takes the whole word as the positional name, so `frames since=drag` then matches no mark and falls back to the latest frames, with no error. `mark drag` works. Either read `name=` as the field, or say "no mark `drag`" when `since` names none.
3. **No per-node attribution of layout.** `frames` gives a frame's layout phase in total (for example 7.5 ms) and the script runs, but not which subtrees were laid out or what that cost. That was the question that mattered: here it took bisecting by hand to find that an empty chat page re-lays out for about 5 ms whenever its width changes, the transcript adds 2 ms, and an open file 3.7 ms.

**The asks:**

- Parse numeric fields (`n`, `runs`, `keep`) from the CLI's `key=value` words as numbers, and `name=`/`since=` as fields. Or reject unknown forms loudly.
- `since` naming no mark is an error, not a fallback.
- Layout attribution in `frames`: the top N subtrees by layout time (node, source line, micros, and whether it was a cache hit). Even a count of nodes laid out versus taken from the cache per frame would do.

**Repro:** any app with `--inspect`:

```sh
zgx inspect --app $PID mark name=x          # → mark `name=x`
zgx inspect --app $PID --json frames since=x n=300 | jq '.data | length'   # → 10
```
