-- The Control room's Scratchpad: the person's long-term notes and
-- drawings, Markdown and a drawing (ds/orch_scratchpad_draw), saved as they
-- are written (ds/orch_scratchpad_sync: no Save button), with its history
-- to go back to. Edits by another device or the Brain dump come in live.
--
--   Scratchpad.Widget { pad, on_save_text, on_save_shapes, on_restore, on_expand, on_page_wheel? }
--   Scratchpad.Page   { pad, on_save_text, on_save_shapes, on_restore, on_collapse }
--   Scratchpad.Status()   how saving stands ("Saved", "Saving…", "Syncing")
--
-- `pad`: a function of account/scratchpad's `now` ({ loaded, text, shapes,
-- history, … }); `on_save_text(text)` and `on_save_shapes(shapes)` a
-- promise of the rev; `on_restore(id)` puts a snapshot back. The widget is
-- two tabs, Notes and Draw; the page sets them side by side where there's
-- room. Notes is the Markdown as it's typed, or (Preview) as it reads.

local Markdown = require("@self/markdown")
local md_edit = require("@self/markdown_edit")
local Segmented = require("@self/segmented")
local IconButton = require("@self/icon_button").IconButton
local Menu = require("@self/menu")
local Glass = require("@self/glass")
local Block = require("@self/orch_block")
local Sync = require("@self/orch_scratchpad_sync")
local Draw = require("@self/orch_scratchpad_draw")
local material = require("@self/material")
local locale = require("@self/locale")
local status = require("@self/status")

local M = {}

M.Draw = Draw

export type Props = {
	id: string?,
	pad: () -> any,
	on_save_text: (text: string) -> any,
	on_save_shapes: (shapes: { any }) -> any,
	on_restore: (id: string) -> (),
	on_expand: (() -> ())?,
	on_collapse: (() -> ())?,
	-- A wheel the page should have (over the notes while they aren't being
	-- written in): `dy` points, down positive.
	on_page_wheel: ((dy: number) -> ())?,
}

-- The compact widget's room, and the page's.
local WIDGET_H = 320
local PAGE_H = 620
-- Where the page sets Notes and Draw side by side.
M.SPLIT = 960

local function text_sig(v: string): string
	return v
end
local text = Sync.channel("text", "", text_sig)
local shapes = Draw.channel()
M.text = text
M.shapes = shapes

-- Which tab the widget shows (and the page, narrow), and Notes as it reads.
local tab = ui.state("ds_scratchpad_tab", "notes")
local previewing = ui.state("ds_scratchpad_preview", false)
M.tab = tab

function M.Status(): Node
	local function word(): string
		return Sync.state({ text, shapes })
	end
	return ui.text {
		function(): string
			return locale.s(`ds.orch.scratchpad.{word()}`)
		end,
		class = "orch-scratchpad-status",
		tabular = true,
		shrink = 0,
		text_size = "$font.size.xs",
		color = function(): string
			return if word() == "failed" then "$zeron.status.failed" else "$text.tertiary"
		end,
	}
end

-- What the Scratchpad lacks to save, while it can't: "engine" (one without
-- it) or "app" (an app without its host module); nil once a copy came, or
-- while it's still coming.
local function missing(props: Props): string?
	local pad = props.pad()
	if not pad or pad.loaded == true then
		return nil
	end
	if pad.host ~= true then
		return "app"
	end
	return if pad.missing == "engine" then "engine" else nil
end

-- Said plainly while nothing can be saved (typing still works: kept here,
-- saved once the engine has the Scratchpad).
local function Notice(props: Props): Node
	return ui.show(function(): boolean
		return missing(props) ~= nil
	end, ui.text {
		function(): string
			return locale.s(if missing(props) == "app" then "ds.orch.scratchpad.needs_app" else "ds.orch.scratchpad.needs_engine")
		end,
		id = "scratchpad-notice",
		class = "orch-scratchpad-notice",
		w_full = true,
		text_size = "$font.size.sm",
		color = "$zeron.status.failed",
	})
end

-- What every view does: the engine's copy followed, the saves wired, and
-- everything saved as it goes.
local function follow(props: Props)
	text.saver = props.on_save_text
	shapes.saver = props.on_save_shapes
	text.remote = function(): any
		return props.pad().text or ""
	end
	shapes.remote = function(): any
		return props.pad().shapes or {}
	end
	ui.effect(function()
		local gone = missing(props) ~= nil
		local was = Sync.offline:peek()
		if was ~= gone then
			Sync.offline:set(gone)
		end
		-- Back: what was typed meanwhile goes now.
		if was and not gone then
			ui.after(0, function()
				text.save_now()
				shapes.save_now()
			end)
		end
	end)
	ui.effect(function()
		local pad = props.pad()
		if not pad or not pad.loaded then
			return
		end
		text.heard(pad.text or "")
		shapes.heard(pad.shapes or {})
	end)
	ui.on_cleanup(function()
		text.save_now()
		shapes.save_now()
	end)
end

-- Who made a version, as history says it.
local BY: { [string]: string } = { person = "by_you", brain_dump = "by_brain_dump" }
local function by_word(by: string?): string
	return locale.s(`ds.orch.scratchpad.{BY[by or ""] or "by_other"}`)
end

-- History: Undo (the version before this one), then every version, newest
-- first; one chosen is put back (itself a new version: nothing is lost).
local function History(props: Props): Node
	local open = ui.use_state(false)
	local list = ui.computed(function(): { any }
		local pad = props.pad()
		return if pad then pad.history or {} else {}
	end)
	local function restore(id: string)
		open:set(false)
		-- (What's not saved yet goes first, so it's in history too.)
		local left = 2
		local function go()
			left -= 1
			if left == 0 then
				props.on_restore(id)
			end
		end
		text.flush(go)
		shapes.flush(go)
	end
	local button = IconButton {
		id = "scratchpad-history",
		icon = "clock-circle",
		label = locale.t("ds.orch.scratchpad.history"),
		size = "sm",
		tone = "quiet",
		lit = open,
		on_click = function()
			open:set(not open:peek())
		end,
	}
	local function row(_r: any, i: number, r: State<any>): Node
		return Menu.Row {
			class = "orch-scratchpad-version",
			bare = true,
			role = "menuitem",
			label = function(): string
				local p = r:get().preview or ""
				return if p ~= "" then p else locale.s("ds.orch.scratchpad.untitled")
			end,
			checked = function(): boolean
				return i == 1
			end,
			trail = {
				ui.text {
					function(): string
						local v = r:get()
						return `{by_word(v.by)}  {status.ago(status.since(v.edited_at or v.at or 0))}`
					end,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			on_click = function()
				if i > 1 then
					restore(r:peek().id)
				else
					open:set(false)
				end
			end,
		}
	end
	local menu = ui.popover(Glass.style({
		id = "scratchpad-history-menu",
		open = open,
		anchor = button,
		label = locale.t("ds.orch.scratchpad.history"),
		placement = { side = "bottom", align = "end", offset = 4 },
		w = 320,
		p = 0,
		gap = 0,
		rounded = "$radius.lg",
		build = function(): Node
			return ui.column {
				w_full = true,
				Menu.Group {
					Menu.Row {
						id = "scratchpad-undo-version",
						icon = "restart",
						label = locale.t("ds.orch.scratchpad.undo"),
						role = "menuitem",
						-- (Built as the menu opens: what history has then.)
						disabled = #list:peek() < 2,
						on_click = function()
							local h = list:peek()
							if h[2] then
								restore(h[2].id)
							end
						end,
					},
				},
				Menu.Rule {},
				Menu.Group {
					heading = locale.t("ds.orch.scratchpad.history"),
					ui.show(function(): boolean
						return #list:get() == 0
					end, ui.text {
						locale.t("ds.orch.scratchpad.no_history"),
						px = "$space.2",
						py = "$space.1",
						text_size = "$font.size.sm",
						color = "$text.tertiary",
					}),
					ui.each(list, row, { key = function(r: any): string
						return r.id
					end, w_full = true, gap = "$zeron.list.gap" }),
				},
			}
		end,
	}, "menu"))
	-- (The popover draws its anchor, the button, where it stands.)
	return menu
end

-- The plate the editor and the drawing sit on (the cards' quiet fill).
local function plate(): () -> string
	return material.pick("$zeron.settings.note", "$zeron.settings.card")
end

-- Notes: the Markdown as it's typed, or as it reads.
local function Notes(props: Props, h: any, grow: number?): Node
	local fill = plate()
	local reading = ui.debounced(text.value, 150)
	local editing = ui.use_state(false)
	local editor_el: any = nil
	local editor = ui.textarea {
		id = "scratchpad-editor",
		class = "orch-scratchpad-editor",
		value = function(): string
			return text.value:get()
		end,
		on_change = function(t: string)
			text.edit(t)
		end,
		on_blur = function()
			editing:set(false)
			text.save_now()
		end,
		on_mount = function(el: any)
			editor_el = el
		end,
		on_focus = function()
			editing:set(true)
		end,
		-- Escape lets go of it (the wheel back to the page).
		on_key = function(e: any): boolean?
			if e.key == "Escape" and editor_el then
				editing:set(false)
				editor_el:blur()
				return true
			end
			return nil
		end,
		-- Not being written in, a wheel over it is the page's (the notes
		-- scroll themselves once focused, and keep the wheel then).
		on_wheel = function(e: any): boolean?
			local to_page = props.on_page_wheel
			if editing:peek() or not to_page then
				return nil
			end
			to_page(e.dy)
			return true
		end,
		label = locale.s("ds.orch.scratchpad.editor"),
		placeholder = locale.s("ds.orch.scratchpad.placeholder"),
		placeholder_color = "$text.tertiary",
		wrap = true,
		highlight = md_edit.highlight,
		w_full = true,
		h_full = true,
		p = "$space.3",
		bg = "#00000000",
		rounded = "$radius.lg",
		border = 0,
		focus = { border = 0 },
		text_size = "$font.size.md",
		line_height = "$font.line.md",
		color = "$text.primary",
		overscroll = "auto",
	}
	local preview = ui.scroll {
		class = "orch-scratchpad-preview",
		w_full = true,
		h_full = true,
		fade = { 8, 8 },
		fade_overflow = true,
		ui.column {
			w_full = true,
			p = "$space.4",
			ui.show(function(): boolean
				return reading:get() == ""
			end, ui.text { locale.t("ds.orch.scratchpad.empty"), text_size = "$font.size.sm", color = "$text.tertiary" }),
			Markdown {
				text = function(): string
					return reading:get()
				end,
				breaks = true,
			},
		},
	}
	return ui.column {
		class = "orch-scratchpad-notes",
		w_full = true,
		h = h,
		grow = grow,
		min_h = 0,
		rounded = "$radius.lg",
		bg = fill,
		ui.switch(function(): string
			return if previewing:get() then "preview" else "write"
		end, { write = editor, preview = preview }),
	}
end

local function preview_button(): Node
	return IconButton {
		id = "scratchpad-preview",
		icon = function(): any
			return if previewing:get() then "pen" else "eye"
		end,
		label = function(): string
			return locale.s(if previewing:get() then "ds.orch.scratchpad.write" else "ds.orch.scratchpad.preview")
		end,
		size = "sm",
		tone = "quiet",
		on_click = function()
			previewing:set(not previewing:peek())
		end,
	}
end

local function tabs(): Node
	return Segmented {
		id = "scratchpad-tabs",
		value = tab,
		label = locale.t("ds.orch.scratchpad.view"),
		options = {
			{ value = "notes", label = locale.t("ds.orch.scratchpad.notes") },
			{ value = "draw", label = locale.t("ds.orch.scratchpad.draw") },
		},
	}
end

-- Notes or Draw, as the tab says.
local function tabbed(props: Props, h: number): Node
	return ui.switch(function(): string
		return tab:get()
	end, {
		notes = Notes(props, h),
		draw = Draw.Board { shapes = shapes, h = h - 34 },
	})
end

M.Widget = ui.component(function(props: Props): Node
	follow(props)
	return ui.column {
		id = props.id or "scratchpad",
		class = "orch-scratchpad",
		w_full = true,
		gap = "$space.3",
		ui.row {
			class = "orch-scratchpad-bar",
			w_full = true,
			gap = "$space.1",
			items = "center",
			tabs(),
			ui.div { grow = 1 },
			ui.show(function(): boolean
				return tab:get() == "notes"
			end, preview_button()),
			History(props),
			IconButton {
				id = "scratchpad-expand",
				icon = "expand-arrows",
				label = locale.t("ds.orch.scratchpad.expand"),
				size = "sm",
				tone = "quiet",
				on_click = function()
					if props.on_expand then
						props.on_expand()
					end
				end,
			},
		},
		Notice(props),
		tabbed(props, WIDGET_H),
	}
end)

M.Page = ui.component(function(props: Props): Node
	follow(props)
	local width = ui.use_state(0)
	local function collapse()
		if props.on_collapse then
			props.on_collapse()
		end
	end
	return ui.column {
		id = props.id or "scratchpad-page",
		class = "orch-scratchpad orch-scratchpad-page",
		w_full = true,
		gap = "$space.4",
		on_resize = function(s: { w: number, h: number })
			if math.abs(s.w - width:peek()) >= 1 then
				width:set(s.w)
			end
		end,
		on_key = function(e: any): boolean
			if e.key == "Escape" and not e.editing then
				collapse()
				return true
			end
			return false
		end,
		ui.row {
			id = "scratchpad-head",
			w_full = true,
			gap = "$space.3",
			items = "center",
			IconButton {
				id = "scratchpad-back",
				icon = "alt-arrow-left",
				label = locale.t("ds.orch.scratchpad.collapse"),
				size = "md",
				tone = "quiet",
				on_click = collapse,
			},
			ui.text { locale.t("ds.orch.scratchpad.title"), text_size = "$font.size.lg", line_height = "$font.line.lg", weight = "$font.weight.medium", color = "$text.primary" },
			ui.row { y = Block.baseline("$font.size.lg", "$font.size.xs"), M.Status() },
			ui.div { grow = 1 },
			History(props),
			IconButton {
				id = "scratchpad-collapse",
				icon = "collapse-arrows",
				label = locale.t("ds.orch.scratchpad.collapse"),
				size = "md",
				tone = "quiet",
				on_click = collapse,
			},
		},
		Notice(props),
		ui.switch(function(): string
			return if width:get() >= M.SPLIT then "split" else "tabs"
		end, {
			-- (A real swap: side by side or one at a time.)
			split = ui.row {
					class = "orch-scratchpad-split",
					w_full = true,
					h = PAGE_H,
					gap = "$space.4",
					items = "stretch",
					ui.column {
						grow = 1,
						basis = 0,
						min_w = 0,
						Block.Block {
							id = "scratchpad-notes-block",
							title = locale.t("ds.orch.scratchpad.notes"),
							menu = preview_button(),
							gap = "$space.3",
							Notes(props, PAGE_H - 74),
						},
					},
					ui.column {
						grow = 1,
						basis = 0,
						min_w = 0,
						Block.Block {
							id = "scratchpad-draw-block",
							title = locale.t("ds.orch.scratchpad.draw"),
							gap = "$space.3",
							Draw.Board { shapes = shapes, h = PAGE_H - 74 - 34 },
						},
					},
				},
			tabs = ui.column {
					w_full = true,
					gap = "$space.3",
					ui.row {
						w_full = true,
						items = "center",
						tabs(),
						ui.div { grow = 1 },
						ui.show(function(): boolean
							return tab:get() == "notes"
						end, preview_button()),
					},
					tabbed(props, PAGE_H),
				},
		}),
	}
end)

return M
