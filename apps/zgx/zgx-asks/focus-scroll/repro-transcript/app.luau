-- Zeron's transcript (ds/transcript) cut down until a focused row still
-- comes to rest out of view (zgx-asks/focus-scroll, follow-up 1). Seen on
-- zgx 2e22d99d3: ⇧Tab from the composer up through the replies' tool rows
-- lands each at y 70 (the list's top 38 + `scroll_padding.top` 32), until
-- step 6 rests at y -255, above the view.
--
-- A bare list of the same shape lands every step. What this one adds, each
-- needed (with any one taken away every step lands at 70, 2 runs or more):
--   1. its offset set past the end a tick after each change of its size
--      (`on_resize` → `ui.after(0, …)`), as ds/transcript holds its end; the
--      list changes size as it first settles, so this runs at launch only;
--   2. a reply whose text is `ui.markdown` (a fixed box of the same height
--      lands every step);
--   3. `overscan = 3` (the transcript's; the default lands every step).
-- The script writes the offset only there (it prints when it does: at
-- launch, never during the steps). The test logs the offset and the focused
-- row every frame. It fails in about 5 runs of 6: the `ui.after(0, …)` races
-- the test's frames.

local messages = {}
for i = 1, 30 do
	table.insert(messages, { id = `u{i}`, role = "user", text = `Question number {i}: how does part {i} work?` })
	table.insert(messages, {
		id = `a{i}`,
		role = "assistant",
		text = `Part {i} works like this.\n\nIt reads its input, then it writes **its answer**, then it stops. That's the whole of part {i}.`,
		tools = { { id = `t{i}`, kind = "read", verb = "Read", target = `part{i}.rs` } },
	})
end
local data = ui.state("messages", messages)
local offset = ui.state("transcript_offset", 1e9)

local function row(m: any): Node
	local content: Node
	if m.role == "user" then
		content = ui.column { w_full = true, ui.text { m.text, h = 44 }, ui.div { h = 32 } }
	else
		content = ui.column {
			w_full = true,
			gap = 12,
			ui.button { w_full = true, h = 26, "Read 1 file" },
			ui.markdown(m.text),
			ui.div { w_full = true, h = 20 },
		}
	end
	return ui.row {
		w_full = true,
		justify = "center",
		items = "start",
		px = 20,
		pt = 16,
		pb = if m.id == "a30" then 45 else 0,
		ui.column { w_full = true, max_w = 660, min_w = 0, content },
	}
end

local Root = ui.component(function(): Node
	local last_w: number?, last_h: number? = nil, nil
	return ui.column {
		w_full = true,
		h_full = true,
		pt = 38,
		ui.list {
			id = "transcript",
			w_full = true,
			grow = 1,
			data = data,
			key = function(m)
				return m.id
			end,
			row = row,
			offset = offset,
			measured = true,
			height = 96,
			-- (The transcript's; without it, 3 runs in 4 still fail.)
			follow = true,
			overscan = 3,
			scroll_padding = { top = 32, bottom = 57 },
			-- Held at its end through a change of size (the window, a panel
			-- easing open), as ds/transcript holds it: the offset past the end
			-- once this layout is done.
			on_resize = function(size: { w: number, h: number })
				local changed = (last_w ~= nil and math.abs(size.w - last_w) >= 0.5)
					or (last_h ~= nil and math.abs(size.h - last_h) >= 0.5)
				last_w, last_h = size.w, size.h
				if changed then
					ui.after(0, function()
						print(`app: offset set past the end (a size change: {size.w}x{size.h})`)
						offset:set(1e9)
					end)
				end
			end,
		},
		ui.input { id = "composer", w = 600, h = 22, value = "" },
	}
end)
return Root {}
