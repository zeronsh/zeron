-- A dump's receipt: what the person said, each part the Brain dump routed
-- underlined in its destination's hue, and a chip per part saying where it
-- went, what it is (a task, a question…) and how it stands. A question's
-- answer opens under its chip; a part the Brain dump wasn't sure about
-- offers its likely places as buttons; any chip moves its part elsewhere
-- from its menu. The one orchestrated moment: as a dump's parts are
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
local IconButton = require("@self/icon_button").IconButton
local ContextMenu = require("@self/context_menu")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")

export type Props = {
	dump: () -> any,
	now: () -> any,
	chats: () -> { [string]: any },
	on_reroute: (dump_id: string, route_id: string, target: { kind: string, id: string? }) -> (),
	on_open: (target: { kind: string, id: string? }) -> (),
	-- The excerpts' text size (a token): the Control room's is larger.
	text_size: string?,
	line_height: string?,
}

-- How far apart the excerpts light.
local STAGGER = 60

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
	-- The route whose chip the pointer is on (its excerpt deepens).
	local hovered = ui.use_state(nil :: string?)
	-- The answers opened, by route id.
	local open_answers = ui.use_state({} :: { [string]: boolean })
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
			known = if first then s.n else 0
		end
		known = math.min(known, s.n)
		shown_lit[id] = known
		lit:set(known)
		if known >= s.n then
			return
		end
		if motion.reduced_now() then
			shown_lit[id] = s.n
			lit:set(s.n)
			return
		end
		for k = known + 1, s.n do
			ui.after((k - known) * STAGGER, function()
				if (shown_lit[id] or 0) < k then
					shown_lit[id] = k
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

	-- The text, cut at its routes. Built again only as the dump, its
	-- routes' count or the scheme changes: a span's underline colour is
	-- fixed as it's made (zgx's), so the key carries what it's made of.
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
		local runs: { any } = {}
		for _, seg in data.segments(d.text or "", routes) do
			local i = seg.route
			if not i then
				table.insert(runs, ui.span { seg.text, literal = true })
			else
				local r = routes[i]
				local unsure = r.target and r.target.kind == "unsure"
				local h = if unsure then nil else route_hue(r)
				local rid = r.id
				local index = i :: number
				table.insert(runs, ui.span {
					seg.text,
					literal = true,
					underline = function(): boolean
						return lit:get() >= index
					end,
					underline_color = if unsure then hue.canvas(nil, 0.7) else hue.canvas(h, 0.9),
					underline_thickness = 1.5,
					underline_offset = 3,
					underline_style = if unsure then "dashed" else "solid",
					radius = 3,
					bg = function(): any
						if lit:get() < index then
							return "#00000000"
						end
						local a = hue.wash_alpha()
						if hovered:get() == rid then
							a *= 2
						end
						return hue.canvas(if unsure then nil else h, if unsure then a * 0.6 else a)
					end,
				})
			end
		end
		runs.wrap = true
		runs.w_full = true
		runs.text_size = props.text_size or "$font.size.md"
		runs.line_height = props.line_height or "$font.line.md"
		runs.color = "$text.primary"
		runs.selectable = true
		return ui.rich(runs)
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

	local function state_mark(row: State<any>): Node
		return ui.switch(function(): string
			local st = row:get().state
			if st == "pending" and row:get().target and row:get().target.kind == "unsure" then
				return "unsure"
			end
			return st or "pending"
		end, {
			pending = ui.div { color = "$text.tertiary", Icon { name = "clock-circle", size = 12 } },
			delivered = ui.div { color = "$text.tertiary", Icon { name = "check", size = 12 } },
			answered = ui.div { color = "$zeron.status.done", Icon { name = "chat-round-line", size = 12 } },
			undone = ui.div { color = "$text.tertiary", Icon { name = "refresh", size = 12 } },
			failed = ui.div { color = "$danger", Icon { name = "danger-triangle", size = 12 } },
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
			pl = "$space.2",
			pr = "$zeron.space.snug",
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.full",
			border = 1,
			border_color = function(): any
				return if unsure() then "$border" else hue.paint(t().hue, 0.38)
			end,
			bg = function(): any
				return if hovered:get() == row:get().id then hue.wash(t().hue) else "#00000000"
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
				unsure = ui.div { color = "$text.tertiary", Icon { name = "map-arrow-right", size = 12 } },
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
			state_mark(row),
		}
		local function dump_id(): string
			local d = props.dump()
			return if d then d.id else ""
		end
		return ui.column {
			class = "orch-route",
			gap = "$space.1",
			opacity = motion.tween(shown, { duration = 180, ease = "outCubic" }),
			y = motion.tween(ui.computed(function(): number
				return (1 - shown:get()) * 4
			end), { duration = 220, ease = "outCubic" }),
			ui.row {
				gap = "$space.1",
				items = "center",
				flex_wrap = true,
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
				-- A question answered: its answer, opened.
				ui.show(function(): boolean
					return row:get().answer ~= nil
				end, IconButton {
					icon = "alt-arrow-down",
					label = locale.t("ds.orch.receipt.answer"),
					size = "sm",
					tone = "quiet",
					lit = function(): boolean
						return open_answers:get()[row:get().id] == true
					end,
					on_click = function()
						local o = table.clone(open_answers:peek())
						local id = row:peek().id
						o[id] = not o[id]
						open_answers:set(o)
					end,
				}),
			},
			ui.show(function(): boolean
				return row:get().answer ~= nil and open_answers:get()[row:get().id] == true
			end, ui.row {
				class = "orch-answer",
				gap = "$space.2",
				pl = "$space.1",
				max_w = 560,
				ui.div {
					w = 2,
					self = "stretch",
					rounded = "$radius.full",
					bg = function(): any
						return hue.paint(t().hue, 0.6)
					end,
				},
				ui.text {
					function(): string
						return row:get().answer or ""
					end,
					literal = true,
					wrap = true,
					grow = 1,
					min_w = 0,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
				},
			}),
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
		ui.show(routing, ui.row {
			gap = "$space.2",
			items = "center",
			status.Spinner { size = 12, ink = "$text.tertiary" },
			ui.text { locale.t("ds.orch.receipt.routing"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		ui.each(routes, chip, { key = by_id, direction = "row", flex_wrap = true, gap_x = "$space.2", gap_y = "$space.2", items = "start", w_full = true }),
	}
end)
