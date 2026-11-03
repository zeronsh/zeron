-- A reply's subagents, as their own block at the top of the reply (where
-- its tools' group would sit), apart from its tools: the agents it set to
-- work, from every harness (a Claude Code `Task`, a Codex spawn, a chat its
-- agent made through Zeron as a subagent).
--
-- Its header: the subagents' marks stacked, each cut where the next sits
-- (ds/people's stack, `cut_into`), then "3 subagents · 2 working" (the
-- working count in the accent; all finished, "· done"), how long they've
-- taken on the right, and the chevron. Open, a row each: its mark, its
-- title, under it "model · what it's on" while it runs or its result's
-- first line once done, its state (the working ring, a check), how long it
-- took, and Stop while it runs. A row opens its subagent's lane in the panel,
-- running or done (a click anywhere on it but Stop, Enter on it), its wash
-- under the pointer. Rows sit on the chat: no card, no box.
--
-- Open while any of them runs; folded once all are done, unless the reader
-- chose (kept per reply, by `key`, across it being made again).
--
--   Subagents { key = message.id, items = function() return subs end,
--               on_stop = function(sub, settle) … end,
--               on_open = function(sub) … end }
--
-- An item changes in place: rows are keyed by its id and read it as it is
-- now, so a status, a step or a time moving touches only what shows it.

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local FadeText = require("@self/fade_text")
local Mark = require("@self/mark")
local people = require("@self/people")
local status = require("@self/status")
local control = require("@self/control")
local locale = require("@self/locale")

export type Status = "running" | "done" | "failed"
export type Sub = {
	id: string,
	title: string,
	status: Status,
	-- Its agent's mark (ds.Mark's name, or a registry agent's harness,
	-- `acp.<id>`); none, the generic agent glyph.
	mark: string?,
	-- Its model as named, and what it's on now (running) or what it came
	-- back with (done: its first line).
	model: string?,
	step: string?,
	result: string?,
	-- When it started and ended (`os.time()` seconds), where known.
	started: number?,
	ended: number?,
	-- Whether it can be stopped alone, and opened in a lane.
	stoppable: boolean?,
	openable: boolean?,
	-- A stop asked of it from elsewhere (the composer's stop of all that's
	-- left running) is on its way: "Stopping", as its own Stop says.
	stopping: boolean?,
}

export type Props = {
	key: string,
	items: () -> { Sub },
	-- Stop one running subagent; `settle(false)` if it wasn't stopped after
	-- all (the caller says why), and its row is as it was.
	on_stop: ((sub: Sub, settle: (stopping: boolean) -> ()) -> ())?,
	on_open: ((sub: Sub) -> ())?,
	-- When the reply began, and how long it worked once done (seconds): the
	-- header's time where its subagents' own aren't known.
	started: (() -> number?)?,
	worked: (() -> number?)?,
}

-- The header's height (Activity's), the stack's faces and a row's.
local HEADER = 28
local FACE = 18
local ROW_FACE = 20
local FOLD = 200
local MAX_FACES = 4
-- The faces' overlap (ds/people's third) and the gap cut round each.
local OVERLAP = math.floor(FACE * 0.3)
local GAP = 1.5

-- What the reader chose per reply (open or folded), and what they stopped
-- (by id: "stopping" until it's no longer running). One map each.
local CHOSEN = ui.state("subagents_chosen", {} :: { [string]: boolean })
local STOPS = ui.state("subagents_stops", {} :: { [string]: string })

-- A subagent's face: its agent's mark on a quiet disc, the bot glyph where
-- it has none. `cut`: the next face's circle taken out of it.
local function face(mark: () -> string?, size: number, cut: { x: number, r: number }?): Node
	local held: { [any]: any } = {
		w = size,
		h = size,
		shrink = 0,
		rounded = "$radius.full",
		bg = "$zeron.secondary.bg",
		color = "$text.primary",
		items = "center",
		justify = "center",
		aria_hidden = true,
		-- (Its glyph swaps only when its agent does.)
		ui.presence(function(): string
			return mark() or ""
		end, function(name: string): Node
			if name == "" then
				return Icon { name = "bot", size = math.floor(size * 0.6) }
			end
			return Mark { name = name, size = math.floor(size * 0.6), bare = true }
		end),
	}
	if cut then
		people.cut_into(held, size, cut)
	end
	return ui.row(held)
end

local function running(s: Sub): boolean
	return s.status == "running"
end

-- "42s", "5m": how long it has taken, as far as is known.
local function took(s: Sub, now: number): string
	if not s.started then
		return ""
	end
	local stop = if running(s) then now else (s.ended or s.started)
	return status.elapsed(math.max(0, stop - s.started))
end

local function row(now_of: () -> Sub, tick: () -> number, props: Props): Node
	local id = now_of().id
	local function stage(): string?
		return STOPS:get()[id] or (if now_of().stopping then "stopping" else nil)
	end
	local function set_stage(to: string?)
		local all = table.clone(STOPS:peek())
		all[id] = to
		STOPS:set(all)
	end
	-- No longer running: a stop asked for is done.
	ui.effect(function()
		if not running(now_of()) and STOPS:peek()[id] ~= nil then
			set_stage(nil)
		end
	end)
	local function kind(): status.Kind
		local s = now_of()
		return if s.status == "running" then "working" elseif s.status == "failed" then "failed" else "done"
	end
	local function second(): string
		local s = now_of()
		if s.status == "running" then
			local step = s.step or locale.s("ds.subagents.working")
			return if s.model then `{s.model} · {step}` else step
		end
		return s.result or locale.s(if s.status == "failed" then "ds.subagents.failed" else "ds.subagents.finished")
	end
	local function can_stop(): boolean
		local s = now_of()
		return props.on_stop ~= nil and running(s) and s.stoppable == true and stage() == nil
	end
	-- Its lane opens from the whole row, running or done.
	local function can_open(): boolean
		return props.on_open ~= nil and now_of().openable == true
	end
	local function open()
		if props.on_open and now_of().openable then
			props.on_open(now_of())
		end
	end
	-- The action's room at the row's end: an icon button's, wider while it
	-- says "Stopping".
	local function room(): number
		return if stage() ~= nil and running(now_of()) then 72 else 22
	end
	return ui.row {
		class = "subagent-row",
		role = "group",
		label = function(): string
			return now_of().title
		end,
		relative = true,
		w_full = true,
		min_w = 0,
		-- The row itself is the press that opens its lane (a click anywhere
		-- on it but Stop, Enter on it), its wash under the pointer.
		ui.button {
			class = "subagent-open",
			label = function(): string
				local t = now_of().title
				return if can_open() then locale.s("ds.subagents.open_named", { title = t }) else t
			end,
			disabled = function(): boolean
				return not can_open()
			end,
			w_full = true,
			min_w = 0,
			px = "$space.2",
			py = "$space.1",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			focus = control.ring,
			cursor = function(): string
				return if can_open() then "pointer" else "default"
			end,
			selectable = false,
			on_click = open,
			face(function(): string?
				return now_of().mark
			end, ROW_FACE),
			ui.column {
				grow = 1,
				min_w = 0,
				FadeText {
					function(): string
						return now_of().title
					end,
					literal = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.semibold",
					color = "$text.primary",
				},
				FadeText {
					second,
					class = "subagent-step",
					literal = true,
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
			},
			status.StatusIcon { kind = kind, size = 14 },
			ui.text {
				function(): string
					return took(now_of(), tick())
				end,
				class = "subagent-time",
				shrink = 0,
				tabular = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			-- Done, the open arrow; running, room for Stop over it.
			ui.row {
				shrink = 0,
				w = room,
				justify = "end",
				color = "$text.tertiary",
				ui.show(function(): boolean
					return can_open() and not running(now_of())
				end, Icon { name = "arrow-up-right", size = 14 }),
			},
		},
		-- Stop: over the row's end, a press of its own (a sibling over the
		-- row's press, never inside it, so it stops and opens nothing).
		ui.row {
			absolute = true,
			top = 0,
			bottom = 0,
			right = ui.raw(8, "the row's inset ($space.2)"),
			items = "center",
			gap = "$space.1",
			ui.show(
				function(): boolean
					return stage() ~= nil and running(now_of())
				end,
				ui.text {
					locale.t("ds.subagents.stopping"),
					role = "status",
					shrink = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				}
			),
			ui.show(
				can_stop,
				IconButton {
					icon = "stop",
					label = function(): string
						return locale.s("ds.subagents.stop_named", { title = now_of().title })
					end,
					tooltip = locale.s("ds.subagents.stop"),
					size = "sm",
					tone = "quiet",
					on_click = function()
						local s = now_of()
						if not props.on_stop then
							return
						end
						set_stage("stopping")
						props.on_stop(s, function(stopping: boolean)
							if not stopping and STOPS:peek()[id] == "stopping" then
								set_stage(nil)
							end
						end)
					end,
				}
			),
		},
	}
end

return ui.component(function(props: Props): Node
	local items = ui.computed(props.items)
	local by_id = ui.computed(function(): { [string]: Sub }
		local out: { [string]: Sub } = {}
		for _, s in items:get() do
			out[s.id] = s
		end
		return out
	end)
	local working = ui.computed(function(): number
		local n = 0
		for _, s in items:get() do
			if running(s) then
				n += 1
			end
		end
		return n
	end)
	local function choice(): boolean?
		return CHOSEN:get()[props.key]
	end
	local opened = ui.computed(function(): boolean
		local c = choice()
		if c ~= nil then
			return c
		end
		return working:get() > 0
	end)
	-- The clock, a second at a time while any of them runs (the timer is
	-- this effect's: it stops as they do).
	local tick = ui.use_state(os.time())
	ui.effect(function()
		if working:get() > 0 then
			tick:set(os.time())
			ui.every(1000, function()
				tick:set(os.time())
			end)
		end
	end)
	-- How long they've taken together: from the first start to now while
	-- any runs, else to the last end (the reply's own time where theirs
	-- isn't known).
	local function elapsed(): string
		local now = tick:get()
		local first, last = nil :: number?, nil :: number?
		for _, s in items:get() do
			if s.started and (first == nil or s.started < first) then
				first = s.started
			end
			local stop = if running(s) then now else s.ended
			if stop and (last == nil or stop > last) then
				last = stop
			end
		end
		first = first or (if props.started then props.started() else nil)
		if working:get() > 0 then
			last = now
		elseif last == nil and first and props.worked then
			local w = props.worked()
			last = if w then first + w else nil
		end
		if not first or not last then
			return ""
		end
		return status.elapsed(math.max(0, last - first))
	end
	-- The rows exist while open or folding (as Activity's).
	local mounted = ui.use_state(opened:peek())
	local folding: (() -> ())? = nil
	ui.effect(function()
		local on = opened:get()
		if folding then
			folding()
			folding = nil
		end
		if on then
			mounted:set(true)
		elseif mounted:peek() then
			folding = ui.after(FOLD, function()
				folding = nil
				mounted:set(false)
			end)
		end
	end)
	-- The stack: the first few faces, each cut by the one after it (or by
	-- the "+N" past them); a face's cut is fixed as it's made, so it's made
	-- again only when whether it's covered changes.
	local faces = ui.computed(function(): { Sub }
		local list = items:get()
		local out = {}
		local shown = if #list == MAX_FACES + 1 then #list else math.min(#list, MAX_FACES)
		for i = 1, shown do
			table.insert(out, list[i])
		end
		return out
	end)
	local function rest(): number
		return #items:get() - #faces:get()
	end
	local next_cut = { x = FACE - OVERLAP + FACE / 2, r = FACE / 2 + GAP }
	local function count_label(): string
		return locale.s("ds.subagents.count", { n = #items:get() })
	end
	local function header_label(): string
		local n = working:get()
		local tail = if n > 0 then locale.s("ds.subagents.working_n", { n = n }) else locale.s("ds.subagents.done")
		return `{count_label()} · {tail}`
	end
	return ui.column {
		class = "subagents",
		w_full = true,
		ui.button {
			class = "subagents-header",
			label = header_label,
			expanded = opened,
			selectable = false,
			w_full = true,
			h = HEADER,
			p = 0,
			gap = "$space.2",
			items = "center",
			bg = "#00000000",
			hover = { bg = "#00000000" },
			active = { bg = "#00000000" },
			focus = control.ring,
			cursor = "pointer",
			on_click = function()
				local all = table.clone(CHOSEN:peek())
				all[props.key] = not opened:peek()
				CHOSEN:set(all)
			end,
			-- (Held from press to release, as Activity's header: a selection
			-- across the reply never starts on it.)
			on_press = function() end,
			on_release = function() end,
			ui.row {
				class = "subagents-faces",
				shrink = 0,
				items = "center",
				ui.each(faces, function(s: Sub, i: number): Node
					local sid = s.id
					local function covered(): boolean
						local list = faces:get()
						return list[#list] == nil or list[#list].id ~= sid or rest() > 0
					end
					return ui.div {
						shrink = 0,
						ml = if i == 1 then 0 else ui.raw(-OVERLAP, "faces overlap by a third, as ds/people's stack"),
						ui.presence(function(): string
							return if covered() then "cut" else "whole"
						end, function(how: string): Node
							return face(function(): string?
								local now = by_id:get()[sid]
								return if now then now.mark else s.mark
							end, FACE, if how == "cut" then next_cut else nil)
						end),
					}
				end, {
					key = function(s: Sub): string
						return s.id
					end,
				}),
				ui.show(
					function(): boolean
						return rest() > 0
					end,
					ui.row {
						w = FACE,
						h = FACE,
						shrink = 0,
						ml = ui.raw(-OVERLAP, "faces overlap by a third, as ds/people's stack"),
						rounded = "$radius.full",
						bg = "$zeron.secondary.hover",
						items = "center",
						justify = "center",
						ui.text {
							function(): string
								return `+{rest()}`
							end,
							text_size = ui.raw(9, "a count inside a face under 24 points, as ds/people's"),
							weight = "$font.weight.medium",
							tabular = true,
							color = "$text.secondary",
						},
					}
				),
			},
			ui.row {
				grow = 1,
				min_w = 0,
				gap = "$space.1",
				items = "center",
				text_size = "$font.size.sm",
				ui.text { count_label, shrink = 0, color = "$text.primary", weight = "$font.weight.medium" },
				ui.text { "·", shrink = 0, color = "$text.tertiary" },
				ui.text {
					function(): string
						local n = working:get()
						return if n > 0 then locale.s("ds.subagents.working_n", { n = n }) else locale.s("ds.subagents.done")
					end,
					class = "subagents-state",
					shrink = 0,
					color = function(): string
						return if working:get() > 0 then "$accent" else "$text.secondary"
					end,
				},
			},
			ui.text {
				elapsed,
				class = "subagents-time",
				shrink = 0,
				tabular = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			ui.div {
				shrink = 0,
				color = "$text.tertiary",
				rotate = ui.spring(opened, { from = 0, to = 90, stiffness = 380, damping = 30 }),
				Icon { name = "alt-arrow-right", size = 14 },
			},
		},
		ui.column {
			w_full = true,
			overflow = "hidden",
			layout = "height",
			layout_clip = true,
			layout_duration = FOLD,
			layout_ease = "outQuart",
			max_h = function(): number
				return if opened:get() then 1e6 else 0
			end,
			ui.show(
				mounted,
				ui.column {
					class = "subagents-rows",
					w_full = true,
					pt = "$space.1",
					ui.each(items, function(s: Sub): Node
						local sid = s.id
						local now = ui.computed(function(): Sub
							return by_id:get()[sid] or s
						end)
						return row(function(): Sub
							return now:get()
						end, function(): number
							return tick:get()
						end, props)
					end, {
						key = function(s: Sub): string
							return s.id
						end,
						enter = { opacity = { 0, 1 }, y = { -6, 0 }, duration = 360, ease = "outExpo" },
					})
				}
			),
		},
	}
end)
