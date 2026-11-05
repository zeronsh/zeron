-- What there is to update, wherever: on the top bar, beside the panel toggle,
-- a button that only shows when there's something, wearing the marks of what's
-- waiting (three at most) and the download glyph; pressed, the list, by device,
-- each with what it can do. Zeron itself is in it, this Mac's (`about`) and any
-- other device's (src/updates.rs), and so is every agent on every device.
--
--   Updates.Button {}    the button and its panel
--   Updates.width()      how much of the bar it takes (0 with nothing to say)
--
-- What each row offers is as the shipping app's agent list has it
-- (crates/ui/src/shell/harness_updates.rs): Update, View steps, Cancel, Retry
-- update, Check again.

local ds = require("@ds")
local sessions = require("@account/sessions")
local locale = require("@ds/locale")
local Menu = require("@ds/menu")
local toast = require("@ds/toast")
local status = require("@ds/status")
local route = require("@app/screens/shell/route")

local M = {}

-- The bar's tab height and its tile, as a session's tab (shell/session_tabs).
-- The gap between circles is cut out of the one behind, not painted: what's
-- under the button shows through it, whatever the button's state or the theme.
local BUTTON_H, TILE, GAP_CUT = 28, 20, 2.5
-- How far along each circle stands: enough of the one behind shows to know it.
local STEP = 15
-- A circle's mark: a little right of its middle on those behind, to sit in the
-- middle of what shows of them.
local MARK_SIDE, MARK_NUDGE = 11, 2
local MOST = 3
local GLYPH = 16
-- The button's insets: its circles as far in as they are from its top, its
-- glyph clear of its end.
local INSET_L, INSET_R, GAP = 4, 6, 5
local WIDTH, MARK, ACTION_H = 340, 28, 26
local MOST_ROWS_H = 340

local agents = ui.shared("updates", { rows = {} })
local about = ui.shared("about", { app_version = "", available = false })

-- This Mac's Zeron, as About & updates has it: nothing unless it can update
-- itself and a release is there for it.
local function zeron_here(): any?
	local a: any = about:get()
	if not a or a.self_updating ~= true or a.held ~= nil then
		return nil
	end
	local flow = a.flow or "idle"
	if flow == "installed" or (flow == "idle" and a.available ~= true) then
		return nil
	end
	local row: any = {
		id = "here:zeron",
		device = "",
		device_name = "",
		here = true,
		connected = true,
		kind = "zeron",
		installed = a.app_version,
		latest = a.flow_version or a.latest,
		phase = if flow == "downloading"
			then "downloading"
			elseif flow == "ready" then "ready"
			elseif flow == "failed" then "failed"
			else "available",
		active = flow == "downloading",
		message = a.flow_error,
		action = if flow == "ready"
			then "restart"
			elseif flow == "failed" then "retry"
			elseif flow == "idle" then "update"
			else nil,
	}
	return row
end

-- Everything there is to say: this Mac's Zeron, others', then the agents.
local rows = ui.computed(function(): { any }
	local out: { any } = {}
	local here = zeron_here()
	if here then
		table.insert(out, here)
	end
	local given: any = agents:get()
	for _, r in (given and given.rows) or {} do
		table.insert(out, r)
	end
	return out
end)

local function shown(): boolean
	return #rows:get() > 0
end

-- What the stack shows: each thing once (Zeron on three devices is one Zeron),
-- three at most.
local marks = ui.computed(function(): { any }
	local out: { any } = {}
	local seen: { [string]: boolean } = {}
	for _, r in rows:get() do
		local what = if r.kind == "zeron" then "zeron" else r.harness or r.id
		if not seen[what] then
			seen[what] = true
			table.insert(out, r)
			if #out == MOST then
				break
			end
		end
	end
	return out
end)

local function stack_w(): number
	return TILE + STEP * (math.max(#marks:get(), 1) - 1)
end

function M.width(): number
	return if shown() then INSET_L + stack_w() + GAP + GLYPH + INSET_R else 0
end

-- What the button's glyph says of them all.
local function state(): string
	local list = rows:get()
	local all_done = #list > 0
	for _, r in list do
		if r.active then
			return "working"
		end
		if r.phase ~= "updated" then
			all_done = false
		end
	end
	return if all_done then "done" else "download"
end

-- A row's mark on its tile, whatever it's on.
local function mark_of(r: any, size: number, bare: boolean?): Node
	if r.kind == "zeron" then
		if bare then
			return ui.svg { src = "assets/icons/zeron-mark.svg", w = size, h = size, shrink = 0 }
		end
		-- (Its logo on the white tile the agents' marks wear.)
		return ui.row {
			w = size,
			h = size,
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = ui.raw(size * 0.25, "as a mark's tile"),
			bg = "$zeron.mark.bg",
			border = 1,
			border_color = "$zeron.mark.border",
			shadow = "$shadow.sm",
			ui.svg { src = "assets/icons/zeron-mark.svg", w = size * 0.62, h = size * 0.62, shrink = 0 },
		}
	end
	return ds.AgentMark {
		label = r.name or "",
		mark = r.mark,
		icon = r.agent_icon,
		seed = r.harness,
		size = size,
		bare = bare,
	}
end

local function name_of(r: any): string
	return if r.kind == "zeron" then "Zeron" else r.name or ""
end

-- Running chats on its device: updating its Zeron restarts its engine, and
-- ends them.
local function running_on(device: string, here: boolean): number
	local n = 0
	for _, c in sessions.now:get().chats or {} do
		if (c.device == device or (here and device == "")) and (c.status == "working" or c.status == "input") then
			n += 1
		end
	end
	return n
end

-- The line under its name.
local function line_of(r: any): string
	if not r.connected then
		return locale.s("shell.updates.offline")
	end
	local p = r.phase
	if p == "failed" then
		return r.message or locale.s("shell.updates.failed")
	elseif p == "waiting" then
		return locale.s("shell.updates.waiting")
	elseif p == "preparing" then
		return locale.s("shell.updates.preparing")
	elseif p == "downloading" then
		return r.message or locale.s("shell.updates.downloading")
	elseif p == "installing" then
		return locale.s("shell.updates.installing")
	elseif p == "verifying" then
		return locale.s("shell.updates.verifying")
	elseif p == "ready" then
		return locale.s("shell.updates.ready", { version = r.latest or "" })
	elseif p == "updated" then
		return if r.installed
			then locale.s("shell.updates.updated_to", { version = r.installed })
			else locale.s("shell.updates.updated")
	end
	local versions = if r.installed and r.latest
		then `{r.installed} → {r.latest}`
		elseif r.latest then locale.s("shell.updates.version", { version = r.latest })
		else locale.s("shell.updates.available")
	if r.kind == "zeron" and not r.here then
		if r.action == nil then
			return locale.s("shell.updates.update_there", { versions = versions })
		end
		local n = running_on(r.device, false)
		if n > 0 then
			return locale.s("shell.updates.stops_chats", { n = n })
		end
	end
	return versions
end

local function label_of(r: any): string
	local a = r.action
	if a == "update" then
		if r.kind == "zeron" and not r.here and running_on(r.device, false) > 0 then
			return locale.s("shell.updates.update_anyway")
		end
		return locale.s("shell.updates.update")
	elseif a == "restart" then
		return locale.s("shell.updates.restart")
	elseif a == "steps" then
		return locale.s("shell.updates.view_steps")
	elseif a == "cancel" then
		return locale.s("common.cancel")
	elseif a == "retry" then
		return locale.s("shell.updates.retry")
	elseif a == "check" then
		return locale.s("shell.updates.check_again")
	end
	return ""
end

local function failed(what: string, err: any)
	toast.show {
		title = locale.s("shell.updates.failed_named", { name = what }),
		body = if type(err) == "string" then err else nil,
		tone = "danger",
	}
end

-- What pressing its action does.
local function act(r: any, close: () -> ())
	local a = r.action
	if a == "steps" then
		close()
		pcall(ui.invoke, "agents.target", if r.here then nil else r.device)
		route.settings("providers")
		return
	end
	local name = name_of(r)
	if r.kind == "zeron" and r.here then
		ui.call("about.apply", {}):after(function() end, function(err: any)
			failed(name, err)
		end)
	elseif r.kind == "zeron" then
		ui.call("updates.apply_zeron", { device = r.device }):after(function() end, function(err: any)
			failed(name, err)
		end)
	else
		ui.call("updates.run", { device = r.device, harness = r.harness, action = a }):after(function() end, function(err: any)
			failed(name, err)
		end)
	end
end

-- A row's action, quiet in a list of them: solid only where it's the one thing
-- to do (a restart to finish; a list of one).
local function action_button(r: any, close: () -> ()): Node
	local primary = r.action == "restart" or (#rows:peek() == 1 and (r.action == "update" or r.action == "retry"))
	local enabled = r.connected or r.action == "steps"
	return ui.button {
		id = `update-action-{r.id}`,
		label = `{label_of(r)} · {name_of(r)}`,
		disabled = not enabled,
		h = ACTION_H,
		px = ui.raw(10, "an action's inset"),
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		bg = if primary then "$text.primary" else "$hover",
		color = if primary then "$background" else "$text.secondary",
		opacity = if enabled then 1 else 0.45,
		hover = if not enabled then nil elseif primary then { opacity = 0.85 } else { color = "$text.primary" },
		cursor = if enabled then "pointer" else nil,
		on_click = function()
			act(r, close)
		end,
		ui.text { label_of(r), literal = true, text_size = "$font.size.xs", weight = "$font.weight.medium" },
	}
end

local function row(r: any, close: () -> ()): Node
	return ui.row {
		id = `update-row-{r.id}`,
		w_full = true,
		min_h = ui.raw(52, "a row of two lines and its room"),
		px = "$space.3",
		py = "$space.2",
		gap = "$space.3",
		items = "center",
		ui.div { shrink = 0, mark_of(r, MARK) },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = ui.raw(1, "a name and its line, one thing"),
			ui.text { name_of(r), literal = true, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.primary", truncate = true },
			ui.text {
				line_of(r),
				literal = true,
				text_size = "$font.size.xs",
				color = if r.phase == "failed" and r.connected then "$danger" else "$text.secondary",
				tabular = true,
				truncate = true,
			},
		},
		if r.active then ui.div { shrink = 0, status.StatusIcon { kind = "working", size = 14 } } else nil,
		if r.phase == "updated" then ui.div { shrink = 0, color = "$success", ds.Icon { name = "check", size = 14 } } else nil,
		if r.action then action_button(r, close) else nil,
	}
end

-- By device: this Mac's first, as they come. One device needs no heading.
local function by_device(list: { any }): { { name: string, rows: { any } } }
	local out: { { name: string, rows: { any } } } = {}
	local at: { [string]: number } = {}
	for _, r in list do
		local key = if r.here then "" else r.device
		local i = at[key]
		if not i then
			table.insert(out, { name = if r.here then locale.s("shell.updates.this_device") else r.device_name, rows = {} })
			i = #out
			at[key] = i
		end
		table.insert(out[i].rows, r)
	end
	return out
end

-- What the list says now: it's made again as this changes (a short list).
local function signature(): string
	local parts: { string } = {}
	for _, r in rows:get() do
		table.insert(parts, table.concat({
			r.id,
			r.phase or "",
			r.action or "",
			r.message or "",
			r.installed or "",
			r.latest or "",
			tostring(r.connected),
			tostring(if r.kind == "zeron" and not r.here then running_on(r.device, false) else 0),
		}, "\0"))
	end
	return table.concat(parts, "\n")
end

local function update_all()
	for _, r in rows:peek() do
		if r.kind == "agent" and r.action == "update" and r.connected then
			act(r, function() end)
		end
	end
end

local function can_update_all(): boolean
	local n = 0
	for _, r in rows:get() do
		if r.kind == "agent" and r.action == "update" and r.connected then
			n += 1
		end
	end
	return n > 1
end

local function panel(open: any, anchor: any): Node
	local function close()
		open:set(false)
	end
	return ui.popover(Menu.panel {
		class = "updates-panel",
		open = open,
		anchor = anchor,
		placement = { side = "bottom", align = "end", offset = 6 },
		w = WIDTH,
		build = function()
			return ui.column {
				w_full = true,
				ui.row {
					w_full = true,
					h = ui.raw(44, "the panel's head"),
					pl = "$space.3",
					pr = "$space.2",
					items = "center",
					justify = "between",
					ui.text { ui.t("shell.updates.title"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.primary" },
					ui.show(
						can_update_all,
						ui.button {
							id = "updates-all",
							label = ui.t("shell.updates.update_all"),
							h = ACTION_H,
							px = ui.raw(10, "an action's inset"),
							items = "center",
							rounded = "$radius.full",
							bg = "$text.primary",
							color = "$background",
							hover = { opacity = 0.85 },
							cursor = "pointer",
							on_click = update_all,
							ui.text { ui.t("shell.updates.update_all"), text_size = "$font.size.xs", weight = "$font.weight.medium" },
						}
					),
				},
				ui.scroll {
					w_full = true,
					max_h = MOST_ROWS_H,
					pb = "$space.1",
					ui.presence(signature, function(_: string): Node
						local groups = by_device(rows:peek())
						local parts: { any } = { w_full = true }
						for _, g in groups do
							if #groups > 1 then
								table.insert(parts, ui.text {
									g.name,
									literal = true,
									px = "$space.3",
									pt = "$space.2",
									text_size = "$font.size.xs",
									color = "$text.tertiary",
									truncate = true,
								})
							end
							for _, r in g.rows do
								table.insert(parts, row(r, close))
							end
						end
						return ui.column(parts)
					end, { w_full = true }),
				},
			}
		end,
	})
end

-- A circle behind another, as a crescent: the whole circle less the one in
-- front of it and the gap around that.
local CRESCENT = (function(): string
	local r, far = TILE / 2, TILE / 2 + GAP_CUT
	local a = (STEP * STEP + r * r - far * far) / (2 * STEP)
	local h = math.sqrt(r * r - a * a)
	local x, top, bottom = r - a, r - h, r + h
	return `code:<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {TILE} {TILE}"><path fill="#ffffff" d="M{x} {top}A{r} {r} 0 1 1 {x} {bottom}A{far} {far} 0 0 0 {x} {top}Z"/></svg>`
end)()

-- A circle of the stack: the mark's own tile colour, the mark on it. Ink-only
-- marks take their tile's ink, as on the list's tiles.
local function tile(at: number): Node
	local behind = at > 1
	return ui.show(function(): boolean
		return #marks:get() >= at
	end, ui.div {
		id = `updates-mark-{at}`,
		absolute = true,
		top = 0,
		left = (at - 1) * STEP,
		w = TILE,
		h = TILE,
		if behind
			then ui.svg { src = CRESCENT, tint = "$zeron.mark.bg", absolute = true, left = 0, top = 0, w = TILE, h = TILE }
			else ui.div { absolute = true, left = 0, top = 0, w = TILE, h = TILE, rounded = "$radius.full", bg = "$zeron.mark.bg" },
		ui.row {
			absolute = true,
			left = if behind then MARK_NUDGE else 0,
			top = 0,
			w = TILE,
			h = TILE,
			items = "center",
			justify = "center",
			color = "#000000",
			ui.presence(function(): string
				local r = marks:get()[at]
				return if r then r.id else ""
			end, function(id: string): Node
				for _, r in marks:peek() do
					if r.id == id then
						return mark_of(r, MARK_SIDE, true)
					end
				end
				return ui.div {}
			end),
		},
	})
end

M.Button = ui.component(function(): Node
	local open = ui.use_state(false)
	local hovered = ui.use_state(false)
	-- (Nothing left to say: nothing left open.)
	ui.effect(function()
		if not shown() and open:peek() then
			open:set(false)
		end
	end)
	local stack: { any } = { relative = true, shrink = 0, h = TILE, w = stack_w }
	-- (Those behind first: each paints under the one after it.)
	for at = MOST, 1, -1 do
		table.insert(stack, tile(at))
	end
	local trigger = ui.row {
			id = "updates-button",
			role = "button",
			label = function(): string
				return locale.s("shell.updates.button", { n = #rows:get() })
			end,
			expanded = open,
			h = BUTTON_H,
			pl = ui.raw(INSET_L, "as far in as they are from its top"),
			pr = ui.raw(INSET_R, "clear of its end"),
			gap = ui.raw(GAP, "its marks and its glyph"),
			items = "center",
			rounded = "$radius.lg",
			cursor = "pointer",
			pointer_events = "auto",
			bg = function(): string
				if open:get() then
					return "$zeron.selected.flat.bg"
				end
				return if hovered:get() then "$hover" else "#00000000"
			end,
			color = function(): string
				return if open:get() or hovered:get() then "$text.primary" else "$text.secondary"
			end,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				open:set(not open:peek())
			end,
			ui.div(stack),
			ui.presence(state, function(which: string): Node
				if which == "working" then
					return status.StatusIcon { kind = "working", size = GLYPH }
				end
				return ds.Icon { name = if which == "done" then "check" else "download", size = GLYPH }
			end, { w = GLYPH, h = GLYPH, shrink = 0 }),
	}
	-- (The popover draws its anchor.)
	return panel(open, trigger)
end)

M.shown = shown

return M
