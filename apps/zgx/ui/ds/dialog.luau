-- Dialogs and sheets: panels that hold the window until they are answered.
-- Both are zgx's modal underneath, so focus is trapped in them (starting
-- on their first control) and goes back where it was as they close. Escape
-- and a click outside close them.
--
-- The modal's own panel is bare: it only places what stands in it, the
-- panel, a denser glass than a menu's (it sits over zgx's dim on the page,
-- not the platform's material). The whole panel moves as it opens and
-- closes, its glass with its content, and the modal stays until it has left.
--
-- A dialog asks one thing in the middle of the window: a title, a line
-- saying what it means, anything of the caller's, then its buttons at the
-- foot, the way on last. Its only marks are its words and its buttons: no
-- badge, no ×. A destructive one (`tone = "danger"`) says so with its red
-- confirm, the focus starting on Cancel so Enter can't take it by accident;
-- `confirm_text` makes the person type a word (the branch's name) first.
-- Any other takes Enter as its confirm.
--
--   Dialog { open = state, title = "Discard 3 changes?", body = "…",
--     tone = "danger", confirm = { label = "Discard", on_click = … } }
--
-- A sheet slides in from the window's end edge (or rises from its foot)
-- for longer work beside the page: a title, a body that scrolls, a foot.
-- With no foot to leave by, it has a × of its own.
--
--   Sheet { open = state, title = "New pull request", foot = …, …content }

local locale = require("@self/locale")
local motion = require("@self/motion")
local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local Icon = require("@self/icon")
local TextField = require("@self/text_field")
local glass = require("@self/glass")
local material = require("@self/material")

local M = {}

-- (Its words may be messages, `ui.t`, drawn as they are.)
-- `keep_open`: the confirm leaves it open (the caller closes it once its
-- work is done); `busy`, while true, the confirm spins in its label's
-- place and takes no click (an install on its way).
export type Action = { label: string | Message | () -> string, on_click: (() -> ())?, keep_open: boolean?, busy: (() -> boolean)? }
export type DialogProps = {
	open: State<boolean>,
	title: string | Message | () -> string,
	body: (string | Message | () -> string)?,
	tone: ("default" | "danger")?,
	-- A glyph over the title, for a dialog that wants one.
	icon: Icon.Name?,
	size: ("sm" | "md" | "lg")?,
	confirm: Action?,
	cancel: (string | Message | false)?,
	-- A third way out, set apart at the foot's start ("Skip these updates").
	aside: Action?,
	-- A word to type before the confirm will go.
	confirm_text: string?,
	-- Whether a click outside closes it (not for one that must be answered).
	dismissable: boolean?,
	-- What stands in it built as it opens, after its children: choices
	-- made from the data as it is then (its focus starts on their first).
	build: (() -> Node)?,
	[number]: Child,
}

-- How the panel arrives and leaves (Zeron's dialog-in, its 180; leaving, a
-- little quicker than it came).
local IN = { duration = 200, ease = "out(3)" }
local OUT = { duration = 130, ease = "in(2)" }

-- The stage: the modal, kept up while the panel leaves. `phase` runs from 0
-- (gone) to 1 (here) and the caller's panel follows it; `panel` builds it,
-- and `w`, `h` and `placement` place it; `ref` is the bare panel's.
local function stage(props: { open: State<boolean>, label: Label, dismissable: boolean, w: any, h: any, placement: any, ref: any?, panel: (phase: State<number>) -> Node }): Node
	local open = props.open
	-- (From where it stands: open across a hot reload, it doesn't arrive
	-- again.)
	local phase = ui.use_state(if open:peek() then 1 else 0)
	local shown = ui.use_state(open:peek())
	local moving: any = nil
	ui.effect(function()
		local want = open:get()
		if moving then
			moving:cancel()
			moving = nil
		end
		if want then
			shown:set(true)
			-- (At once with motion reduced: ds/motion.)
			local opening: TableParams = { value = 1, duration = IN.duration, ease = IN.ease }
			moving = anime.animate(phase, motion.params(opening))
		elseif shown:peek() then
			local closing: TableParams = {
				value = 0,
				duration = OUT.duration,
				ease = OUT.ease,
				onComplete = function()
					shown:set(false)
				end,
			}
			moving = anime.animate(phase, motion.params(closing))
		end
	end)
	return ui.modal {
		open = function()
			return shown:get()
		end,
		on_close = function()
			open:set(false)
		end,
		label = props.label,
		w = props.w,
		h = props.h,
		placement = props.placement,
		p = 0,
		gap = 0,
		rounded = "$zeron.dialog.radius",
		bg = "#00000000",
		border = 0,
		shadow = "$zeron.dialog.stage",
		dismiss_on_backdrop = props.dismissable,
		build = function()
			return ui.column {
				ref = props.ref,
				w_full = true,
				h_full = props.h ~= nil,
				props.panel(phase),
			}
		end,
	}
end

-- The panel: a dialog's glass, corner and shadow.
local function panel(props: ContainerProps): ContainerProps
	glass.style(props, "menu")
	props.bg = function()
		-- (As its glass above: solid where a popup can't blur, or what's behind shows
		-- through sharp.)
		return if material.menus_see_through() then "$zeron.dialog.tint" else "$zeron.dialog.solid"
	end
	props.rounded = "$zeron.dialog.radius"
	props.shadow = "$zeron.dialog.shadow"
	return props
end

-- What a dialog says, built fresh each time it opens (a typed confirmation
-- starts empty).
local DialogBody = ui.component(function(args: { props: DialogProps, close: () -> (), phase: State<number> }): Node
	local props, close, phase = args.props, args.close, args.phase
	local danger = props.tone == "danger"
	local typed = ui.use_state("")
	local function ready(): boolean
		return props.confirm_text == nil or typed:get() == props.confirm_text
	end
	local function go()
		if not ready() then
			return
		end
		local confirm = props.confirm
		-- (Working: Enter, as a click, waits for it.)
		if confirm and confirm.busy and confirm.busy() then
			return
		end
		if confirm and not confirm.keep_open then
			close()
		end
		if confirm and confirm.on_click then
			confirm.on_click()
		end
	end
	local children: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(children, c)
	end
	if props.build then
		table.insert(children, props.build())
	end
	local has_field = #children > 0 or props.confirm_text ~= nil
	return ui.column(panel {
		w_full = true,
		p = "$space.5",
		gap = "$space.5",
		opacity = function()
			return phase:get()
		end,
		scale = function()
			return 0.98 + 0.02 * phase:get()
		end,
		y = function()
			return 4 * (1 - phase:get())
		end,
		on_key = function(e)
			if e.key == "Enter" and not danger and props.confirm then
				go()
				return true
			end
			return false
		end,
		ui.column {
			gap = "$zeron.space.tight",
			if props.icon
				then ui.div { pb = "$zeron.space.tight", color = "$text.secondary", Icon { name = props.icon, size = 20 } }
				else nil,
			ui.text { props.title, heading = 2, text_size = "$font.size.lg", line_height = "$font.line.lg", weight = "$font.weight.semibold", color = "$text.primary", wrap = true },
			if props.body
				then ui.text { props.body, text_size = "$font.size.md", line_height = 20, color = "$text.secondary", wrap = true }
				else nil,
		},
		if #children > 0 then ui.column { gap = "$space.3", table.unpack(children) } else nil,
		if props.confirm_text
			then ui.column {
				gap = "$space.2",
				ui.rich {
					text_size = "$font.size.md",
					line_height = 20,
					color = "$text.secondary",
					ui.span { locale.t("ds.dialog.type_confirm.before") },
					ui.span { props.confirm_text, font = "$font.mono", color = "$text.primary" },
					ui.span { locale.t("ds.dialog.type_confirm.after") },
				},
				TextField {
					value = typed,
					name = locale.t("ds.dialog.type_confirm.name", { word = props.confirm_text }),
					w_full = true,
					autofocus = true,
					-- Enter confirms; leaving the field never does (an input's
					-- commit also fires on leaving it changed: typed, then
					-- Cancel, must not delete anything).
					on_key = function(e: any): boolean?
						if e.key == "Enter" and not (e.shift or e.cmd or e.alt or e.ctrl) then
							go()
							return true
						end
						return nil
					end,
				},
			}
			else nil,
		ui.row {
			gap = "$space.2",
			justify = "end",
			items = "center",
			if props.aside
				then ui.div {
					grow = 1,
					min_w = 0,
					Button {
						props.aside.label,
						tone = "ghost",
						on_click = function()
							close()
							local run = (props.aside :: Action).on_click
							if run then
								run()
							end
						end,
					},
				}
				else nil,
			if props.cancel ~= false
				then Button { (props.cancel or locale.t("common.cancel")) :: any, tone = "secondary", on_click = close }
				else nil,
			if props.confirm
				then Button {
					props.confirm.label,
					tone = if danger then "danger" else "primary",
					busy = props.confirm.busy,
					disabled = function()
						return not ready()
					end,
					-- Nothing to type in: the way on has the focus, but for a
					-- danger's, where that is Cancel, the first control.
					on_mount = if has_field or danger
						then nil
						else function(el: Element)
							ui.after(0, function()
								el:focus()
							end)
						end,
					on_click = go,
				}
				else nil,
		},
	})
end)

M.Dialog = ui.component(function(props: DialogProps): Node
	local open = props.open
	local function close()
		open:set(false)
	end
	return stage {
		open = open,
		label = props.title,
		dismissable = props.dismissable ~= false,
		w = `$zeron.dialog.{props.size or "sm"}`,
		panel = function(phase: State<number>): Node
			return DialogBody { props = props, close = close, phase = phase }
		end,
	}
end)

export type SheetProps = {
	open: State<boolean>,
	title: string | Message | () -> string,
	description: (string | Message | () -> string)?,
	side: ("end" | "bottom")?,
	-- Its width (end) or height (bottom).
	size: number?,
	-- What sits at its foot, over a hairline (its buttons).
	foot: Node?,
	[number]: Child,
}

-- A sheet's way across: its own size, and the gap to the window's edge.
local EDGE = 8

M.Sheet = ui.component(function(props: SheetProps): Node
	local open = props.open
	local bottom = props.side == "bottom"
	local children: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(children, c)
	end
	local width = props.size or (ui.theme.get("zeron.dialog.sheet") :: number)
	-- Tall as the window: zgx's modal has no height to take a percentage
	-- of, but its foot sits `EDGE` off the window's, wherever its top is, so
	-- where that foot lies says how tall the window is.
	local box = ui.ref()
	local foot = ui.use_state(nil :: number?)
	if not bottom then
		ui.effect(function()
			foot:get()
			if box:get() and (box.height or 0) > 0 then
				-- A beat on, its top having settled with its height (and
				-- again while that moves it).
				ui.after(32, function()
					local el = box:peek()
					if el then
						foot:set((el.y or 0) + (box.height or 0))
					end
				end)
			end
		end)
	end
	local function height(): number?
		if bottom then
			return props.size or 360
		end
		local at = foot:get()
		return if at then at - EDGE else nil
	end
	local function travel(): number
		return if bottom then (height() or 0) + EDGE else width + EDGE
	end
	return stage {
		open = open,
		label = props.title,
		dismissable = true,
		w = if bottom then "$zeron.dialog.lg" else width,
		h = height,
		ref = box,
		placement = { x = if bottom then "center" else "end", y = "end", inset = EDGE },
		panel = function(phase: State<number>): Node
			-- (Moved by a box round it: the panel clips its corners, and what
			-- moves inside a clip reads as overflowing it.)
			return ui.column {
				w_full = true,
				h_full = true,
				x = function()
					return if bottom then 0 else travel() * (1 - phase:get())
				end,
				y = function()
					return if bottom then travel() * (1 - phase:get()) else 0
				end,
				ui.column(panel {
				w_full = true,
				h_full = true,
				p = 0,
				gap = 0,
				overflow = "hidden",
				ui.row {
					px = "$space.5",
					pt = "$space.5",
					pb = "$space.4",
					gap = "$space.3",
					items = "start",
					shrink = 0,
					ui.column {
						grow = 1,
						min_w = 0,
						gap = "$zeron.space.tight",
						ui.text { props.title, heading = 2, text_size = "$font.size.lg", line_height = "$font.line.lg", weight = "$font.weight.semibold", color = "$text.primary" },
						if props.description
							then ui.text { props.description, text_size = "$font.size.md", line_height = 20, color = "$text.secondary", wrap = true }
							else nil,
					},
					if props.foot
						then nil
						else ui.div {
							mt = ui.raw(-2, "centres the × on the title's line"),
							mr = ui.raw(-4, "the ×'s glyph on the sheet's padding edge"),
							IconButton {
								icon = "close",
								label = locale.t("common.close"),
								tooltip = false,
								on_click = function()
									open:set(false)
								end,
							},
						},
				},
				ui.scroll {
					grow = 1,
					min_h = 0,
					w_full = true,
					fade = { 12, 12 },
					fade_overflow = true,
					-- The inset on a column inside: a scroll clips inside its
					-- own padding, and its fades would fall short of its edges.
					ui.column { w_full = true, px = "$space.5", pb = "$space.5", gap = "$space.4", table.unpack(children) },
				},
				if props.foot
					then ui.row {
						shrink = 0,
						px = "$space.5",
						py = "$space.4",
						gap = "$space.2",
						justify = "end",
						items = "center",
						border_sides = { 1, 0, 0, 0 },
						border_color = "$border.subtle",
						props.foot,
					}
					else nil,
				}),
			}
		end,
	}
end)

return M
