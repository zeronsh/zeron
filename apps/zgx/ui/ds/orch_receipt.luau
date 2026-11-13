-- A dump's receipt: what the person said, each part the Brain dump routed
-- underlined in its destination's hue, and a chip per part saying where it
-- went, what it is (a task, a question…) and how it stands (Zeron's own
-- status glyphs). The chips stand in one row in the text's order, never
-- wrapping: past the room they scroll sideways (the wheel, a trackpad, a
-- drag), their edges fading where more waits. The pointer on a part of the
-- text shows a hover card (where it went, what it is, how it stands, the
-- message sent, the answer, a way in) and brings its chip into view, lit;
-- the pointer on a chip lights its part. A question's answer also comes
-- back as one quiet reply line under the chips; a part the Brain dump
-- wasn't sure about offers its likely places as buttons; any chip moves
-- its part elsewhere from its menu. The one orchestrated moment: as a dump's parts are
-- placed, their excerpts tint one after another, 60 ms apart, and their
-- chips arrive with them (all at once with motion reduced, and never for a
-- receipt already there when the page opened).
--
--   Receipt {
--     dump = function() return dump end,          -- as orchestration.now's
--     now = function() return orchestration.now:get() end,
--     chats = function() return chats by id end,  -- ds/orch_data.chats
--     on_reroute = function(dump_id, route_id, target) end,
--     on_open = function(target) end,             -- { kind, id }
--   }

local Icon = require("@self/icon")
local ContextMenu = require("@self/context_menu")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")
local hold = require("@self/orch_hold")
local PartCard = require("@self/orch_part_card")
local IconButton = require("@self/icon_button").IconButton
local Markdown = require("@self/markdown")

export type Props = {
	dump: () -> any,
	now: () -> any,
	chats: () -> { [string]: any },
	on_reroute: (dump_id: string, route_id: string, target: { kind: string, id: string? }) -> (),
	on_open: (target: { kind: string, id: string? }) -> (),
	-- Its parts there at once, never lit one after another (an earlier
	-- dump opened in the history).
	settled: boolean?,
	-- The excerpts' text size (a token): the Control room's is larger.
	text_size: string?,
	line_height: string?,
}

-- How far apart the excerpts light.
local STAGGER = 60
-- A part's underline: how thick, and how far up into the line's leading.
local UNDERLINE, UNDERLINE_STRONG = 1.5, 2.5
local UNDER_GAP = 3

-- The chips' edges: a fade as wide as this once they run past it, reached
-- this many times faster than they overflow (as the session tabs' strip).
local FADE, FADE_RATE = 28, 10

-- What a part's text is, cut to tokens that flow as words do: each word
-- with the space after it (a part's last word keeps its space out, so its
-- underline stops at the word), and which route it belongs to.
type Token = { text: string, route: number? }
local function tokens_of(segments: { any }): { Token }
	local out: { Token } = {}
	for _, seg in segments do
		local pieces: { string } = {}
		for word, space in string.gmatch(seg.text, "([^%s]*)(%s*)") do
			if word ~= "" or space ~= "" then
				table.insert(pieces, (word :: string) .. (space :: string))
			end
		end
		for i, piece in pieces do
			if seg.route and i == #pieces then
				local word, space = string.match(piece, "^(.-)(%s*)$")
				if word and word ~= "" then
					table.insert(out, { text = word, route = seg.route })
				end
				if space and space ~= "" then
					table.insert(out, { text = space })
				end
			else
				table.insert(out, { text = piece, route = seg.route })
			end
		end
	end
	return out
end

local INTENT_ICON: { [string]: Icon.Name } = {
	task = "checklist",
	info = "info-circle",
	question = "chat-round-line",
	priority = "flag",
	cancel = "close-circle",
	note = "document",
}

-- The routes lit so far, by dump id, as this window last showed them: a
-- receipt shown again picks up where it was, and one first seen with its
-- routes already there shows them at once.
local shown_lit: { [string]: number } = {}
-- (The same, for tests to read: `orch_receipt_lit`.)
local lit_seen = ui.state("orch_receipt_lit", {} :: { [string]: number })
local hovered_seen = ui.state("orch_receipt_hovered", "")
local function said_lit(id: string, n: number)
	shown_lit[id] = n
	lit_seen:set_key({ id }, n)
end
-- (Whether a receipt has been drawn yet this window: the first one's dumps
-- were there before it.)
local opened = false

local function words(kind: string): string
	return locale.s(`ds.orch.target.{kind}`)
end

local function routes_of(d: any): { any }
	return (d and d.routes) or {}
end

return ui.component(function(props: Props): Node
	-- How many of the shown dump's routes are lit.
	local lit = ui.use_state(0)
	-- The route whose chip or part the pointer is on (its excerpt deepens,
	-- its chip lights; `orch_receipt_hovered` says it to tests).
	local hovered = ui.use_state(nil :: string?)
	ui.effect(function()
		hovered_seen:set(hovered:get() or "")
	end)
	-- The chips' strip: where it's scrolled, its view and its row, each
	-- chip's box (to bring one into view), and a drag across it.
	local offset = ui.use_state(0)
	local view, content = ui.ref(), ui.ref()
	local chip_refs: { [string]: any } = {}
	local drag: { x: number, at: number, moved: boolean }? = nil
	local dragged = false
	local first = not opened
	opened = true

	-- Which dump, and how many routes: what lighting follows.
	local shape = ui.computed(function(): { id: string?, n: number }
		local d = props.dump()
		return { id = d and d.id, n = #routes_of(d) }
	end)
	ui.effect(function()
		local s = shape:get()
		if not s.id then
			lit:set(0)
			return
		end
		local id = s.id :: string
		local known = shown_lit[id]
		if known == nil then
			-- First seen: there before the page (its routes shown at once),
			-- or new (lit from nothing as its routes come).
			known = if first or props.settled then s.n else 0
		end
		known = math.min(known, s.n)
		said_lit(id, known)
		lit:set(known)
		if known >= s.n then
			return
		end
		if motion.reduced_now() then
			said_lit(id, s.n)
			lit:set(s.n)
			return
		end
		for k = known + 1, s.n do
			ui.after((k - known) * STAGGER, function()
				if (shown_lit[id] or 0) < k then
					said_lit(id, k)
				end
				if (shape:peek().id == id) and lit:peek() < k then
					lit:set(k)
				end
			end)
		end
	end)

	local function route_hue(r: any): number?
		if not r or not r.target then
			return nil
		end
		local t = data.target(props.now(), props.chats(), r.target, words)
		return t.hue
	end

	-- A part's hover card (ds/orch_part_card), reading its route as it
	-- stands now.
	local function card(index: number): Node
		local function r(): any
			return routes_of(props.dump())[index] or {}
		end
		return PartCard {
			route = r,
			target = function(): data.Target
				return data.target(props.now(), props.chats(), r().target, words)
			end,
			on_open = props.on_open,
		}
	end

	-- The chip of route `rid` brought into view, clear of the fade.
	local function reveal(rid: string)
		local r = chip_refs[rid]
		local el = r and r:peek()
		local row = content:peek()
		if not el or not row or not el.x or not row.x then
			return
		end
		local left = el.x - row.x
		local w = el.width or 0
		local room = view.width or 0
		local total = content.width or 0
		local at = offset:peek()
		local want = at
		if left - FADE < at then
			want = left - FADE
		elseif left + w + FADE > at + room then
			want = left + w + FADE - room
		end
		want = math.clamp(want, 0, math.max(0, total - room))
		if math.abs(want - at) > 0.5 then
			offset:set(want)
		end
	end

	-- The text, cut at its routes, a node a word so each part's words show
	-- its hover card. Built again only as the dump, its routes' count or the
	-- scheme changes: a span's underline colour is fixed as it's made
	-- (zgx's), so the key carries what it's made of.
	local function paragraph_key(): string
		local s = shape:get()
		return `{s.id or ""}:{s.n}:{if hue.dark() then "d" else "l"}`
	end
	local function paragraph(_key: string): Node
		local d = props.dump()
		if not d then
			return ui.div {}
		end
		local routes = routes_of(d)
		local text_size = props.text_size or "$font.size.md"
		local line_height = props.line_height or "$font.line.md"
		local nodes: { Node } = {}
		-- A word: its text, and under it a line (a part's underline, drawn
		-- here so it runs on through the space to the next word; a plain
		-- word's the same room, clear, so every line of words is as tall).
		local function word(text: string, line: (() -> any)?, strong: (() -> boolean)?): Node
			return ui.column {
				shrink = 0,
				ui.text {
					text,
					literal = true,
					text_size = text_size,
					line_height = line_height,
					color = "$text.primary",
				},
				ui.div {
					w_full = true,
					h = UNDERLINE_STRONG,
					mt = ui.raw(-UNDER_GAP, "the underline up into the line's leading, under the descenders"),
					ui.div {
						w_full = true,
						h = function(): number
							return if strong and strong() then UNDERLINE_STRONG else UNDERLINE
						end,
						bg = line or "#00000000",
					},
				},
			}
		end
		for _, tok in tokens_of(data.segments(d.text or "", routes)) do
			local i = tok.route
			if not i then
				table.insert(nodes, word(tok.text))
			else
				local r = routes[i]
				local unsure = r.target and r.target.kind == "unsure"
				local h = if unsure then nil else route_hue(r)
				local rid = r.id
				local index = i :: number
				table.insert(nodes, ui.div {
					class = "orch-part",
					shrink = 0,
					tooltip = card(index),
					tooltip_placement = { side = "bottom", align = "start", offset = 6 },
					tooltip_interactive = true,
					on_hover = function(inside: boolean)
						if inside then
							hovered:set(rid)
							reveal(rid)
						elseif hovered:peek() == rid then
							hovered:set(nil)
						end
					end,
					-- (The pointer on the part, its chip or its reply: its line
					-- full strength and thicker.)
					word(tok.text, function(): any
						if lit:get() < index then
							return "#00000000"
						end
						return hue.paint(if unsure then nil else h, if hovered:get() == rid then 1 else (if unsure then 0.5 else 0.7))
					end, function(): boolean
						return hovered:get() == rid
					end),
				})
			end
		end
		local p: any = nodes
		p.class = "orch-receipt-text"
		p.role = "group"
		p.label = d.text or ""
		p.w_full = true
		p.flex_wrap = true
		p.items = "start"
		p.selectable = true
		return ui.row(p)
	end

	local routes = ui.computed(function(): { any }
		return routes_of(props.dump())
	end)
	local function by_id(r: any): string
		return r.id
	end

	local function target_of(r: any): data.Target
		return data.target(props.now(), props.chats(), r.target, words)
	end

	-- A chip's menu: open where it went, or move it to another place.
	local function menu(row: State<any>): { any }
		local r = row:peek()
		local d = props.dump()
		local entries: { any } = {}
		local t = target_of(r)
		if t.id and t.kind ~= "unsure" then
			table.insert(entries, {
				label = locale.s("ds.orch.receipt.open", { name = t.label }),
				icon = "arrow-up-right",
				on_select = function()
					props.on_open({ kind = t.kind, id = t.id })
				end,
			})
			table.insert(entries, { separator = true })
		end
		table.insert(entries, { heading = locale.s("ds.orch.receipt.move_to") })
		for _, o in (props.now() or {}).orchestrators or {} do
			if o.kind ~= "brain_dump" and not o.archived and not (r.target and r.target.kind == "orchestrator" and r.target.id == o.id) then
				table.insert(entries, {
					label = o.name,
					on_select = function()
						props.on_reroute(d.id, r.id, { kind = "orchestrator", id = o.id })
					end,
				})
			end
		end
		table.insert(entries, {
			label = words("new_thread"),
			icon = "plus",
			on_select = function()
				props.on_reroute(d.id, r.id, { kind = "new_thread" })
			end,
		})
		table.insert(entries, {
			label = words("note"),
			icon = "document",
			on_select = function()
				props.on_reroute(d.id, r.id, { kind = "note" })
			end,
		})
		return entries
	end

	-- How a part stands, as Zeron says it: on its way (the ring, quiet),
	-- delivered or answered (done), failed; moved, the refresh glyph. Held
	-- 300 ms at least (ds/orch_hold).
	local function state_mark(row: State<any>): Node
		local kind = hold.value(function(): string
			local r = row:get()
			if r.target and r.target.kind == "unsure" then
				return "unsure"
			end
			return r.state or "pending"
		end)
		return ui.switch(kind, {
			pending = ui.div { class = "orch-chip-state", status.Spinner { size = 12, ink = "$text.tertiary" } },
			delivered = ui.div { class = "orch-chip-state", status.StatusIcon { kind = "done", size = 13 } },
			answered = ui.div { class = "orch-chip-state", status.StatusIcon { kind = "done", size = 13 } },
			undone = ui.div { class = "orch-chip-state", color = "$text.tertiary", Icon { name = "refresh", size = 12 } },
			failed = ui.div { class = "orch-chip-state", status.StatusIcon { kind = "failed", size = 13 } },
			unsure = ui.div {},
		}, ui.div {})
	end

	local function candidate(dump_id: () -> string, row: State<any>, c: any): Node
		local function t(): data.Target
			return data.target(props.now(), props.chats(), c, words)
		end
		return ui.button {
			class = "orch-candidate",
			label = function(): string
				return t().label
			end,
			h = 24,
			px = "$zeron.space.snug",
			gap = "$space.1",
			items = "center",
			rounded = "$radius.full",
			border = 1,
			border_color = "$border",
			bg = "#00000000",
			hover = { bg = "$hover" },
			on_click = function()
				props.on_reroute(dump_id(), row:peek().id, { kind = c.kind, id = c.id })
			end,
			ui.show(function(): boolean
				return t().hue ~= nil
			end, hue.Dot { hue = function(): number?
				return t().hue
			end, size = 7 }, ui.div { color = "$text.secondary", Icon { name = "plus", size = 12 } }),
			ui.text {
				function(): string
					return t().label
				end,
				text_size = "$font.size.sm",
				color = "$text.primary",
			},
		}
	end

	local function chip(r0: any, i: number, row: State<any>): Node
		local shown = ui.computed(function(): number
			return if lit:get() >= i then 1 else 0
		end)
		local function t(): data.Target
			return target_of(row:get())
		end
		local function unsure(): boolean
			local tg = row:get().target
			return tg ~= nil and tg.kind == "unsure"
		end
		local opener: ((x: number, y: number) -> boolean)? = nil
		local chip_el: Element? = nil
		local function intent(): string
			return row:get().intent or "task"
		end
		local body = ui.button {
			class = "orch-chip",
			label = function(): string
				local r = row:get()
				return locale.s("ds.orch.receipt.chip", {
					target = t().label,
					intent = locale.s(`ds.orch.intent.{intent()}`),
					state = locale.s(`ds.orch.route_state.{if unsure() then "unsure" else (r.state or "pending")}`),
				})
			end,
			h = 26,
			shrink = 0,
			pl = "$zeron.space.snug",
			pr = "$space.2",
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.full",
			border = 1,
			border_color = function(): any
				local lit_up = hovered:get() == row:get().id
				return if unsure() then (if lit_up then "$border.strong" else "$border") else hue.paint(t().hue, if lit_up then 0.85 else 0.3)
			end,
			bg = function(): any
				local h = if unsure() then nil else t().hue
				return if hovered:get() == row:get().id then hue.wash(h) else hue.paint(h, hue.wash_alpha() * 0.35)
			end,
			hover = {},
			on_hover = function(inside: boolean)
				if inside then
					hovered:set(row:peek().id)
				elseif hovered:peek() == row:peek().id then
					hovered:set(nil)
				end
			end,
			on_visible = function(_v: boolean, el: Element)
				chip_el = el
			end,
			on_click = function()
				-- (The end of a drag across the strip is no click.)
				if dragged then
					return
				end
				local el = chip_el
				if opener and el and el.x and el.y then
					opener(el.x, el.y + (el.height or 26) + 4)
				end
			end,
			ui.switch(function(): string
				return if unsure() then "unsure" else "placed"
			end, {
				placed = hue.Dot { hue = function(): number?
					return t().hue
				end, size = 7 },
				unsure = ui.div { color = "$zeron.status.input", Icon { name = "status-input", size = 13 } },
			}),
			ui.text {
				function(): string
					return t().label
				end,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
				truncate = true,
				max_w = 200,
			},
			ui.div {
				color = "$text.tertiary",
				Icon {
					name = function(): Icon.Name
						return INTENT_ICON[intent()] or "checklist"
					end,
					size = 12,
				},
			},
			ui.div { w = 1, h = 12, bg = "$border", shrink = 0 },
			state_mark(row),
		}
		local function dump_id(): string
			local d = props.dump()
			return if d then d.id else ""
		end
		local ref = ui.ref()
		chip_refs[r0.id] = ref
		return ui.column {
			class = "orch-route",
			ref = ref,
			shrink = 0,
			gap = "$space.1",
			opacity = motion.tween(shown, { duration = 180, ease = "outCubic" }),
			y = motion.tween(ui.computed(function(): number
				return (1 - shown:get()) * 4
			end), { duration = 220, ease = "outCubic" }),
			ui.row {
				gap = "$space.1",
				items = "center",
				ContextMenu {
					items = function(): { any }?
						return menu(row)
					end,
					hook = function(open_at: (x: number, y: number) -> boolean)
						opener = open_at
					end,
					label = function(): string
						return t().label
					end,
					body,
				},
				-- Unsure: its likely places, then elsewhere from the menu.
				ui.show(unsure, ui.row {
					gap = "$space.1",
					items = "center",
					ui.each(ui.computed(function(): { any }
						return row:get().candidates or {}
					end), function(c: any): Node
						return candidate(dump_id, row, c)
					end, { direction = "row", gap = "$space.1", items = "center" }),
				}),
			},
		}
	end

	-- A question's answer, come back: a quiet line under the chips, its
	-- place's dot and name, the answer, and the way into it. The pointer on
	-- it lights its part in the text, as its chip does.
	local answered = ui.computed(function(): { any }
		local out = {}
		local n = lit:get()
		for i, r in routes:get() do
			if i <= n and type(r.answer) == "string" and r.answer ~= "" then
				table.insert(out, r)
			end
		end
		return out
	end)
	local function reply(_r: any, _i: number, row: State<any>): Node
		local function t(): data.Target
			return target_of(row:get())
		end
		local index = 0
		for i, rr in routes:peek() do
			if rr.id == row:peek().id then
				index = i
			end
		end
		return ui.row {
			class = "orch-answer",
			w_full = true,
			h = 24,
			gap = "$space.2",
			items = "center",
			tooltip = card(index),
			tooltip_placement = { side = "bottom", align = "start", offset = 6 },
			tooltip_interactive = true,
			on_hover = function(inside: boolean)
				if inside then
					hovered:set(row:peek().id)
				elseif hovered:peek() == row:peek().id then
					hovered:set(nil)
				end
			end,
			hue.Dot { hue = function(): number?
				return t().hue
			end, size = 6 },
			ui.text {
				function(): string
					return t().label
				end,
				truncate = true,
				min_w = 0,
				max_w = 180,
				shrink = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.secondary",
			},
			-- Into its thread, beside its name.
			ui.show(function(): boolean
				local tg = t()
				return tg.id ~= nil and tg.kind ~= "unsure"
			end, IconButton {
				class = "orch-answer-open",
				icon = "arrow-up-right",
				label = function(): string
					return locale.s("ds.orch.receipt.open", { name = t().label })
				end,
				size = "sm",
				tone = "quiet",
				on_click = function()
					local tg = t()
					props.on_open({ kind = tg.kind, id = tg.id })
				end,
			}),
			ui.column {
				class = "orch-answer-text",
				grow = 1,
				min_w = 0,
				Markdown {
					text = function(): string
						return data.answer_line(row:get())
					end,
					inline = true,
					small = true,
				},
			},
		}
	end

	-- The chips' edges: each fades in as chips run past it.
	local function bands(): { number }
		local at = offset:get()
		local past = (content.width or 0) - (view.width or 0) - at
		return { math.clamp(at * FADE_RATE, 0, FADE), math.clamp(past * FADE_RATE, 0, FADE) }
	end

	-- The chips: one row in the text's order, scrolled sideways past the
	-- room (the wheel, a trackpad, or a drag across them).
	local function strip(): Node
		local p: any = {
			id = "orch-chips",
			class = "orch-chips",
			ref = view,
			offset = offset,
			w_full = true,
			fade_x = bands,
			-- A mouse wheel without a sideways axis moves them too, once the
			-- pointer has moved onto them (Shift + wheel always does), as the
			-- model picker's tabs.
			wheel_across = true,
			on_press = function(e: PointerEvent)
				drag = { x = e.x, at = offset:peek(), moved = false }
				dragged = false
			end,
			on_pointer_move = function(e: PointerEvent)
				local d = drag
				if not d then
					return
				end
				local dx = e.x - d.x
				if math.abs(dx) > 4 then
					d.moved = true
					dragged = true
				end
				if d.moved then
					local room = view.width or 0
					local total = content.width or 0
					offset:set(math.clamp(d.at - dx, 0, math.max(0, total - room)))
				end
			end,
			on_release = function()
				drag = nil
				-- (The click that ends a drag comes after the release.)
				ui.after(0, function()
					dragged = false
				end)
			end,
			on_hover = function(inside: boolean)
				if not inside then
					drag = nil
				end
			end,
			ui.row {
				ref = content,
				class = "orch-chips-row",
				gap = "$space.2",
				items = "center",
				py = "$zeron.space.hair",
				ui.each(routes, chip, { key = by_id, direction = "row", gap = "$space.2", items = "center" }),
			},
		}
		return ui.scroll_x(p)
	end

	local function routing(): boolean
		local d = props.dump()
		return d ~= nil and d.state == "routing"
	end

	return ui.column {
		class = "orch-receipt",
		w_full = true,
		gap = "$space.3",
		ui.presence(paragraph_key, paragraph, { w_full = true }),
		ui.show(routing, ui.row {
			gap = "$space.2",
			items = "center",
			status.Spinner { size = 12, ink = "$text.tertiary" },
			ui.text { locale.t("ds.orch.receipt.routing"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		strip(),
		ui.show(function(): boolean
			return #answered:get() > 0
		end, ui.column {
			class = "orch-answers",
			w_full = true,
			max_w = 760,
			ui.each(answered, reply, { key = by_id, w_full = true, gap = "$space.1" }),
		}),
	}
end)
