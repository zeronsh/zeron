-- An orchestrator's flow (ds/orch_flow), drawn to be edited: its blocks as
-- cards on a field, its connections as curves between their ports, and
-- beside the field what's selected, to change (a block: its name, agent
-- and model, instructions; a connection: when it's followed and how often
-- it may go back; nothing: the flow's name and what stops it running).
--
-- Drag a block to move it; drag from the port under a block to another
-- block to connect them; drag the space to pan; scroll pans too, and
-- ⌘/Ctrl-scroll or a pinch zooms. Every change is handed on whole
-- (`on_change`), never held here.
--
-- The editing follows Orchestrator Builder, the Zeron plugin by
-- Master0fFate (github.com/Master0fFate/zeron-orchestration-builder),
-- written anew for Zeron's field and inspector.
--
--   FlowEditor {
--     flow,               () -> Flow? (nil: nothing yet, a starter offered)
--     on_change,          (Flow) -> ()
--     hue?,               () -> number? (the orchestrator's: what's selected)
--     scope?,             () -> string? (whose flow: a new one clears the
--                         selection)
--     models?,            () -> { { value, label, group? } } as "harness/model"
--     on_import?, on_export?, on_save_template?, on_start?(kind)
--     run?,               () -> Run? (its run as it stands: { state =
--                         "running" | "blocked" | "finished" | "stopped",
--                         at? = node id, step? = its name, done? = { [node
--                         id] = true } })
--     on_run?,            (task: string) -> () (Run…: a run started on it)
--     on_stop?,           () -> () (the run under way stopped)
--   }
--
-- With `on_run`, a flow that can run has Run… in its toolbar (a task
-- asked for first); while a run is under way, where it is and Stop
-- instead, the step it's at ringed on the field and the steps done
-- ticked.

local flow = require("@self/orch_flow")
local locale = require("@ds/locale")

-- Each block kind's name, written out so the locale check sees every key.
local KIND_KEYS = {
	auditor = "orch.flow.kind.auditor",
	chief = "orch.flow.kind.chief",
	finish = "orch.flow.kind.finish",
	gate = "orch.flow.kind.gate",
	planner = "orch.flow.kind.planner",
	qa = "orch.flow.kind.qa",
	researcher = "orch.flow.kind.researcher",
	validator = "orch.flow.kind.validator",
	worker = "orch.flow.kind.worker",
}
local function kind_name(kind: string): string
	local key = KIND_KEYS[kind]
	return if key then locale.s(key) else kind
end
local hue = require("@self/orch_hue")
local Icon = require("@ds/icon")
local IconButton = require("@ds/icon_button").IconButton
local Button = require("@ds/button")
local Text = require("@ds/text")
local TextField = require("@ds/text_field")
local TextArea = require("@ds/text_area")
local Select = require("@ds/select")
local badge = require("@ds/badge")
local Toolbar = require("@ds/toolbar")
local Sheet = require("@ds/dialog").Sheet
local Dialog = require("@ds/dialog").Dialog

local M = {}

-- What's selected: { node = id } or { edge = index } or {}. (One editor a
-- window: the page's.)
M.selected = ui.state("orch_flow_selected", {} :: { node: string?, edge: number? })

local W, H, H_END = 208, 76, 48
local PORT = 5
local HIT_PORT = 14
local ZOOM_MIN, ZOOM_MAX = 0.3, 2
local INSPECTOR_W = 300
local NARROW_AT = 860
local NONE = "none"

-- Every block's height: an entry or a finish says only its name.
local function height(n: any): number
	return if n.kind == "chief" or n.kind == "finish" then H_END else H
end

-- A connection's curve, out of the bottom of one block into the top of
-- another; one that goes back up leaves and comes in at the right side,
-- each a little wider than the last so they don't lie on each other.
local function curve(a: any, b: any, i: number): { number }
	local x1, y1 = a.x + W / 2, a.y + height(a)
	local x2, y2 = b.x + W / 2, b.y
	if y2 > y1 + 8 then
		local reach = math.max(32, (y2 - y1) * 0.45)
		return { x1, y1, x1, y1 + reach, x2, y2 - reach, x2, y2 }
	end
	-- Back up: out of its right side, round, into the right side of the
	-- one it returns to.
	local sx, sy = a.x + W, a.y + height(a) / 2
	local tx, ty = b.x + W, b.y + height(b) / 2
	local reach = 56 + (i % 4) * 16 + math.abs(sy - ty) * 0.12
	return { sx, sy, sx + reach, sy, tx + reach, ty, tx, ty }
end

local function at(p: { number }, t: number): (number, number)
	local s = 1 - t
	return s ^ 3 * p[1] + 3 * s ^ 2 * t * p[3] + 3 * s * t ^ 2 * p[5] + t ^ 3 * p[7],
		s ^ 3 * p[2] + 3 * s ^ 2 * t * p[4] + 3 * s * t ^ 2 * p[6] + t ^ 3 * p[8]
end

-- How far (x, y) is from a curve, in the field's units.
local function distance(x: number, y: number, p: { number }): number
	local best = math.huge
	local ax, ay = p[1], p[2]
	for i = 1, 32 do
		local bx, by = at(p, i / 32)
		local dx, dy = bx - ax, by - ay
		local len = dx * dx + dy * dy
		local t = if len == 0 then 0 else math.clamp(((x - ax) * dx + (y - ay) * dy) / len, 0, 1)
		best = math.min(best, math.sqrt((x - ax - t * dx) ^ 2 + (y - ay - t * dy) ^ 2))
		ax, ay = bx, by
	end
	return best
end

-- A connection's label: nothing when it's always followed.
local function edge_label(e: any): string?
	if e.condition == "pass" then
		return locale.s("orch.flow.pass")
	elseif e.condition == "fail" then
		return if e.max_retries
			then locale.s("orch.flow.fail_retries", { n = e.max_retries })
			else locale.s("orch.flow.fail")
	end
	return nil
end

-- "codex/gpt-6" as its model's part.
local function model_line(n: any): string
	if n.kind == "chief" or n.kind == "finish" then
		return ""
	end
	if n.model ~= "" then
		return string.match(n.model, "^[^/]+/(.+)$") or n.model
	end
	return if n.harness ~= "" then n.harness else locale.s("orch.flow.default_model")
end

local function first_line(s: string): string
	return string.match(s, "^%s*([^\n]*)") or ""
end

export type Props = {
	flow: () -> any,
	on_change: (any) -> (),
	hue: (() -> number?)?,
	scope: (() -> string?)?,
	models: (() -> { any })?,
	on_import: (() -> ())?,
	on_export: (() -> ())?,
	on_save_template: (() -> ())?,
	on_start: ((kind: string) -> ())?,
	run: (() -> any)?,
	on_run: ((task: string) -> ())?,
	on_stop: (() -> ())?,
}

-- A run's states that are under way.
local LIVE: { [string]: boolean } = { running = true, blocked = true }

function M.FlowEditor(props: Props): Node
	return M.Editor(props)
end

M.Editor = ui.component(function(props: Props): Node
	local view = ui.use_state({ x = 40, y = 24, zoom = 0.9 })
	local size = ui.use_state({ w = 800, h = 600 })
	local selected = M.selected
	-- Another orchestrator's flow: nothing of the last one stays selected.
	local scoped: string? = nil
	ui.effect(function()
		local scope = props.scope
		local now = if scope then scope() else nil
		if scoped ~= nil and now ~= scoped then
			selected:set({})
		end
		scoped = now
	end)
	-- A connection being drawn, from a block to the pointer (field units).
	local wire = ui.use_state(nil :: { from: string, x: number, y: number }?)
	-- A block being moved, as it is now (applied once it's let go).
	local moving = ui.use_state(nil :: { id: string, x: number, y: number }?)
	local drag: any = nil
	-- A block pressed: its sheet opens as it's let go, unless it was moved.
	local pending_sheet = false
	-- Whether the person has panned or zoomed (else it fits as it resizes).
	local touched = false
	local add_kind = ui.use_state("")
	-- The editor's own width: narrower than NARROW_AT, what's selected opens
	-- in a sheet rather than a column beside the field.
	local width = ui.use_state(1200)
	local sheet_open = ui.use_state(false)
	local function narrow(): boolean
		return width:get() < NARROW_AT
	end

	local function hue_ink(): string
		local of_hue = props.hue
		local h = if of_hue then of_hue() else nil
		return if h then hue.canvas(h) else "$accent"
	end

	local function current(): any
		return props.flow()
	end

	-- A block where it's drawn: moved, while it's being moved.
	local function placed(n: any): any
		local m = moving:get()
		if m and m.id == n.id then
			return { id = n.id, kind = n.kind, name = n.name, x = m.x, y = m.y, model = n.model, harness = n.harness, instructions = n.instructions }
		end
		return n
	end

	local function nodes_by_id(f: any): { [string]: any }
		local out = {}
		for _, n in f.nodes do
			out[n.id] = placed(n)
		end
		return out
	end

	local issues = ui.computed(function(): { any }
		local f = current()
		return if f then flow.check(f) else {}
	end)
	local faulty = ui.computed(function(): { [string]: boolean }
		local out = {}
		for _, i in issues:get() do
			if i.id then
				out[i.id] = true
			end
		end
		return out
	end)

	-- Its run as it stands, and that run while it's under way.
	local run_now = ui.computed(function(): any
		local r = props.run
		return if r then r() else nil
	end)
	local function live(): any
		local r = run_now:get()
		return if r and LIVE[r.state or ""] then r else nil
	end
	local run_open = ui.use_state(false)
	local task = ui.use_state("")

	local function change(fn: (any) -> boolean?)
		local f = current()
		if not f then
			return
		end
		local next_flow = flow.copy(f)
		if fn(next_flow) ~= false then
			props.on_change(next_flow)
		end
	end

	-- Field units under a point of the field's box.
	local function field(lx: number, ly: number): (number, number)
		local v = view:peek()
		return (lx - v.x) / v.zoom, (ly - v.y) / v.zoom
	end

	local function zoom_at(px: number, py: number, k: number)
		touched = true
		local v = view:peek()
		local z = math.clamp(v.zoom * k, ZOOM_MIN, ZOOM_MAX)
		local wx, wy = (px - v.x) / v.zoom, (py - v.y) / v.zoom
		view:set({ x = px - wx * z, y = py - wy * z, zoom = z })
	end

	local function fit()
		local f = current()
		if not f or #f.nodes == 0 then
			return
		end
		local l: number, t: number, r: number, b: number = math.huge, math.huge, -math.huge, -math.huge
		local by = nodes_by_id(f)
		for _, n in f.nodes do
			local nx: number, ny: number, nh: number = n.x, n.y, height(n)
			l, t = math.min(l, nx), math.min(t, ny)
			r, b = math.max(r, nx + W), math.max(b, ny + nh)
		end
		for i, e in f.edges do
			local a, z = by[e.from], by[e.to]
			if a and z then
				local p = curve(a, z, i)
				local px: number = math.max(p[3], p[5])
				r = math.max(r, px)
			end
		end
		local s: { w: number, h: number } = size:peek()
		local left: number, top: number = l :: number, t :: number
		local span_w: number, span_h: number = (r :: number) - left, (b :: number) - top
		local z = math.clamp(math.min((s.w - 64) / math.max(span_w, 1), (s.h - 64) / math.max(span_h, 1)), ZOOM_MIN, 1.2)
		view:set({ x = (s.w - span_w * z) / 2 - left * z, y = (s.h - span_h * z) / 2 - top * z, zoom = z })
	end

	local function press(e: any)
		local f = current()
		if not f then
			return
		end
		local x, y = field(e.local_x or 0, e.local_y or 0)
		for i = #f.nodes, 1, -1 do
			local n = f.nodes[i]
			if n.kind ~= "finish" and (x - (n.x + W / 2)) ^ 2 + (y - (n.y + height(n))) ^ 2 <= HIT_PORT ^ 2 then
				selected:set({ node = n.id })
				drag = { kind = "wire", from = n.id }
				wire:set({ from = n.id, x = x, y = y })
				return
			end
			if x >= n.x and x <= n.x + W and y >= n.y and y <= n.y + height(n) then
				selected:set({ node = n.id })
				pending_sheet = true
				drag = { kind = "node", id = n.id, dx = x - n.x, dy = y - n.y }
				return
			end
		end
		local by = nodes_by_id(f)
		local best, near = 0, 8 / view:peek().zoom
		for i, edge in f.edges do
			local a, b = by[edge.from], by[edge.to]
			if a and b then
				local d = distance(x, y, curve(a, b, i))
				if d < near then
					best, near = i, d
				end
			end
		end
		if best > 0 then
			selected:set({ edge = best })
			if narrow() then
				sheet_open:set(true)
			end
			return
		end
		selected:set({})
		local v = view:peek()
		drag = { kind = "pan", x = e.local_x or 0, y = e.local_y or 0, vx = v.x, vy = v.y }
	end

	local function move(e: any)
		if not drag then
			return
		end
		if drag.kind == "pan" then
			touched = true
			local v = view:peek()
			view:set({ x = drag.vx + (e.local_x or 0) - drag.x, y = drag.vy + (e.local_y or 0) - drag.y, zoom = v.zoom })
		elseif drag.kind == "wire" then
			local x, y = field(e.local_x or 0, e.local_y or 0)
			wire:set({ from = drag.from, x = x, y = y })
		elseif drag.kind == "node" then
			local x, y = field(e.local_x or 0, e.local_y or 0)
			moving:set({ id = drag.id, x = math.floor(x - drag.dx + 0.5), y = math.floor(y - drag.dy + 0.5) })
		end
	end

	local function release(e: any)
		local d = drag
		drag = nil
		if not d then
			return
		end
		if d.kind == "wire" then
			local x, y = field(e.local_x or 0, e.local_y or 0)
			wire:set(nil)
			local f = current()
			if not f then
				return
			end
			for _, n in f.nodes do
				if x >= n.x and x <= n.x + W and y >= n.y - HIT_PORT and y <= n.y + height(n) then
					if flow.can_connect(f, d.from, n.id) then
						change(function(g: any): boolean?
							return flow.connect(g, d.from, n.id)
						end)
						selected:set({ edge = #f.edges + 1 })
					end
					return
				end
			end
		elseif d.kind == "node" then
			local m = moving:peek()
			-- A block pressed and let go where it was: open, when narrow.
			if not m and pending_sheet and narrow() then
				sheet_open:set(true)
			end
			pending_sheet = false
			if m then
				change(function(g: any): boolean?
					local n = flow.node(g, m.id)
					if not n or (n.x == m.x and n.y == m.y) then
						return false
					end
					n.x, n.y = m.x, m.y
					return true
				end)
			end
			moving:set(nil)
		end
	end

	local function draw(c: any, _w: number, _h: number)
		local f = current()
		if not f then
			return
		end
		local by = nodes_by_id(f)
		local sel = selected:get()
		local ink = hue_ink()
		local bad = faulty:get()
		local r = run_now:get()
		local at_id = if r and LIVE[r.state or ""] then r.at else nil
		local done = (r and r.done) or {}
		-- Connections first, under the cards.
		for i, e in f.edges do
			local a, b = by[e.from], by[e.to]
			if a and b then
				local p = curve(a, b, i)
				local on = sel.edge == i
				local color = if on then ink else "$text.tertiary"
				local path = ui.path():move(p[1], p[2]):cubic(p[3], p[4], p[5], p[6], p[7], p[8])
				c:stroke(path, color, if on then 2 else 1.4, { cap = "round" })
				-- Its way, a small arrowhead just before it arrives.
				local ax, ay = at(p, 0.9)
				local bx, by2 = at(p, 0.94)
				local ang = math.atan2(by2 - ay, bx - ax)
				local s = 6
				c:stroke(
					ui.path()
						:move(bx - s * math.cos(ang - 0.5), by2 - s * math.sin(ang - 0.5))
						:line(bx, by2)
						:line(bx - s * math.cos(ang + 0.5), by2 - s * math.sin(ang + 0.5)),
					color,
					1.6,
					{ cap = "round", join = "round" }
				)
				local label = edge_label(e)
				if label then
					local lx, ly = at(p, 0.5)
					local tw = c:measure_text(label, { size = 11 })
					c:fill(ui.path():rect(lx - tw / 2 - 6, ly - 9, tw + 12, 18, 9), "$background")
					c:stroke(ui.path():rect(lx - tw / 2 - 6, ly - 9, tw + 12, 18, 9), color, 1)
					c:text(label, lx - tw / 2, ly, { size = 11, color = if on then ink else "$text.secondary", baseline = "middle" })
				end
			end
		end
		local w = wire:get()
		if w then
			local a = by[w.from]
			if a then
				local x1, y1 = a.x + W / 2, a.y + height(a)
				local reach = math.max(24, math.abs(w.y - y1) * 0.45)
				c:stroke(ui.path():move(x1, y1):cubic(x1, y1 + reach, w.x, w.y - reach, w.x, w.y), ink, 2, { cap = "round" })
			end
		end
		-- The cards.
		for _, n0 in f.nodes do
			local n = by[n0.id]
			local h = height(n)
			local card = ui.path():rect(n.x, n.y, W, h, 10)
			c:fill(card, "$surface.raised")
			c:stroke(card, if sel.node == n.id then ink else "$border", if sel.node == n.id then 2 else 1)
			local tx = n.x + 38
			local name = n.name
			local room = W - 50
			if c:measure_text(name, { size = 13 }) > room then
				while #name > 1 and c:measure_text(name .. "…", { size = 13 }) > room do
					name = string.sub(name, 1, (utf8.offset(name, -1) or #name) - 1)
				end
				name ..= "…"
			end
			if h == H then
				c:text(name, tx, n.y + 20, { size = 13, color = "$text.primary", baseline = "middle" })
				c:text(model_line(n), tx, n.y + 38, { size = 11, color = "$text.secondary", baseline = "middle" })
				local brief = first_line(n.instructions)
				local brief_room = W - 24
				if c:measure_text(brief, { size = 11 }) > brief_room then
					while #brief > 1 and c:measure_text(brief .. "…", { size = 11 }) > brief_room do
						brief = string.sub(brief, 1, (utf8.offset(brief, -1) or #brief) - 1)
					end
					brief ..= "…"
				end
				c:text(brief, n.x + 12, n.y + 58, { size = 11, color = "$text.tertiary", baseline = "middle" })
			else
				c:text(name, tx, n.y + h / 2, { size = 13, color = "$text.primary", baseline = "middle" })
			end
			-- The step a run is at, ringed (its hue; waiting on you, the
			-- input's); a step it has done, ticked.
			if at_id == n.id then
				local ring = ui.path():rect(n.x - 4, n.y - 4, W + 8, h + 8, 13)
				c:stroke(ring, if r.state == "blocked" then "$zeron.status.input" else ink, 2)
			end
			if done[n.id] and at_id ~= n.id then
				local cx, cy = n.x + W - 14, n.y + 14
				c:circle(cx, cy, 6, "$zeron.status.done")
				c:stroke(
					ui.path():move(cx - 2.6, cy + 0.2):line(cx - 0.6, cy + 2.2):line(cx + 2.8, cy - 1.8),
					"$zeron.check.mark",
					1.6,
					{ cap = "round", join = "round" }
				)
			elseif bad[n.id] then
				c:circle(n.x + W - 12, n.y + 12, 3.5, "$danger")
			end
			-- Ports: in at the top, out at the bottom.
			if n.kind ~= "chief" then
				c:circle(n.x + W / 2, n.y, PORT, "$surface.raised")
				c:circle(n.x + W / 2, n.y, PORT - 1.5, "$text.tertiary")
			end
			if n.kind ~= "finish" then
				c:circle(n.x + W / 2, n.y + h, PORT, "$surface.raised")
				c:circle(n.x + W / 2, n.y + h, PORT - 1.5, if sel.node == n.id then ink else "$text.secondary")
			end
		end
	end

	-- Each block's icon, over its card where the pan and zoom put it.
	local ids = ui.computed(function(): { string }
		local out = {}
		local f = current()
		for _, n in (if f then f.nodes else {}) do
			table.insert(out, n.id)
		end
		return out
	end)
	local function icon_of(id: string): Node
		local function node(): any
			local f = current()
			local n = if f then flow.node(f, id) else nil
			return if n then placed(n) else nil
		end
		return ui.div {
			absolute = true,
			pointer_events = "none",
			left = function(): number
				local n, v = node(), view:get()
				return v.x + ((if n then n.x else 0) + 12) * v.zoom
			end,
			top = function(): number
				local n, v = node(), view:get()
				local h = if n then height(n) else H
				local cy = if h == H then 20 else h / 2
				return v.y + ((if n then n.y else 0) + cy - 9) * v.zoom
			end,
			ui.presence(function(): string
				local n = node()
				return (if n then n.kind else "") .. ":" .. tostring(view:get().zoom)
			end, function(key: string): Node
				local kind = string.match(key, "^([^:]*)") or ""
				local k = flow.KINDS[kind]
				return Icon {
					name = (if k then k.icon else "widget") :: any,
					size = 18 * view:peek().zoom,
					color = "$text.secondary",
				}
			end),
		}
	end

	-- The inspector's three faces.
	local function block_panel(id: string): Node
		local f = current()
		local n: any = if f then flow.node(f, id) else nil
		if not n then
			return ui.div {}
		end
		local name = ui.use_state(n.name :: string)
		local instructions = ui.use_state(n.instructions :: string)
		local model = ui.use_state((if n.model ~= "" then (if n.harness ~= "" then `{n.harness}/{n.model}` else n.model) elseif n.harness ~= "" then `{n.harness}/` else NONE) :: string)
		local function set(fn: (any) -> ())
			change(function(g: any): boolean?
				local m = flow.node(g, id)
				if not m then
					return false
				end
				fn(m)
				return true
			end)
		end
		-- Instructions applied as typing rests.
		local typed = 0
		ui.effect(function()
			local text = instructions:get()
			typed += 1
			local mine = typed
			ui.after(400, function()
				if mine == typed then
					local now = current()
					local m = now and flow.node(now, id)
					if m and m.instructions ~= text then
						set(function(x: any)
							x.instructions = text
						end)
					end
				end
			end)
		end)
		local rows: { any } = {
			TextField {
				value = name,
				label = ui.t("orch.flow.name"),
				w_full = true,
				on_commit = function(text: string)
					local t = string.match(text, "^%s*(.-)%s*$") or ""
					if t ~= "" and t ~= n.name then
						set(function(x: any)
							x.name = string.sub(t, 1, 256)
						end)
					end
				end,
			},
		}
		if n.kind ~= "chief" and n.kind ~= "finish" then
			if n.kind ~= "gate" then
				table.insert(rows, Select {
					value = model,
					label = ui.t("orch.flow.agent_model") :: any,
					options = function(): { any }
						local out: { any } = { { value = NONE, label = locale.s("orch.flow.default_model") } }
						local found = false
						local models = props.models
						for _, o in (if models then models() else {}) do
							table.insert(out, o)
							found = found or o.value == model:peek()
						end
						if not found and model:peek() ~= NONE then
							table.insert(out, { value = model:peek(), label = model:peek() })
						end
						return out
					end,
					size = "sm",
					w_full = true,
					on_change = function(v: any)
						set(function(x: any)
							if v == NONE then
								x.harness, x.model = "", ""
							else
								local h, m = string.match(v, "^([^/]+)/(.*)$")
								x.harness, x.model = h or "", m or v
							end
						end)
					end,
				})
			end
			table.insert(rows, TextArea {
				value = instructions,
				label = ui.t(if n.kind == "gate" then "orch.flow.gate_ask" else "orch.flow.instructions") :: any,
				rows = 5,
				max_rows = 12,
				w_full = true,
			})
		end
		-- Its actions, along its foot: put its instructions back, or take it
		-- out (the entry stays).
		local actions: { any } = {}
		if n.kind ~= "chief" and n.kind ~= "finish" then
			table.insert(actions, Button {
				ui.t("orch.flow.restore_default") :: any,
				id = "orch-flow-restore",
				tone = "secondary",
				size = "sm",
				disabled = function(): boolean
					return instructions:get() == flow.instructions(n.kind)
				end,
				on_click = function()
					instructions:set(flow.instructions(n.kind))
				end,
			})
		end
		table.insert(actions, ui.div { grow = 1 })
		if n.kind ~= "chief" then
			table.insert(actions, Button {
				ui.t("orch.flow.remove_block") :: any,
				id = "orch-flow-remove-block",
				tone = "danger",
				size = "sm",
				on_click = function()
					selected:set({})
					change(function(g: any): boolean?
						flow.remove(g, id)
						return true
					end)
				end,
			})
		end
		if #actions > 1 then
			table.insert(rows, ui.row { w_full = true, gap = "$space.2", items = "center", pt = "$space.1", table.unpack(actions) })
		end
		local mine: { any } = {}
		for _, i in issues:peek() do
			if i.id == id then
				table.insert(mine, Text { locale.s(i.key), style = "caption", tone = "danger", wrap = true })
			end
		end
		local body: { [any]: any } = {
			id = "orch-flow-block",
			w_full = true,
			gap = "$space.3",
			ui.row {
				gap = "$space.2",
				items = "center",
				Icon { name = (flow.KINDS[n.kind] and flow.KINDS[n.kind].icon or "widget") :: any, size = 16, color = "$text.secondary" },
				Text { kind_name(n.kind), style = "label", tone = "secondary" },
			},
		}
		for _, r in rows do
			table.insert(body, r)
		end
		if #mine > 0 then
			table.insert(body, ui.column { w_full = true, gap = "$space.1", table.unpack(mine) })
		end
		return ui.column(body)
	end

	local function edge_panel(index: number): Node
		local f = current()
		local e = f and f.edges[index]
		if not e then
			return ui.div {}
		end
		local a, b = flow.node(f, e.from), flow.node(f, e.to)
		local condition = ui.use_state((e.condition or "always") :: string)
		local retries = ui.use_state((if e.max_retries then tostring(e.max_retries) else "") :: string)
		local function apply()
			change(function(g: any): boolean?
				local x = g.edges[index]
				if not x then
					return false
				end
				x.condition = condition:peek()
				x.max_retries = if x.condition == "fail" then tonumber(retries:peek()) else nil
				return true
			end)
		end
		local retry_options: { any } = { { value = "", label = locale.s("orch.flow.no_limit") } }
		for i = 1, flow.MAX_RETRIES do
			table.insert(retry_options, { value = tostring(i), label = tostring(i) })
		end
		return ui.column {
			id = "orch-flow-connection",
			w_full = true,
			gap = "$space.3",
			-- From one step to the next: the arrow an icon, not a glyph.
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				Text { a and a.name or e.from, style = "heading", wrap = true },
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "arrow-right", size = 16 } },
				Text { b and b.name or e.to, style = "heading", wrap = true },
			},
			Select {
				value = condition,
				label = ui.t("orch.flow.when") :: any,
				options = {
					{ value = "always", label = locale.s("orch.flow.always") },
					{ value = "pass", label = locale.s("orch.flow.pass") },
					{ value = "fail", label = locale.s("orch.flow.fail") },
				},
				size = "sm",
				w_full = true,
				on_change = function(_v: any)
					apply()
				end,
			},
			ui.show(function(): boolean
				return condition:get() == "fail"
			end, ui.column {
				w_full = true,
				gap = "$space.1",
				Select {
					value = retries,
					label = ui.t("orch.flow.retries") :: any,
					options = retry_options,
					size = "sm",
					w_full = true,
					on_change = function(_v: any)
						apply()
					end,
				},
				Text { locale.s("orch.flow.retries_hint"), style = "caption", tone = "tertiary", wrap = true },
			}),
			ui.row {
				w_full = true,
				pt = "$space.1",
				ui.div { grow = 1 },
				Button {
					ui.t("orch.flow.remove_connection") :: any,
					id = "orch-flow-remove-connection",
					tone = "danger",
					size = "sm",
					on_click = function()
						selected:set({})
						change(function(g: any): boolean?
							if not g.edges[index] then
								return false
							end
							table.remove(g.edges, index)
							return true
						end)
					end,
				},
			},
		}
	end

	local function flow_panel(): Node
		local f = current()
		local name = ui.use_state((if f then f.name else "") :: string)
		return ui.column {
			id = "orch-flow-overview",
			w_full = true,
			gap = "$space.3",
			TextField {
				value = name,
				label = ui.t("orch.flow.flow_name"),
				w_full = true,
				on_commit = function(text: string)
					local t = string.match(text, "^%s*(.-)%s*$") or ""
					change(function(g: any): boolean?
						if t == "" or t == g.name then
							return false
						end
						g.name = string.sub(t, 1, 256)
						return true
					end)
				end,
			},
			Text { locale.s("orch.flow.checks"), style = "label", tone = "secondary" },
			ui.presence(function(): string
				local keys = {}
				for _, i in issues:get() do
					table.insert(keys, (i.id or "") .. "/" .. i.key)
				end
				return table.concat(keys, ",")
			end, function(_: string): Node
				local list = issues:peek()
				local now = current()
				if #list == 0 then
					return ui.row {
						gap = "$space.2",
						items = "center",
						Icon { name = "check-circle", size = 14, color = "$zeron.status.done" },
						Text { locale.s("orch.flow.ready"), style = "caption", tone = "secondary" },
					}
				end
				local rows: { any } = {}
				for _, i in list do
					local n = if i.id and now then flow.node(now, i.id) else nil
					table.insert(rows, Text {
						(if n then n.name .. ": " else "") .. locale.s(i.key),
						style = "caption",
						tone = "secondary",
						wrap = true,
					})
				end
				return ui.column { w_full = true, gap = "$space.1", table.unpack(rows) }
			end, { w_full = true }),
			-- (Whose idea it is.)
			Text { locale.s("orch.flow.credit"), id = "orch-flow-credit", style = "caption", tone = "tertiary", wrap = true },
		}
	end

	-- (Each a component of its own: its fields' states are its own.)
	local BlockPanel = ui.component(function(p: { node: string }): Node
		return block_panel(p.node)
	end)
	local EdgePanel = ui.component(function(p: { index: number }): Node
		return edge_panel(p.index)
	end)
	local FlowPanel = ui.component(function(_: any): Node
		return flow_panel()
	end)

	-- What's selected, to change: a block, a connection, or the flow.
	local function inspector_body(pad: boolean): Node
		return ui.presence(function(): string
					local s = selected:get()
					if s.node then
						return "n:" .. s.node
					elseif s.edge then
						return "e:" .. tostring(s.edge) .. ":" .. tostring(#(current() or { edges = {} }).edges)
					end
					return "f"
				end, function(key: string): Node
					local kind, found = string.match(key, "^(%a):?(.*)$")
					local rest: string = found or ""
					local body = if kind == "n" then BlockPanel { node = rest }
						elseif kind == "e" then EdgePanel { index = tonumber(string.match(rest, "^(%d+)")) or 0 }
						else FlowPanel {}
					return ui.column { w_full = true, p = if pad then "$space.4" else nil, body }
				end, { w_full = true })
	end

	-- Wide: a column beside the field.
	local function inspector(): Node
		return ui.column {
			id = "orch-flow-inspector",
			w = INSPECTOR_W,
			shrink = 0,
			h_full = true,
			ui.scroll {
				w_full = true,
				grow = 1,
				min_h = 0,
				fade = { 12, 12 },
				fade_overflow = true,
				inspector_body(true),
			},
		}
	end

	-- Narrow: a sheet over the page, opened by selecting something (or
	-- Details), closed to go back to the field.
	local function inspector_sheet(): Node
		return Sheet {
			open = sheet_open,
			title = function(): string
				local s = selected:get()
				local f = current()
				if s.node and f then
					local n = flow.node(f, s.node)
					return if n then n.name else ""
				elseif s.edge then
					return locale.s("orch.flow.connection")
				end
				return if f then f.name else ""
			end,
			ui.column { id = "orch-flow-sheet", w_full = true, inspector_body(false) },
		}
	end

	local function add(kind: string)
		local f = current()
		if not f or kind == "" then
			return
		end
		local s, v = size:peek(), view:peek()
		-- In the middle of what's in view.
		local x, y = field(s.w / 2, s.h / 2)
		local made: string? = nil
		change(function(g: any): boolean?
			local n = flow.add(g, kind, math.floor(x - W / 2 + 0.5), math.floor(y - H / 2 + 0.5))
			made = if n then n.id else nil
			return n ~= nil
		end)
		if made then
			selected:set({ node = made })
		end
		local _ = v
	end

	local add_options: { any } = { { value = "", label = locale.s("orch.flow.add_block") } }
	for _, kind in flow.ROLES do
		table.insert(add_options, { value = kind, label = kind_name(kind) })
	end

	-- Run…, while it can run; under way, where the run is and Stop.
	local function run_controls(): Node
		return ui.presence(function(): string
			local r = live()
			if r then
				return "live:" .. (r.state or "")
			end
			return if #issues:get() == 0 then "ready" else ""
		end, function(key: string): Node
			if key == "ready" then
				return ui.row {
					pr = "$space.2",
					Button {
						ui.t("orch.flow.run.button") :: any,
						id = "orch-flow-run",
						size = "sm",
						on_click = function()
							run_open:set(true)
						end,
					},
				}
			elseif key == "" then
				return ui.div {}
			end
			local blocked = key == "live:blocked"
			return ui.row {
				pr = "$space.2",
				gap = "$space.1",
				items = "center",
				ui.div {
					id = "orch-flow-run-state",
					pr = "$space.1",
					badge.Badge {
						function(): string
							local r = live()
							local f = current()
							local n = if r and r.at and f then flow.node(f, r.at) else nil
							local step = (r and r.step) or (n and n.name) or ""
							return locale.s(if blocked then "orch.flow.run.waiting" else "orch.flow.run.running", { step = step })
						end,
						color = if blocked then "$zeron.status.input" else "$zeron.status.working",
						dot = true,
						size = "sm",
						shape = "pill",
					},
				},
				Button {
					ui.t("orch.flow.run.stop") :: any,
					id = "orch-flow-stop",
					tone = "secondary",
					size = "sm",
					on_click = function()
						local stop = props.on_stop
						if stop then
							stop()
						end
					end,
				},
			}
		end)
	end

	-- What a run is to work on, asked as it starts.
	local function run_dialog(): Node
		local function go()
			local t = string.match(task:peek(), "^%s*(.-)%s*$") or ""
			local start = props.on_run
			if t == "" or not start then
				return
			end
			start(t)
			task:set("")
			run_open:set(false)
		end
		return Dialog {
			open = run_open,
			title = ui.t("orch.flow.run.title") :: any,
			body = ui.t("orch.flow.run.body") :: any,
			size = "md",
			confirm = { label = ui.t("orch.flow.run.go") :: any, on_click = go, keep_open = true },
			ui.column {
				id = "orch-flow-run-dialog",
				w_full = true,
				TextArea {
					value = task,
					name = ui.t("orch.flow.run.task") :: any,
					placeholder = ui.t("orch.flow.run.task_placeholder") :: any,
					autofocus = true,
					rows = 4,
					max_rows = 10,
					w_full = true,
				},
			},
		}
	end

	local function toolbar(): Node
		-- What adds to it and what's wrong with it; what moves it in and out
		-- of files; the view; and, narrow, the inspector's sheet.
		local file: { any } = {}
		if props.on_import then
			table.insert(file, Button { ui.t("orch.flow.import_button") :: any, id = "orch-flow-import", tone = "ghost", size = "sm", on_click = props.on_import })
		end
		if props.on_export then
			table.insert(file, Button { ui.t("orch.flow.export_button") :: any, id = "orch-flow-export", tone = "ghost", size = "sm", on_click = props.on_export })
		end
		if props.on_save_template then
			table.insert(file, Button { ui.t("orch.flow.save_template") :: any, id = "orch-flow-save-template", tone = "ghost", size = "sm", on_click = props.on_save_template })
		end
		local groups: { any } = {
			Toolbar.Group {
				Select {
					value = add_kind,
					name = ui.t("orch.flow.add_block") :: any,
					options = add_options,
					size = "sm",
					w = 148,
					on_change = function(kind: any)
						if kind ~= "" then
							add(kind)
							add_kind:set("")
						end
					end,
				},
			},
			ui.show(function(): boolean
				return #issues:get() > 0
			end, ui.div {
				id = "orch-flow-issues",
				pl = "$space.2",
				on_click = function()
					selected:set({})
					if narrow() then
						sheet_open:set(true)
					end
				end,
				badge.Badge {
					function(): string
						return locale.s("orch.flow.issues", { n = #issues:get() })
					end,
					tone = "warning",
					size = "sm",
					icon = "danger-triangle",
				},
			}),
			Toolbar.Spacer {},
		}
		if props.on_run then
			table.insert(groups, run_controls())
		end
		if #file > 0 then
			table.insert(groups, Toolbar.Group(file :: any))
		end
		table.insert(groups, Toolbar.Group {
			IconButton { icon = "minus", label = ui.t("orch.flow.zoom_out") :: any, size = "sm", tone = "quiet", on_click = function()
				local s = size:peek()
				zoom_at(s.w / 2, s.h / 2, 1 / 1.2)
			end },
			IconButton { icon = "plus", label = ui.t("orch.flow.zoom_in") :: any, size = "sm", tone = "quiet", on_click = function()
				local s = size:peek()
				zoom_at(s.w / 2, s.h / 2, 1.2)
			end },
			IconButton { icon = "expand-arrows", label = ui.t("orch.flow.fit") :: any, size = "sm", tone = "quiet", on_click = function()
				touched = false
				fit()
			end },
		})
		table.insert(groups, ui.show(narrow, IconButton {
			id = "orch-flow-details",
			icon = "tuning",
			label = ui.t("orch.flow.details") :: any,
			size = "sm",
			tone = "quiet",
			on_click = function()
				sheet_open:set(true)
			end,
		}))
		return ui.row {
			id = "orch-flow-toolbar",
			w_full = true,
			shrink = 0,
			Toolbar.Toolbar {
				label = ui.t("orch.flow.toolbar") :: any,
				look = "bar",
				table.unpack(groups),
			},
		}
	end

	-- Nothing yet: what it can start from.
	local function empty(): Node
		return ui.column {
			id = "orch-flow-empty",
			w_full = true,
			grow = 1,
			items = "center",
			justify = "center",
			gap = "$space.3",
			Icon { name = "git-branch", size = 28, color = "$text.tertiary" },
			Text { locale.s("orch.flow.empty"), tone = "secondary" },
			ui.row {
				gap = "$space.2",
				Button { ui.t("orch.flow.start_default") :: any, size = "sm", on_click = function()
					local start = props.on_start
					if start then
						start("default")
					end
				end },
				Button { ui.t("orch.flow.start_blank") :: any, tone = "secondary", size = "sm", on_click = function()
					local start = props.on_start
					if start then
						start("blank")
					end
				end },
				if props.on_import then Button { ui.t("orch.flow.import_button") :: any, tone = "ghost", size = "sm", on_click = props.on_import } else nil,
			},
		}
	end

	local function field_box(): Node
		return ui.div {
			id = "orch-flow-field",
			grow = 1,
			min_w = 0,
			h_full = true,
			relative = true,
			overflow = "hidden",
			on_resize = function(s: { w: number, h: number })
				size:set({ w = s.w, h = s.h })
				-- Fitted as it's laid out, until the person moves the view.
				if not touched and s.w > 0 then
					ui.after(0, fit)
				end
			end,
			on_press = press,
			on_pointer_move = move,
			on_release = release,
			on_wheel = function(e: any): boolean
				if e.ctrl or e.cmd then
					zoom_at(e.local_x or 0, e.local_y or 0, math.exp(-(e.dy or 0) * 0.01))
				else
					touched = true
					local v = view:peek()
					view:set({ x = v.x - (e.dx or 0), y = v.y - (e.dy or 0), zoom = v.zoom })
				end
				return true
			end,
			on_pinch = function(e: any): boolean
				zoom_at(e.local_x or 0, e.local_y or 0, e.scale or 1)
				return true
			end,
			ui.canvas {
				id = "orch-flow-canvas",
				absolute = true,
				left = 0,
				top = 0,
				right = 0,
				bottom = 0,
				pointer_events = "none",
				label = function(): string
					local f = current()
					return if f then f.name else ""
				end,
				view = view,
				bg_grid = { step = 24, min_step = 12, dot = 1, color = "$border.subtle" },
				draw = draw,
			},
			ui.div {
				absolute = true,
				left = 0,
				top = 0,
				right = 0,
				bottom = 0,
				pointer_events = "none",
				ui.each(ids, icon_of, { key = function(id: string): string
					return id
				end }),
			},
		}
	end

	return ui.column {
		id = "orch-flow",
		w_full = true,
		h_full = true,
		on_resize = function(s: { w: number, h: number })
			if math.abs(s.w - width:peek()) >= 1 then
				width:set(s.w)
			end
		end,
		ui.presence(function(): string
			return if current() then "flow" else "empty"
		end, function(kind: string): Node
			if kind == "empty" then
				return empty()
			end
			return ui.column {
				w_full = true,
				h_full = true,
				toolbar(),
				ui.row {
					w_full = true,
					grow = 1,
					min_h = 0,
					field_box(),
					ui.div { w = 1, h_full = true, shrink = 0, bg = "$border", hidden = narrow },
					ui.show(function(): boolean
						return not narrow()
					end, inspector()),
				},
				ui.show(narrow, inspector_sheet()),
			}
		end, { w_full = true, h_full = true }),
		if props.on_run then run_dialog() else nil,
	}
end)

return M
