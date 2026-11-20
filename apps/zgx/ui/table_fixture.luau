-- Tables as a reply draws them, for ds/markdown's table tests and pictures:
-- the four shapes that matter, each in the reply's own column, in a page that
-- scrolls as a transcript does. `only_<name>` shows one alone; `width_<n>`
-- narrows the column; `theme_*`, `opaque` and `frosted` wear the others.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local DataTable = require("@ds/data_table")
local Markdown = require("@ds/markdown")
local material = require("@ds/material")

local TABLES: { [string]: string } = {
	-- Two columns of prose, as in a reply weighing a change.
	two = [[
| Cost of #1520 | Does #1523 offset it? |
| --- | --- |
| Keyed row reconciliation adds an allocation per row on every streamed token | Mostly: the keyed cache removes the per-row allocation for rows that stayed, so only new rows pay |
| The veil keeps a chunk list per paragraph until it finishes fading | No, but the list is short and goes when the reply settles |
| A wider retained tree makes hit testing walk more nodes | Yes, the spatial index skips whole columns once #1523 lands |
]],
	-- A 30-character label beside prose, as in a reply listing its chats: the
	-- label's room is its words' own, the prose takes the rest.
	running = [[
| Chat | Doing |
| --- | --- |
| **a1b2c3d4**, engine hang | Getting function names into the hang capture, which lock froze and whose code holds it, then a fix in a separate worktree. It won't deploy, restart the engine or push. It will say whether the build should go back to the previous release until there's a fix. |
| **e5f6a7b8**, nightly CI builds | The first full build on GitHub passed: Linux in 17m and Windows in 43m29s. It's rerunning to measure how fast a second build is once cached. |
| **c9d0e1f2**, sidebar icon | Building it. |
]],
	-- A 38-character label beside long prose with bold and code, as in a
	-- reply giving where things stand (the table Wing couldn't scroll).
	status = [[
| Area | Status |
|---|---|
| **Beta.5** | Windows and both Linux builds passed. macOS has been building, signing and notarizing for about 40 minutes; notarization usually takes the longest. It publishes right after. |
| **Memory** | Two of the three fixes are merged in zgx (#1960, #1981); #1945 merges after its checks. Together they bring memory after heavy use to about **264 MB** (was 315, and beta.1 was 271). The memory chat is now chasing the remaining ~50 MB stuck in partly used memory pages. |
| **Streaming** | ff4bff4f's three zgx PRs are open and in CI: append-only text, styled text that follows state, and the fade-in drawn as one piece. Next is the GPU-run fade-in under the glass, worked out with the render chat. Your staging app's build is at 5.9% (from 7.1%), with about 4–4.5% as the next target. |
| **Scrolling** | Waiting on zgx CI: accessibility updates (#1931), zgx's built-in scroll bar (#1961), and the "scrolling stopped" timer (#1975). The Zeron side of each is ready on branches. The working-rings fix is merged. |
| **The scroll-skip / overlapping-rows bug** | Not reproduced, and no cause yet. Tests are in place. If it happens again, send a screenshot and the chat name before navigating away. |
]],
	-- Short labels and numbers beside a column of prose.
	mixed = [[
| Build | Size | Change | Notes |
| --- | ---: | ---: | --- |
| v0.2.115 | 41.2 MB | -3.1% | Dropped the second copy of the emoji table, which both the picker and the composer's mentions had been carrying |
| v0.2.116 | 41.9 MB | +1.7% | Long transcripts split their messages so the engine socket doesn't drop |
]],
	-- More columns than a reply's width can ever hold in words.
	many = [[
| Harness | Model | Region | Turns | Input tokens | Output tokens | Cached share | Median first token |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| claude-code | opus-5-5 | us-west-2 | 128 | 1,482,190 | 96,204 | 81% | 1.24 s |
| codex | gpt-5.5-codex | us-east-1 | 94 | 902,441 | 71,880 | 66% | 1.61 s |
| opencode | sonnet-5-5 | eu-central-1 | 41 | 318,007 | 28,512 | 42% | 1.08 s |
| muse | muse-spark | us-west-2 | 12 | 77,300 | 9,014 | 0% | 2.47 s |
]],
	-- Words the source sets on their right, numbers or not.
	said = [[
| Plan | Note |
| ---: | --- |
| Free | Yours to keep |
| Team of many | Billed by seat |
]],
	-- A column of prose short of a wrap beside numbers: the table is
	-- narrower than the reply until its prose column takes what's left.
	loose = [[
| Changes | Frames |
| --- | --- |
| layout box easing | 3 |
| sideways wheel and drift latch | 12 |
| `~~` only | 1 |
]],
	-- A cell whose code can't break anywhere.
	code = [[
| Setting | Where it's read |
| --- | --- |
| Scroll | `application.rs::ScrollResampler::push_wheel_with_a_very_long_name_that_cannot_wrap::and_goes_on_past_any_reply_width::as_generated_code_does` |
| Theme | `ds/tokens.luau` |
]],
}

-- `ui.table` in a visual (ds/data_table), whose columns share the width.
local DATA = {
	{ "Keyed row reconciliation adds an allocation per row on every streamed token", "1,482" },
	{ "The veil keeps a chunk list per paragraph", "96" },
}

local only = ui.state("table_only", "all")
local width = ui.state("table_width", 760)

for name in TABLES do
	ui.command { id = `only_{name}`, title = name, run = function()
		only:set(name)
	end }
end
ui.command { id = "only_data", title = "data", run = function()
	only:set("data")
end }
ui.command { id = "only_all", title = "all", run = function()
	only:set("all")
end }
for _, w in { 280, 360, 400, 480, 560, 620, 680, 760, 860 } do
	ui.command { id = `width_{w}`, title = `{w}`, run = function()
		width:set(w)
	end }
end
ui.command { id = "theme_light", title = "Light", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }
ui.command { id = "opaque", title = "Opaque", run = function()
	material.frosted:set(false)
end }
ui.command { id = "frosted", title = "Frosted", run = function()
	material.frosted:set(true)
end }

local function shown(name: string): Node
	return ui.show(function(): boolean
		return only:get() == "all" or only:get() == name
	end, ui.column {
		id = `case-{name}`,
		w_full = true,
		pb = "$space.6",
		Markdown { text = TABLES[name] },
	})
end

return ui.column {
	id = "stage",
	w_full = true,
	h_full = true,
	bg = function(): string
		return if material.frosted:get() then "$background" else "$zeron.window"
	end,
	ui.scroll {
		id = "page",
		grow = 1,
		min_h = 0,
		w_full = true,
		ui.column {
			id = "reply",
			self = "center",
			p = "$space.6",
			w = function(): number
				return width:get()
			end,
			shown("two"),
			shown("running"),
			shown("status"),
			shown("mixed"),
			shown("many"),
			shown("code"),
			shown("said"),
			shown("loose"),
			ui.show(function(): boolean
				return only:get() == "all" or only:get() == "data"
			end, ui.column {
				id = "case-data",
				w_full = true,
				px = "$space.3",
				pb = "$space.6",
				DataTable {
					columns = {
						{ label = "What a long heading says about what the column holds" },
						{ label = "Allocations per streamed token", align = "end", w = 96 },
					},
					rows = #DATA,
					cell = function(r: number, c: number): Node
						return ui.text { class = "data-text", DATA[r][c], literal = true, wrap = true, min_w = 0 }
					end,
				},
			}),
			ui.div { id = "below", w_full = true, h = 48, shrink = 0 },
			ui.div { id = "foot", h = 1200, shrink = 0 },
		},
	},
}
