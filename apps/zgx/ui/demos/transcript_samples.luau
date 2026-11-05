-- The Transcript page's conversation: prompts, the tools that answer them
-- and replies in every kind of markdown, mixed into as many turns as asked
-- (the same turns each time: a seeded mix, not a random one).

local M = {}

local PROMPTS = {
	"Why does the sidebar drop frames when a session is working?",
	"Can you add a copy button to code blocks in replies?",
	"Rename `ComposerState` to `Draft` everywhere, and keep the old name as a deprecated alias for a release.",
	"Where do we decide whether a reply's tool group starts open?",
	"Make the jump-to-latest pill appear only when you're more than a screen away from the end.",
	"ok, ship it",
	"Can you check why `local_first::shutdown` flakes on CI? It passed locally three times in a row.",
	"What's the difference between `measured` and `heights` on a list?",
	[[I pasted the crash below. It happens when I switch chats while a reply is still streaming, but only on the second switch, never the first.

```
thread 'main' panicked at crates/ui/src/transcript.rs:6712:
index out of bounds: the len is 41 but the index is 41
stack backtrace:
   0: rust_begin_unwind
   1: core::panicking::panic_fmt
   2: zeron_ui::transcript::TranscriptView::render_rows
   3: gpui::view::AnyView::render
```

Can you find what's holding the stale index? I think it's the fold state keyed by row index rather than row id, but I haven't checked. Also, while you're in there, the user bubble's fold animation jumps when the window is narrower than about 500 points: the measured height lags a frame behind the wrap.]],
	"Thanks! One more: the light theme's code wash is too strong next to the bubble. Can you soften it a little?",
	[[Here's the plan I'd like you to follow for the transcript port:

1. Port the user bubble first, with the fold and its fade.
2. Then replies: paragraphs, headings, lists, quotes and tables.
3. Code blocks with a header strip, the language on the left and Copy on the right.
4. The tool tree last, sharing the sidebar's trunk and elbows.

Keep every value in the theme. Don't add tokens unless nothing existing fits, and if you do, say why in a comment above them. Measure scrolling with `zgx inspect frames` before and after each step, and tell me the numbers, not just that it "feels smooth".]],
	"Is the streaming cadence tied to the display's refresh rate?",
}

local REPLIES = {
	[[The sidebar's spinner notifies its whole window on every frame, so each frame lays out and paints far more than the spinner.

## What happens

- The spinner's dot pulses at the display's rate.
- Each pulse writes a state the **row** reads, so the row is built again.
- The row's parent reads the row's height, so the list lays out again.

## The fix

Animate the dot's `opacity` instead of writing state: the engine eases it on the compositor, with no layout and no script.

```luau
ui.div {
	w = 6,
	h = 6,
	rounded = 3,
	bg = "$zeron.status.dot.working",
	opacity = ui.loop { from = 0.4, to = 1, duration = 1200 },
}
```

With that, a working session costs a few hundred microseconds a frame instead of four milliseconds.]],
	[[Done. Code blocks now have a header strip with the language on the left and a **Copy** button on the right. The button says *Copied* for a moment after a click, then goes back.

A few details:

- The strip doesn't take part in text selection, so selecting across a block copies only the code.
- Long lines scroll sideways instead of wrapping, and the edge they run past fades.
- The trailing newline is dropped before copying, so pasting into a terminal doesn't run the command.]],
	[[Renamed in 14 files. `ComposerState` is now `Draft`, with the old name kept as an alias:

```rust
#[deprecated(since = "0.2.130", note = "renamed to `Draft`")]
pub type ComposerState = Draft;
```

| Crate | Files | Uses |
| --- | --- | --- |
| `ui` | 9 | 112 |
| `app` | 3 | 17 |
| `proto` | 2 | 6 |

`cargo check --workspace` passes, and the alias produces one warning per remaining use outside the workspace, which is what we want.]],
	[[It's decided in `render_tool_group`, from three things:

1. Whether the reader has toggled it (their choice always wins).
2. Whether the turn is still running: a live group opens so you can watch it work.
3. Otherwise it stays folded to its summary.

> A group that opens on its own and then snaps shut when the reply starts reads as flicker, so the live group stays open until the reply's first words arrive, then folds once.

The fold itself is a height ease over 140 ms. The rows inside are laid out the whole time, so opening it never rewraps anything.]],
	[[Done: the pill now waits until the end of the conversation is more than **160 points** out of view, about seven lines. A small nudge up while reading no longer brings it up.

While a reply is still being written below, the pill says *New reply* with the working dot, and clicking it goes to the end and starts following again.]],
	[[Shipped. The release workflow is running; it usually takes about 25 minutes.]],
	[[It's a race between the engine's shutdown and the test's last write.

### What I found

- The test drops the engine while a sync write is still in flight.
- On a fast machine, the write finishes first and the test passes.
- On CI's slower runners, shutdown wins and the write fails with `Closed`.

### The fix

Await the write's acknowledgement before dropping the engine:

```rust
let ack = engine.write(doc, change).await?;
ack.persisted().await?;
drop(engine);
```

I ran it 200 times in a loop under `taskset -c 0` to imitate a slow runner: no failures, against 23 before.]],
	[[Both give rows different heights. They differ in **who knows the height and when**:

| | `heights` | `measured` |
| --- | --- | --- |
| Heights come from | your script | the rows' content |
| Known before a row is built | yes | no, it's estimated |
| Rows built offscreen | never | never |
| Good for | sections, diffs, fixed rows | chat, mail, wrapped text |

Use `heights` when you can compute each row's height cheaply (a header is 28, a file row is 22). Use `measured` when height depends on wrapping text, which a script can't measure.

With `measured`, the scrollbar's length is an estimate until the rows have been seen. Rows in view never move when rows above them are measured.]],
	[[You're right that it's the fold state. `user_folds` is keyed by row **id**, but the expander's click handler captures the row **index**, and the index goes stale when switching chats.

The sequence:

1. The first switch rebuilds the rows, and the handlers capture fresh indices.
2. While a reply streams, its rows are split into more blocks, which shifts every later index.
3. On the second switch the old handler still fires with its stale index and reads one past the end.

The fix is to capture the id and look up the index when the click lands:

```rust
.on_click(cx.listener(move |this, _, _, cx| {
    let Some(ix) = this.row_index(&row_id) else { return };
    this.toggle_user_fold(row_id.clone(), ix, collapsed_h, cx);
}))
```

For the fold animation at narrow widths: the measured height is read a frame after layout, so the ease starts from last frame's wrap. I now measure in the same pass as layout (`on_resize`), so the animation starts from the height that's actually drawn. At 420 points wide it no longer jumps.

---

Both fixes are in `transcript.rs`; tests added for each:

- `fold_toggle_survives_chat_switch_mid_stream`
- `fold_animation_starts_from_current_wrap`]],
	[[Softened. In the light theme the code wash went from 5% to **3.5%** ink, the same step the bubble uses, so the two read as one family.

Dark is unchanged: at 8% the wash still separates from the bubble there.]],
	[[Following the plan, step one is in: the user bubble, with the fold and its fade.

## User bubble

- Five lines of 22 points, then a fade over the last line and a half.
- *Show more* under the text; the bubble's height eases open, and the text is laid out the whole time, so nothing rewraps.
- Whether it folds is **measured**, not guessed from character counts.

## Numbers

Scrolling 300 turns with a fling, release build:

| Frame | Before | After |
| --- | --- | --- |
| median | 3.9 ms | 0.8 ms |
| worst | 11.2 ms | 2.1 ms |

Next: replies.]],
	[[No. The reply's text is committed on its own timer, about 30 times a second, and each commit parses only the text added since the last one. The display then draws at its own rate.

- Faster commits make no visible difference above roughly 30 a second.
- Slower ones (Zeron's 8 a second) make the text arrive in visible lumps.

So the cadence is fixed, and the cost of each commit is the size of what arrived, not the size of the reply.]],
}

local TOOLS: { { { any } } } = {
	{
		{ "search", "Searched", "spinner notify" },
		{ "read", "Read", "crates/ui/src/sidebar.rs" },
		{ "read", "Read", "crates/ui/src/status.rs" },
		{ "exec", "Ran", "cargo run --release -- --profile sidebar" },
		{ "edit", "Edited", "crates/ui/src/status.rs", 18, 31 },
	},
	{ { "read", "Read", "ds/markdown.luau" }, { "edit", "Edited", "ds/markdown.luau", 42, 6 } },
	{
		{ "glob", "Listed", "crates/**/*.rs" },
		{ "search", "Searched", "ComposerState" },
		{ "edit", "Edited", "crates/ui/src/composer.rs", 64, 64 },
		{ "edit", "Edited", "crates/app/src/main.rs", 7, 7 },
		{ "exec", "Ran", "cargo check --workspace" },
	},
	{ { "search", "Searched", "fn render_tool_group" }, { "read", "Read", "crates/ui/src/transcript.rs" } },
	{ { "read", "Read", "ds/transcript.luau" }, { "edit", "Edited", "ds/transcript.luau", 9, 3 } },
	{ { "exec", "Ran", "git tag v0.2.130 && git push origin v0.2.130" } },
	{
		{ "exec", "Ran", "cargo test -p local_first shutdown", nil, nil, true },
		{ "read", "Read", "crates/local_first/src/engine.rs" },
		{ "thought", "Thought about", "the shutdown ordering" },
		{ "edit", "Edited", "crates/local_first/tests/shutdown.rs", 6, 2 },
		{ "exec", "Ran", "for i in $(seq 200); do cargo test -q shutdown; done" },
	},
	{ { "web", "Searched the web for", "zgx ui.list measured heights" } },
	{
		{ "search", "Searched", "user_folds" },
		{ "read", "Read", "crates/ui/src/transcript.rs" },
		{ "agent", "Asked a subagent to", "reproduce the panic on a second chat switch" },
		{ "edit", "Edited", "crates/ui/src/transcript.rs", 23, 9 },
		{ "exec", "Ran", "cargo test -p zeron-ui fold_" },
		{ "todo", "Updated", "the plan" },
	},
	{ { "edit", "Edited", "ds/themes/zeron-light.json", 1, 1 } },
	{
		{ "read", "Read", "crates/ui/src/transcript.rs" },
		{ "write", "Wrote", "ds/user_message.luau", 118 },
		{ "exec", "Ran", "zgx inspect frames --since fling" },
	},
	{},
}

-- What came with some prompts: a log, a screenshot, a plan.
local ATTACH: { [number]: { any } } = {
	[9] = { { kind = "file", name = "crash-2026-10-29.log", meta = "Log · 3 KB" } },
	[10] = {
		{ kind = "image", name = "Light theme.png", src = "assets/cards/forest/back.webp" },
		{ kind = "image", name = "Dark theme.png", src = "assets/art/plate-1-night.jpg" },
	},
	[11] = { { kind = "file", name = "transcript-port-plan.md", meta = "Markdown · 2 KB" } },
}
M.ATTACH = ATTACH

-- Who answers: mostly one agent, sometimes another.
local AGENTS = {
	{ harness = "claude", model = "Opus 5.5" },
	{ harness = "claude", model = "Opus 5.5" },
	{ harness = "claude", model = "Opus 5.5" },
	{ harness = "openai", model = "GPT-5.5" },
}
M.AGENTS = AGENTS

-- A clock's time of day, "9:41 AM", from minutes since midnight.
local function clock(minutes: number): string
	local m = minutes % (24 * 60)
	local h = m // 60
	local half = if h < 12 then "AM" else "PM"
	local h12 = if h % 12 == 0 then 12 else h % 12
	return string.format("%d:%02d %s", h12, m % 60, half)
end
M.clock = clock

-- A seeded mix: the same sequence on every run.
local function rng(seed: number): () -> number
	local x = seed
	return function(): number
		x = (x * 1103515245 + 12345) % 2147483648
		return x / 2147483648
	end
end

function M.tools(i: number, tag: string): { any }
	local out = {}
	for n, t in TOOLS[i] or {} do
		table.insert(out, {
			id = `{tag}-t{n}`,
			kind = t[1],
			verb = t[2],
			target = t[3],
			added = t[4],
			removed = t[5],
			failed = t[6],
		})
	end
	return out
end

-- `turns` turns of prompt and reply (each reply's tools with it).
function M.conversation(turns: number): { any }
	local next = rng(7)
	local out = {}
	local minutes = 9 * 60 + 2
	for turn = 1, turns do
		local i = math.floor(next() * #PROMPTS) + 1
		local agent = AGENTS[math.floor(next() * #AGENTS) + 1]
		local tag = `m{turn}`
		minutes += 1 + math.floor(next() * 5)
		table.insert(out, { id = `{tag}u`, role = "user", text = PROMPTS[i], attachments = ATTACH[i] })
		table.insert(out, {
			id = `{tag}a`,
			role = "assistant",
			text = REPLIES[i],
			harness = agent.harness,
			model = agent.model,
			at = clock(minutes),
			tools = M.tools(i, tag),
			worked = 4 + math.floor(next() * 90),
		})
	end
	return out
end

-- The next live turn's prompt, reply and tools, cycling through them.
function M.live(n: number): (string, string, number)
	local i = (n - 1) % #PROMPTS + 1
	return PROMPTS[i], REPLIES[i], i
end

return M
