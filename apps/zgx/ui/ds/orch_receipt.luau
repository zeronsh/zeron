-- A dump's receipt: what the person said, each part the Brain dump routed
-- underlined in its destination's hue, and a chip per part saying where it
-- went, what it is (a task, a question…) and how it stands (Zeron's own
-- status glyphs). The chips stand in one row in the text's order, never
-- wrapping: past the room they scroll sideways (the wheel, a trackpad, a
-- drag), their edges fading where more waits. The pointer on a part of the
-- text shows a hover card (where it went, what it is, how it stands, the
-- message sent, the answer, a way in) and brings its chip into view, lit;
-- the pointer on a chip lights its part. A question the person wants
-- answered (`direct`) is also washed in its hue, a highlighter's mark: its
-- answer waits in the Control room's Q&A. A question's answer also comes
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
local RouteMark = require("@self/orch_route_mark")
local PartCard = require("@self/orch_part_card")
local IconButton = require("@self/icon_button").IconButton
local Markdown = require("@self/markdown")
local HoverCard = require("@self/hover_card")

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
-- How long the pointer rests on another word before the part card hangs
-- from it (past zgx's grace for crossing into a hover card, 120 ms), and
-- the card's width (ds/orch_part_card's).
local SWITCH_MS = 250
local CARD_W = 480

-- The chips' edges: a fade as wide as this once they run past it, reached
-- this many times faster than they overflow (as the session tabs' strip).
local FADE, FADE_RATE = 28, 10

-- What a part's text is, cut to tokens that flow as words do: each word
-- with the space after it (a part's last word keeps its space out, so its
-- underline stops at the word), and the routes it belongs to (none, one,
-- or several: words routed to more than one place).
type Token = { text: string, routes: { number }? }
local function tokens_of(segments: { any }): { Token }
	local out: { Token } = {}
	for _, seg in segments do
		local routes = if #seg.routes > 0 then seg.routes else nil
		local pieces: { string } = {}
		for word, space in string.gmatch(seg.text, "([^%s]*)(%s*)") do
			if word ~= "" or space ~= "" then
				table.insert(pieces, (word :: string) .. (space :: string))
			end
		end
		for i, piece in pieces do
			if routes and i == #pieces then
				local word, space = string.match(piece, "^(.-)(%s*)$")
				if word and word ~= "" then
					table.insert(out, { text = word, routes = routes })
				end
				if space and space ~= "" then
					table.insert(out, { text = space })
				end
			else
				table.insert(out, { text = piece, routes = routes })
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
-- (And what the shown dump's text draws, for tests: each piece's words and
-- the route it's underlined in (`marks`), and the pieces washed under the
-- pointer, each by the route lighting it (`washed`), as "route:words",
-- by dump id.)
local marks_seen = ui.state("orch_receipt_marks", {} :: { [string]: { string } })
local washed_seen = ui.state("orch_receipt_washed", {} :: { [string]: { string } })
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
	-- The routes whose chip or part the pointer is on (their excerpts
	-- deepen, their chips light; `orch_receipt_hovered` says them to tests,
	-- joined by commas): a chip's one, or every route holding the words.
	local hovered = ui.use_state({} :: { string })
	ui.effect(function()
		hovered_seen:set(table.concat(hovered:get(), ","))
	end)
	-- Whether route `rid` is lit by the pointer.
	local function hot(rid: string): boolean
		return table.find(hovered:get(), rid) ~= nil
	end
	local function hover_on(rids: { string })
		hovered:set(rids)
	end
	-- Let go, if what's lit is still `rids`.
	local function hover_off(rids: { string })
		if table.concat(hovered:peek(), ",") == table.concat(rids, ",") then
			hovered:set({})
		end
	end
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
	-- stands now; `c`, ds/hover_card Anchored's (framed or not, its close).
	local function card(index: number | () -> number, c: HoverCard.CardContext): Node
		local function r(): any
			local i = if type(index) == "function" then (index :: any)() else index
			return routes_of(props.dump())[i] or {}
		end
		return PartCard {
			route = r,
			target = function(): data.Target
				return data.target(props.now(), props.chats(), r().target, words)
			end,
			on_open = props.on_open,
			bare = c.bare,
			on_close = c.close,
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

	-- The part under the pointer in the text: its word's box (in the text's
	-- own points), its first route's index and id (whose card hangs from
	-- it), and every route holding the word; nil off every part.
	type At = { x: number, y: number, w: number, h: number, index: number, rid: string, rids: { string } }
	local at = ui.use_state(nil :: At?)
	-- The part the pointer is on now (what lights), apart from `at`, the
	-- word the card hangs from: that one stays put while the pointer
	-- crosses other words on its way into the card, and moves only once
	-- the pointer rests on another word (`SWITCH_MS`, past zgx's grace
	-- for the crossing). Off the text and off the card, it goes.
	local under: At? = nil
	local switching = 0
	local over_card = false
	local words_of = ""
	ui.effect(function()
		-- (A dump of other words: no word of the last one's.)
		local sh = shape:get()
		local k = `{sh.id or ""}:{sh.n}`
		if k ~= words_of then
			words_of = k
			at:set(nil)
			under = nil
		end
	end)

	-- The text, cut at its routes: drawn as one paragraph (each part a span,
	-- underlined in its hue, tinted under the pointer), so it reads, selects
	-- and copies exactly as it was written. Under it, unseen, the same words
	-- laid out a box each, to know which part the pointer is on; over the
	-- word it's on, a box of the same size holds the part's hover card.
	-- Built again only as the dump, its routes' count or the scheme changes:
	-- a span's underline colour is fixed as it's made (zgx's), so the key
	-- carries what it's made of.
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
		-- (Worked out once for each dump, its routes' count and the scheme,
		-- as this paragraph is: never as the pointer moves.)
		local segments = data.segments(d.text or "", routes)

		-- How route `i` underlines, wherever its words are.
		type Look = { hue: number?, unsure: boolean, rid: string, asked: boolean }
		local looks: { [number]: Look } = {}
		for i, r in routes do
			local unsure = r.target ~= nil and r.target.kind == "unsure"
			looks[i] = { hue = if unsure then nil else route_hue(r), unsure = unsure, rid = r.id, asked = r.direct == true }
		end
		-- Route `i`'s underline on `span`, lit as its route is.
		local function underline(span: any, i: number)
			local look = looks[i]
			span.underline = function(): boolean
				return lit:get() >= i
			end
			span.underline_color = if look.unsure then hue.canvas(nil, 0.6) else hue.canvas(look.hue, 0.85)
			span.underline_thickness = 1.5
			span.underline_offset = 4
			span.underline_style = if look.unsure then "dashed" else "solid"
		end

		-- The route lighting words held by routes `mine` under the pointer
		-- (a chip's, or the first of the words' own), once it's lit.
		local function washing(mine: { number }): number?
			local lit_now = lit:get()
			for _, i in mine do
				if lit_now >= i and hot(looks[i].rid) then
					return i
				end
			end
			return nil
		end
		local marks: { string } = {}
		local pieces_lit: { { text: string, routes: { number } } } = {}

		-- The drawn text: each piece underlined in its route's colour; words
		-- routed to several places striped, a word each in each place's
		-- colour by turns; washed under the pointer in the colour of the
		-- route lit there (a chip's, or the first of the words' own).
		local runs: any = {}
		for _, seg in segments do
			local mine = seg.routes
			if #mine == 0 then
				table.insert(runs, ui.span { seg.text, literal = true })
				continue
			end
			-- A question the person wants answered (its answer waits in the
			-- Q&A): its words held in a highlighter's wash of its hue, not
			-- only underlined.
			local asked: number? = nil
			for _, i in mine do
				if looks[i].asked then
					asked = i
					break
				end
			end
			local function bg(): any
				local lit_now = lit:get()
				local i = washing(mine)
				if i then
					local look = looks[i]
					return hue.canvas(if look.unsure then nil else look.hue, hue.wash_alpha() * (if look.unsure then 0.8 else 1.3))
				end
				if asked and lit_now >= asked then
					return hue.canvas(looks[asked].hue, hue.wash_alpha() * 0.75)
				end
				return "#00000000"
			end
			for k, piece in data.stripes(seg.text, #mine) do
				local span: any = { piece, literal = true, radius = 3, bg = bg }
				local i = mine[(k - 1) % #mine + 1]
				underline(span, i)
				table.insert(runs, ui.span(span))
				table.insert(marks, `{looks[i].rid}:{piece}`)
			end
			table.insert(pieces_lit, { text = seg.text, routes = mine })
		end
		-- (Said for tests: what's underlined, and what the pointer washes.)
		local dump_id = d.id
		ui.after(0, function()
			marks_seen:set_key({ dump_id }, marks)
		end)
		ui.effect(function()
			local out: { string } = {}
			for _, p in pieces_lit do
				local i = washing(p.routes)
				if i then
					table.insert(out, `{looks[i].rid}:{p.text}`)
				end
			end
			washed_seen:set_key({ dump_id }, out)
		end)
		runs.class = "orch-receipt-words"
		runs.wrap = true
		runs.w_full = true
		runs.text_size = text_size
		runs.line_height = line_height
		runs.color = "$text.primary"

		-- The same words, unseen, a box each: a run of words not parted by a
		-- space (a part's last word and the comma after it) stays one box, so
		-- they break where the paragraph does.
		local boxes: { { ref: any, index: number, rid: string, rids: { string } } } = {}
		local chunks: { Node } = {}
		local pieces: { Node } = {}
		local function close()
			if #pieces > 0 then
				local row: any = pieces
				row.shrink = 0
				table.insert(chunks, ui.row(row))
				pieces = {}
			end
		end
		for _, tok in tokens_of(segments) do
			local i = if tok.routes then tok.routes[1] else nil
			local asked = false
			for _, j in tok.routes or {} do
				asked = asked or looks[j].asked
			end
			local text = ui.text {
				tok.text,
				-- (A question's words, for tests to find.)
				class = if asked then "orch-question-word" else nil,
				literal = true,
				text_size = text_size,
				line_height = line_height,
				-- (Laid out, never seen: the paragraph over it draws the words.)
				color = ui.raw("#00000000", "the hit layer's words, unseen"),
			}
			if i then
				local ref = ui.ref()
				local rids: { string } = {}
				for _, j in tok.routes or {} do
					table.insert(rids, looks[j].rid)
				end
				table.insert(boxes, { ref = ref, index = i, rid = looks[i].rid, rids = rids })
				table.insert(pieces, ui.div { class = "orch-part", ref = ref, shrink = 0, text })
			else
				table.insert(pieces, text)
			end
			if string.match(tok.text, "%s$") then
				close()
			end
		end
		close()
		local layout: any = chunks
		layout.class = "orch-receipt-boxes"
		layout.absolute = true
		layout.left = 0
		layout.top = 0
		layout.right = 0
		layout.flex_wrap = true
		layout.items = "start"
		layout.pointer_events = "none"
		layout.aria_hidden = true
		layout.selectable = false

		local holder = ui.ref()
		-- Which part the pointer is on, from the boxes.
		local function find(x: number, y: number): At?
			local o = holder:peek()
			if not o or not o.x then
				return nil
			end
			for _, b in boxes do
				local el = b.ref:peek()
				if el and el.x and x >= el.x and x < el.x + el.width and y >= el.y and y < el.y + el.height then
					return { x = el.x - o.x, y = el.y - o.y, w = el.width, h = el.height, index = b.index, rid = b.rid, rids = b.rids }
				end
			end
			return nil
		end
		local function same(a: At?, b: At?): boolean
			return a ~= nil and b ~= nil and a.x == b.x and a.y == b.y
		end
		local function point(x: number, y: number)
			local found = find(x, y)
			local was = under
			under = found
			if found then
				if not same(found, was) then
					-- (Every route holding the word: each chip lights.)
					hover_on(found.rids)
					reveal(found.rid)
				end
			elseif was then
				hover_off(was.rids)
			end
			if not found or same(found, at:peek()) then
				return
			end
			if at:peek() == nil then
				at:set(found)
				return
			end
			switching += 1
			local mine = switching
			ui.after(SWITCH_MS, function()
				if mine == switching and same(under, found) then
					at:set(under)
				end
			end)
		end
		-- Off the text: the card's anchor goes, unless the pointer went into
		-- the card (it goes as the pointer leaves that).
		local function let_go()
			switching += 1
			local mine = switching
			ui.after(SWITCH_MS, function()
				if mine == switching and under == nil and not over_card then
					at:set(nil)
				end
			end)
		end
		local function leave()
			local was = under
			under = nil
			if was then
				hover_off(was.rids)
			end
			let_go()
		end

		return ui.overlay {
			class = "orch-receipt-text",
			ref = holder,
			role = "group",
			label = d.text or "",
			w_full = true,
			relative = true,
			selectable = true,
			on_pointer_move = function(e: PointerEvent)
				point(e.x, e.y)
			end,
			on_hover = function(inside: boolean)
				if not inside then
					leave()
				end
			end,
			ui.rich(runs),
			ui.row(layout),
			-- Over the word the pointer rests on: the part's hover card,
			-- hanging from that word (ds/hover_card Anchored).
			ui.show(function(): boolean
				return at:get() ~= nil
			end, ui.div {
				class = "orch-part-anchor",
				absolute = true,
				left = function(): number
					local a = at:get()
					return if a then a.x else 0
				end,
				top = function(): number
					local a = at:get()
					return if a then a.y else 0
				end,
				w = function(): number
					local a = at:get()
					return if a then a.w else 0
				end,
				h = function(): number
					local a = at:get()
					return if a then a.h else 0
				end,
				on_pointer_move = function(e: PointerEvent)
					point(e.x, e.y)
				end,
				HoverCard.Anchored {
					class = "orch-part-hold",
					w = CARD_W,
					-- (Over the word: under it, it would cover the chip the
					-- word has just scrolled into view.)
					placement = { side = "top", align = "start", offset = 6 },
					-- (The word's box: the outer one's, which places it.)
					anchor = ui.div {
						w = function(): number
							local a = at:get()
							return if a then a.w else 0
						end,
						h = function(): number
							local a = at:get()
							return if a then a.h else 0
						end,
					},
					card = function(c: HoverCard.CardContext): Node
						return card(function(): number
							local a = at:get()
							return if a then a.index else 0
						end, c)
					end,
					on_card_hover = function(inside: boolean)
						over_card = inside
						if not inside then
							let_go()
						end
					end,
				},
			}),
		}
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
	-- How a part stands, as Zeron says it (ds/orch_route_mark).
	local function state_mark(row: State<any>): Node
		return RouteMark.Mark {
			state = function(): string
				local r = row:get()
				if r.target and r.target.kind == "unsure" then
					return "unsure"
				end
				return r.state or "pending"
			end,
		}
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
		-- Its part's card, pinned by a click on it (ds/hover_card Anchored);
		-- its menu is the secondary click's, the Menu key's, Shift+F10's.
		local pinned = ui.use_state(false)
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
				local lit_up = hot(row:get().id)
				return if unsure() then (if lit_up then "$border.strong" else "$border") else hue.paint(t().hue, if lit_up then 0.85 else 0.3)
			end,
			bg = function(): any
				local h = if unsure() then nil else t().hue
				return if hot(row:get().id) then hue.wash(h) else hue.paint(h, hue.wash_alpha() * 0.35)
			end,
			hover = {},
			on_hover = function(inside: boolean)
				if inside then
					hover_on({ row:peek().id })
				else
					hover_off({ row:peek().id })
				end
			end,
			on_click = function()
				-- (The end of a drag across the strip is no click.)
				if dragged then
					return
				end
				pinned:set(not pinned:peek())
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
					label = function(): string
						return t().label
					end,
					HoverCard.Anchored {
						class = "orch-chip-hold",
						w = CARD_W,
						pinned = pinned,
						anchor = body,
						card = function(c: HoverCard.CardContext): Node
							return card(function(): number
								local id = row:get().id
								for k, rr in routes:get() do
									if rr.id == id then
										return k
									end
								end
								return 0
							end, c)
						end,
					},
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
		local line = ui.row {
			class = "orch-answer",
			w_full = true,
			h = 24,
			gap = "$space.2",
			items = "center",
			on_hover = function(inside: boolean)
				if inside then
					hover_on({ row:peek().id })
				else
					hover_off({ row:peek().id })
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
		-- Its part's card, hanging from the line (ds/hover_card Anchored).
		return HoverCard.Anchored {
			class = "orch-answer-hold",
			w = CARD_W,
			w_full = true,
			anchor = line,
			card = function(c: HoverCard.CardContext): Node
				return card(index, c)
			end,
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

	-- What the person dumped with it: a picture as a small thumbnail, any
	-- other file as a chip of its name.
	local files = ui.computed(function(): { any }
		local d = props.dump()
		return (d and d.attachments) or {}
	end)
	local function picture(name: string): boolean
		local ext = string.lower(string.match(name, "%.([%w]+)$") or "")
		return ext == "png" or ext == "jpg" or ext == "jpeg" or ext == "gif" or ext == "webp" or ext == "heic"
	end
	local function file(f: any, _i: number): Node
		local name = f.name or f.path or ""
		-- (A file still on its way to the engine is a `pending://` ref: a chip.)
		if picture(name) and type(f.path) == "string" and not string.match(f.path, "^pending://") then
			return ui.div {
				class = "orch-dump-picture",
				w = 56,
				h = 40,
				shrink = 0,
				rounded = "$radius.md",
				overflow = "hidden",
				border = 1,
				border_color = "$zeron.composer.chip.edge",
				border_over = true,
				tooltip = name,
				ui.image {
					src = f.path,
					fit = "cover",
					w_full = true,
					h_full = true,
					fallback = ui.div { w_full = true, h_full = true, items = "center", justify = "center", color = "$text.tertiary", Icon { name = "gallery", size = 16 } },
				},
			}
		end
		return ui.row {
			class = "orch-dump-file",
			h = 26,
			px = "$space.2",
			gap = "$space.1",
			items = "center",
			shrink = 0,
			rounded = "$radius.md",
			bg = "$hover",
			ui.div { color = "$text.tertiary", Icon { name = if picture(name) then "gallery" else "document", size = 12 } },
			ui.text { name, literal = true, truncate = true, max_w = 200, text_size = "$font.size.xs", color = "$text.secondary" },
		}
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
		ui.show(function(): boolean
			return #files:get() > 0
		end, ui.each(files, file, { key = function(f: any): string
			return f.path or f.name or ""
		end, class = "orch-dump-files", direction = "row", flex_wrap = true, gap = "$space.2", items = "center", w_full = true })),
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
