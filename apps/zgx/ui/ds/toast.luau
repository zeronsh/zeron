-- Toasts: short messages that come and go in the window's bottom corner,
-- over everything, taking no focus. Each is a card of the menus' glass: a
-- tone's glyph, a title and a line, an action, and a × that shows under
-- the pointer. They stack newest at the bottom, rising in; each goes by
-- itself after a while (longer for a warning; a danger stays until put
-- away), paused while the pointer is over the stack. Showing the same
-- message again restarts it rather than adding another.
--
--   toast.show { title = "Pushed to origin", tone = "success" }
--   toast.show { title = "Commit failed", body = "pre-commit hook exited 1",
--     tone = "danger", action = { label = "Show output", run = … } }
--   toast.Host {}   -- once, in the window's root
--
-- `show` returns the toast's id, which `dismiss(id)` puts away.

local motion = require("@self/motion")
local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Button = require("@self/button")
local glass = require("@self/glass")
local control = require("@self/control")
local locale = require("@self/locale")

local M = {}

-- (Its words may be messages, `ui.t`: read as text as it is shown, in the
-- locale then; the stack is a state, which holds no message.)
export type Tone = "neutral" | "info" | "success" | "warning" | "danger"
export type Toast = {
	title: string | Message,
	body: (string | Message)?,
	tone: Tone?,
	action: { label: string | Message, run: () -> () }?,
	-- How long it stays, in milliseconds (0: until put away).
	duration: number?,
}

-- How many show at once; older ones wait their turn.
local MAX = 4
-- How long each tone stays.
local DURATION: { [Tone]: number } = { neutral = 4000, info = 4000, success = 4000, warning = 7000, danger = 0 }
local GLYPH: { [Tone]: Icon.Name } = {
	neutral = "info-circle",
	info = "info-circle",
	success = "check-circle",
	warning = "danger-triangle",
	danger = "danger-triangle",
}

-- The stack: each toast as data, with an id and the time it was (re)shown.
local toasts = ui.state("ds_toasts", {} :: { any })
local hovered = ui.state("ds_toasts_hovered", false)
-- (Kept as state, as the stack is: a hot reload keeps the toasts showing,
-- so a new one mustn't take one of their ids.)
local next_id = ui.state("ds_toasts_next", 0)
-- What the action of each toast runs (functions can't live in state: a
-- toast kept across a hot reload loses its action's button).
local runs: { [number]: () -> () } = {}

function M.dismiss(id: number)
	local i = toasts:index_of("id", id)
	if i then
		toasts:remove(i)
		runs[id] = nil
	end
end

function M.show(given: Toast): number
	-- (Its words as strings, as it is shown.)
	local t = table.clone(given) :: any
	t.title = control.text(given.title) or ""
	t.body = control.text(given.body)
	if given.action then
		t.action = { label = control.text(given.action.label) or "", run = given.action.run }
	end
	local list = toasts:peek()
	-- The same message again: restart it in place.
	for _, existing in list do
		if existing.title == t.title and existing.body == t.body and existing.tone == (t.tone or "neutral") then
			local i = toasts:index_of("id", existing.id)
			if i then
				toasts:set_key({ i, "shown" }, ui.now())
			end
			return existing.id
		end
	end
	local tone: Tone = t.tone or "neutral"
	local id = next_id:peek() + 1
	next_id:set(id)
	if t.action then
		runs[id] = t.action.run
	end
	toasts:push({
		id = id,
		title = t.title,
		body = t.body,
		tone = tone,
		action = if t.action then t.action.label else nil,
		duration = t.duration or DURATION[tone],
		shown = ui.now(),
	})
	return id
end

-- One card. Its timer runs while the stack isn't under the pointer;
-- reshown, it starts again.
local Card = ui.component(function(props: { toast: any }): Node
	local t = props.toast
	local over = ui.use_state(false)
	local tone: Tone = t.tone
	if t.duration > 0 then
		-- From when it was (re)shown, as the stack says now: shown again,
		-- it starts over; built again (a hot reload), it runs on from where
		-- it was. (The stack's rows are made once, as they come.)
		local timer: any = nil
		ui.effect(function()
			local i = toasts:index_of("id", t.id)
			local now = if i then toasts:get()[i] else nil
			if not now then
				return
			end
			if timer then
				timer:cancel()
			end
			timer = anime.createTimer {
				duration = math.max(1, t.duration - (ui.now() - now.shown)),
				onComplete = function()
					M.dismiss(t.id)
				end,
			}
			timer.paused = hovered:peek()
		end)
		ui.effect(function()
			local paused = hovered:get()
			if timer then
				timer.paused = paused
			end
		end)
		ui.on_cleanup(function()
			if timer then
				timer:cancel()
			end
		end)
	end
	-- (Its props typed; the menus' glass laid on them in place.)
	local card: ContainerProps = {
		w = "$zeron.toast.width",
		pl = "$space.4",
		pr = "$space.2",
		py = "$space.3",
		gap = "$space.3",
		items = "start",
		rounded = "$radius.xl",
		role = if tone == "danger" then "alert" else "status",
		live = if tone == "danger" then "assertive" else "polite",
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		ui.div {
			pt = ui.raw(1, "centres the 16 glyph on an 18 line"),
			shrink = 0,
			color = if tone == "neutral" then "$text.secondary" else `$zeron.tone.{tone}.ink`,
			Icon { name = GLYPH[tone], size = 16 },
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.text { t.title, text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary", wrap = true },
			if t.body
				then ui.text { t.body, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary", wrap = true }
				else nil,
			if t.action and runs[t.id]
				then ui.row {
					pt = "$space.2",
					Button {
						t.action,
						size = "sm",
						tone = "secondary",
						on_click = function()
							local run = runs[t.id]
							M.dismiss(t.id)
							if run then
								run()
							end
						end,
					},
				}
				else nil,
		},
		-- The ×: always for a toast that stays, else under the pointer.
		ui.div {
			-- In the corner: as far from the top as from the end, concentric.
			mt = ui.raw(-4, "as far from the top as from the end"),
			theme = { zeron = { icon_button = { radius = control.nest("$radius.xl", 1, "$space.2") } } },
			shrink = 0,
			opacity = if t.duration == 0
				then 1
				else ui.tween(over, { duration = 120 }),
			IconButton {
				icon = "close",
				label = locale.t("common.dismiss"),
				size = "sm",
				tone = "quiet",
				tooltip = false,
				on_click = function()
					M.dismiss(t.id)
				end,
			},
		},
	}
	glass.style(card, "menu")
	return ui.row(card)
end)

-- The stack, in the bottom corner of what holds it: put one in the
-- window's root (an overlay), over the page; `show` fills it. (It is
-- not a zgx layer, which would raise it over dialogs too: zgx draws the
-- modal dim behind a layer for now.)
function M.Host(_props: { [any]: any }?): Node
	local visible = ui.computed(function(): { any }
		local list = toasts:get()
		local out = {}
		for i = math.max(1, #list - MAX + 1), #list do
			table.insert(out, list[i])
		end
		return out
	end)
	return ui.each(visible, function(t: any): Node
		return Card { toast = t }
	end, {
		key = function(t: any)
			return t.id
		end,
		animate = motion.layout({ duration = 260, ease = "out(3)" }),
		enter = motion.transition({ opacity = { 0, 1 }, y = { 16, 0 }, duration = 260, ease = "out(3)" }),
		exit = motion.transition({ opacity = { 1, 0 }, scale = { 1, 0.96 }, duration = 160, ease = "in(2)" }),
		absolute = true,
		-- (Insets take points: space.4 off the corner.)
		right = 16,
		bottom = 16,
		gap = "$space.2",
		items = "end",
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
	})
end

return M
