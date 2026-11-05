-- A label that rolls as it changes, as Zeron's chips do (crates/ui/src/
-- roll_text.rs, a port of Scritto's `<scritto-text>` roll): the characters
-- the old and new text share at either end stay put (the shared tail slides
-- to its new place), and the changed middle rolls: the old characters rise
-- out and shrink away as the new rise in from below, each softened by a
-- slight blur and tilt while it moves, staggered left to right, landing
-- dead still (no overshoot). The label's width glides between the two.
--
-- At rest it is a plain text (its truncation, wrapping and hit-testing as
-- ever); the rolling characters exist only for the moment a roll takes.
-- Reduced motion: no roll, the new text at once.
--
-- `RollText { function() return model_name() end, text_size = "$font.size.sm" }`
-- (the first entry: a function, a state, or a string). Other props style the
-- text; `ref`, `shrink`, `grow`, `min_w`, `max_w` go to its box. `anchor =
-- "end"` for a label held by its right edge: the old characters rise out
-- from that edge too.

local motion = require("@self/motion")
local prefers = require("@self/prefers")

-- Scritto's roll: 550 ms, a glyph travelling .35 em from a .6 scale, tilted
-- 2 degrees and blurred .1 em while it moves, the starts spread over 30% of
-- the duration; Zeron's ease, out-quint (no overshoot).
local DURATION, STAGGER = 550, 0.3
local TRAVEL, REST_SCALE, TILT, BLUR = 0.35, 0.6, 2, 0.1
local EASE = "outQuint"
local TOTAL = math.ceil(DURATION * (1 + STAGGER)) + 40

-- The roll in flight: from what, to what (its number, so a later change
-- outruns it).
type Roll = { from: string, to: string, id: number }

-- Props that belong to the box, not the text (`min_w` to both: a truncating
-- text needs it too).
local BOX = { ref = true, shrink = true, grow = true, min_w = true, max_w = true, w = true }
local BOTH = { min_w = true }

local function chars(s: string): { string }
	local out: { string } = {}
	for _, code in utf8.codes(s) do
		table.insert(out, utf8.char(code))
	end
	return out
end

-- How many characters the two share at the start, and at the end (never
-- overlapping in either).
local function shared(a: { string }, b: { string }): (number, number)
	local head = 0
	while head < #a and head < #b and a[head + 1] == b[head + 1] do
		head += 1
	end
	local tail = 0
	while tail < #a - head and tail < #b - head and a[#a - tail] == b[#b - tail] do
		tail += 1
	end
	return head, tail
end

local function join(list: { string }, from: number, to: number): string
	return table.concat(list, "", from, to)
end

return ui.component(function(props: { [any]: any }): Node
	-- (A message, `ui.t`, reads as a state does: its text, followed.)
	local source: string | State<string> | Message | (() -> string) = props[1]
	local function read(): string
		if type(source) == "function" then
			return source()
		elseif type(source) == "string" then
			return source
		end
		return source:get()
	end
	-- The text's style (all but the box's), and its size in points.
	local style: { [any]: any } = {}
	local box: { [any]: any } = {}
	for k, v in props do
		-- (`children`: what ui.component adds of a component's own.)
		if k ~= 1 and k ~= "children" and k ~= "anchor" then
			if BOX[k] then
				box[k] = v
			end
			if not BOX[k] or BOTH[k] then
				style[k] = v
			end
		end
	end
	local function size(): number
		local v = style.text_size
		if type(v) == "number" then
			return v
		elseif type(v) == "string" and string.sub(v, 1, 1) == "$" then
			return (ui.theme.get(string.sub(v, 2)) :: number?) or 13
		end
		return (ui.theme.get("font.size.md") :: number?) or 13
	end
	local function text(s: string, extra: { [any]: any }?): Node
		local p = table.clone(style)
		p.truncate, p.wrap, p.lines = nil, nil, nil
		p[1] = s
		if extra then
			for k, v in extra do
				p[k] = v
			end
		end
		return ui.text(p)
	end

	local roll = ui.use_state(nil :: Roll?)
	local last = read()
	local count = 0
	ui.effect(function()
		local now = read()
		if now == last then
			return
		end
		local from = last
		last = now
		count += 1
		if motion.reduced_now() or from == "" then
			roll:set(nil)
			return
		end
		roll:set({ from = from, to = now, id = count })
	end)

	-- The rolling label. Each character rolls on its own, but stands where
	-- the whole word, shaped as one text, puts it (its kerning kept): the
	-- roll ends on the settled text exactly, and handing back to it moves
	-- nothing. Where a character stands is measured, a frame before the
	-- roll starts (the old text still showing): how wide the word is up to
	-- and with it, less its own width. The box's width eases once, from the
	-- old word's to the new's; the shared head stays, the shared tail slides.
	local function rolling(r: Roll): Node
		local a, b = chars(r.from), chars(r.to)
		local head, tail = shared(a, b)
		local em = size()
		-- The strings to measure: each word's prefixes and its characters.
		local wanted: { [string]: boolean } = { [r.from] = true, [r.to] = true }
		local function want(w: { string })
			for i = 1, #w do
				wanted[join(w, 1, i)] = true
				wanted[w[i]] = true
			end
		end
		want(a)
		want(b)
		local widths: { [string]: number } = {}
		local height = 0
		local left = 0
		for _ in wanted do
			left += 1
		end
		local measured = ui.use_state(false)
		local measures: { Node } = {}
		for str in wanted do
			table.insert(
				measures,
				text(str, {
					absolute = true,
					left = 0,
					top = 0,
					-- Laid out, never drawn nor found.
					invisible = true,
					pointer_events = "none",
					aria_hidden = true,
					on_resize = function(sz: { w: number, h: number })
						if widths[str] == nil then
							left -= 1
						end
						widths[str] = sz.w
						height = math.max(height, sz.h)
						if left <= 0 and not measured:peek() then
							measured:set(true)
						end
					end,
				})
			)
		end
		-- Where character `i` of word `w` stands, shaped as the whole word.
		local function at(w: { string }, i: number): number
			if i <= 1 then
				return 0
			end
			-- (Past its last character: its end.)
			if i > #w then
				return widths[join(w, 1, #w)] or 0
			end
			return (widths[join(w, 1, i)] or 0) - (widths[w[i]] or 0)
		end
		local function width_of(word: string): number
			return widths[word] or 0
		end
		-- Laid out from the end (`anchor = "end"`): a word's x from the box's
		-- right edge less its width.
		local box_w = ui.use_state(0)
		local function offset(word: string): number
			return if props.anchor == "end" then box_w:get() - width_of(word) else 0
		end
		local function glyph(w: { string }, word: string, i: number, coming: boolean, delay: number): Node
			return text(w[i], {
				absolute = true,
				top = 0,
				left = function(): number
					return offset(word) + at(w, i)
				end,
				opacity = if coming then 0 else 1,
				y = if coming then TRAVEL * em else 0,
				scale = if coming then REST_SCALE else 1,
				on_mount = function(el)
					anime.animate(el, {
						opacity = if coming then { 0, 1 } else { 1, 0 },
						y = if coming then { TRAVEL * em, 0 } else { 0, -TRAVEL * em },
						scale = if coming then { REST_SCALE, 1 } else { 1, REST_SCALE },
						rotate = if coming then { TILT, 0 } else { 0, -TILT },
						blur = if coming then { BLUR * em, 0 } else { 0, BLUR * em },
						duration = DURATION,
						delay = delay,
						ease = EASE,
					})
				end,
			})
		end
		local function changed(w: { string }, word: string, coming: boolean): { Node }
			local out: { Node } = {}
			local from, to = head + 1, #w - tail
			local n = to - from + 1
			for i = from, to do
				local delay = if n > 1 then (i - from) / (n - 1) * STAGGER * DURATION else 0
				table.insert(out, glyph(w, word, i, coming, delay))
			end
			return out
		end
		-- Built once measured: the box at the old word's width, easing to
		-- the new word's; the tail from where the old word had it.
		local function view(): Node
			local tail_from = offset(r.from) + at(a, #a - tail + 1)
			local tail_to = function(): number
				return offset(r.to) + at(b, #b - tail + 1)
			end
			local v: { [any]: any } = {
				relative = true,
				w = width_of(r.from),
				h = height,
				on_resize = function(sz: { w: number, h: number })
					box_w:set(sz.w)
				end,
				on_mount = function(el)
					anime.animate(el, { w = { width_of(r.from), width_of(r.to) }, duration = DURATION, ease = EASE })
				end,
			}
			-- The shared head, where both words put it.
			if head > 0 then
				table.insert(
					v,
					text(join(b, 1, head), {
						absolute = true,
						top = 0,
						left = function(): number
							return offset(r.to)
						end,
					})
				)
			end
			for _, g in changed(b, r.to, true) do
				table.insert(v, g)
			end
			for _, g in changed(a, r.from, false) do
				table.insert(v, g)
			end
			-- The shared tail, sliding from where the old word had it to
			-- where the new one does.
			if tail > 0 then
				table.insert(
					v,
					text(join(b, #b - tail + 1, #b), {
						absolute = true,
						top = 0,
						left = tail_from,
						on_mount = function(el)
							anime.animate(el, { left = { tail_from, tail_to() }, duration = DURATION, ease = EASE })
						end,
					})
				)
			end
			return ui.div(v :: any)
		end
		-- Until measured, the old text as it stood (and the measures, unseen).
		return ui.div {
			relative = true,
			ui.presence(function(): string
				return if measured:get() then "roll" else "old"
			end, function(k: string): Node
				return if k == "roll" then view() else text(r.from)
			end),
			-- (Gone once read: a word in the tree only where it shows.)
			ui.show(function(): boolean
				return not measured:get()
			end, ui.div { absolute = true, left = 0, top = 0, w = 0, h = 0, overflow = "hidden", table.unpack(measures) }),
		}
	end

	-- The roll lets go once it has run its course, counted from when it
	-- started (measured), not from the change.
	ui.effect(function()
		local r = roll:get()
		if not r then
			return
		end
		local id = r.id
		ui.after(TOTAL + 16, function()
			local now = roll:peek()
			if now and now.id == id then
				roll:set(nil)
			end
		end)
	end)

	local opts = table.clone(box)
	return ui.presence(function(): number
		local r = roll:get()
		return if r then r.id else 0
	end, function(id: number): Node
		local r = roll:peek()
		if id ~= 0 and r and r.id == id then
			return rolling(r)
		end
		local p = table.clone(style)
		p[1] = read
		return ui.text(p)
	end, opts)
end)
