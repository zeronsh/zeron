-- What the agent asks, in the composer's own pill: a question with options,
-- or an approval. Either takes the place of the cargo strip, over the
-- draft, which becomes the answer ("Type your own answer, or pick an option
-- above"); the pill's controls become the way through (Back, Submit; Deny,
-- Allow). The surface, its corners and the caret stay where they were, so
-- answering reads as writing, not as a dialog.
--
-- A question (Zeron's question panel, crates/ui/src/composer.rs
-- render_question_panel): its header small and tracked, its page among its
-- pages, the question itself, then its options, each with its number: 1 to
-- 9 pick one from the keys. Chosen, an option wears a ring, never a fill.
-- An approval is ours to shape: what it wants to do (a command, in its
-- face), a line on why, and Deny, Allow once, Always allow.
--
--   Question { question = q, chosen = chosen, on_pick = … }
--   Approval { approval = a }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Badge = require("@self/badge").Badge
local control = require("@self/control")
local SelectionMenu = require("@self/selection_menu")
local locale = require("@self/locale")

local M = {}

export type Option = { value: string, label: string, detail: string? }
export type Question = {
	-- Its kind, as the agent names it ("Question", "Choose a branch").
	header: string?,
	text: string,
	options: { Option },
	-- Several may be chosen.
	multi: boolean?,
	-- This page among the pages, from 1.
	page: number?,
	pages: number?,
}
export type QuestionProps = {
	question: Question,
	-- Which options are chosen, by value.
	chosen: State<{ [string]: boolean }>,
	on_pick: ((value: string) -> ())?,
	on_close: (() -> ())?,
	-- Escape would close it (nothing picked or typed yet): the close says so.
	closes_on_escape: (() -> boolean)?,
}

export type Approval = {
	-- What it asks to do ("Run this command?").
	title: string,
	-- The command, shown in its own face.
	command: string?,
	-- Where, and a line on why.
	cwd: string?,
	detail: string?,
}
export type ApprovalProps = { approval: Approval }

-- A panel's head: its kind, small and tracked (the words alone: no glyph
-- stands for a question or an approval, as Zeron's panel has none), with
-- its page among its pages and the × at the end.
-- (`kind` the agent's header, or a function of the locale's word.)
local function head(kind: string | () -> string, page: string?, on_close: (() -> ())?, close_hint: (() -> string)?): Node
	return ui.row {
		h = 28,
		items = "center",
		gap = "$space.2",
		ui.text {
			function(): string
				return string.upper(if type(kind) == "function" then kind() else kind)
			end,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			weight = "$font.weight.medium",
			letter_spacing = 0.6,
			color = "$text.tertiary",
		},
		if page then Badge { page, size = "sm", tabular = true } else nil,
		ui.div { grow = 1 },
		if on_close
			then IconButton {
				icon = "close",
				label = locale.t("ds.composer_ask.close"),
				-- (A tooltip is a string: read as the head is built.)
				tooltip = if close_hint then close_hint() else locale.s("ds.composer_ask.close"),
				size = "sm",
				tone = "quiet",
				-- Its ink on the right text edge: the box hangs past it by the
				-- glyph's inset.
				mr = ui.raw(-4, "the ×'s glyph sits on the text's edge, its box past it"),
				on_click = on_close,
			}
			else nil,
	}
end

-- An option's number, in a cap at its start: the key that picks it.
local function key(n: number): Node
	return ui.div {
		w = "$zeron.composer.option.key",
		h = "$zeron.composer.option.key",
		shrink = 0,
		items = "center",
		justify = "center",
		-- Concentric with its option, 8 in from the option's edge.
		rounded = "$zeron.composer.option.key_radius",
		bg = "$zeron.secondary.bg",
		ui.text {
			tostring(n),
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			tabular = true,
			color = "$text.secondary",
		},
	}
end

M.Question = ui.component(function(props: QuestionProps): Node
	local q = props.question
	local chosen = props.chosen
	local function picked(v: string): boolean
		return chosen:get()[v] == true
	end
	local rows: { Node } = {}
	for i, o in q.options do
		local function on(): boolean
			return picked(o.value)
		end
		table.insert(
			rows,
			ui.row {
				min_h = "$zeron.composer.option.h",
				-- Its wash and ring hang past the text's edge: the number cap
				-- sits on the edge the title and the answer share.
				mx = ui.raw(-8, "the row's wash hangs a step past the text's edge"),
				pl = "$space.2",
				pr = "$space.2",
				py = "$zeron.space.tight",
				gap = "$zeron.space.snug",
				items = "center",
				rounded = "$zeron.composer.option.radius",
				role = if q.multi then "checkbox" else "radio",
				checked = on,
				label = o.label,
				focusable = true,
				cursor = "pointer",
				bg = "#00000000",
				-- Chosen: the accent's ring over the row's edge; never a fill.
				border = 1,
				border_over = true,
				border_color = function()
					return if on() then "$zeron.composer.option.ring" else "#00000000"
				end,
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				focus = control.ring,
				on_click = function()
					if props.on_pick then
						props.on_pick(o.value)
					end
				end,
				on_key = function(e)
					if e.key == " " or e.key == "Enter" then
						if props.on_pick then
							props.on_pick(o.value)
						end
						return true
					end
					return false
				end,
				if i <= 9 then key(i) else ui.div { w = "$zeron.composer.option.key", shrink = 0 },
				ui.column {
					grow = 1,
					min_w = 0,
					gap = 0,
					ui.text {
						o.label,
						wrap = true,
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						weight = "$font.weight.medium",
						color = "$text.primary",
					},
					if o.detail
						then ui.text {
							o.detail,
							wrap = true,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.tertiary",
						}
						else nil,
				},
				ui.div {
					w = 14,
					shrink = 0,
					items = "center",
					color = "$accent",
					ui.show(on, Icon { name = "check", size = 14 }),
				},
			}
		)
	end
	local page = if q.pages and q.pages > 1 then `{q.page or 1}/{q.pages}` else nil
	-- One left edge for all of it, the draft's own (the pill's text inset):
	-- the kind, the question, its hint, the options' number caps and the
	-- answer typed under them. (Out of plugins' reach: the agent's question
	-- is the person's to answer.)
	return ui.column {
		hook = false,
		w_full = true,
		px = "$space.4",
		pt = "$space.2",
		gap = "$space.1",
		head(q.header or function(): string
			return locale.s("ds.composer_ask.question")
		end, page, props.on_close, if props.closes_on_escape
			then function()
				return locale.s(if (props.closes_on_escape :: () -> boolean)() then "ds.composer_ask.close_esc" else "ds.composer_ask.close")
			end
			else nil),
		ui.column {
			pb = "$space.2",
			gap = "$zeron.space.hair",
			ui.text {
				q.text,
				wrap = true,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			if q.multi
				then ui.text { locale.t("ds.composer_ask.multi"), text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" }
				else nil,
		},
		ui.column { w_full = true, gap = "$zeron.space.hair", role = if q.multi then "group" else "radiogroup", label = q.text, table.unpack(rows) },
	}
end)

M.Approval = ui.component(function(props: ApprovalProps): Node
	local a = props.approval
	-- (Out of plugins' reach: what the agent may do is the person's to say.)
	return ui.column {
		hook = false,
		w_full = true,
		px = "$space.4",
		pt = "$space.2",
		gap = "$space.1",
		head(function(): string
			return locale.s("ds.composer_ask.approval")
		end, nil, nil, nil),
		ui.column {
			pb = "$space.2",
			gap = "$space.2",
			ui.text {
				a.title,
				wrap = true,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			if a.command
				then ui.column {
					w_full = true,
					px = "$space.3",
					py = "$space.2",
					-- At the strip's inset, concentric with the pill as its
					-- chips are.
					rounded = "$zeron.composer.chip.radius",
					bg = "$zeron.code.wash",
					border = 1,
					border_color = "$zeron.prose.code.edge",
					SelectionMenu { w_full = true, ui.text {
						a.command,
						literal = true,
						wrap = true,
						font = "$font.mono",
						text_size = "$zeron.prose.code.size",
						line_height = "$zeron.prose.code.line",
						color = "$text.primary",
					} },
				}
				else nil,
			ui.row {
				gap = "$space.2",
				items = "center",
				if a.cwd
					then ui.text { a.cwd, literal = true, truncate = true, shrink = 1, min_w = 0, font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary" }
					else nil,
				if a.detail
					then ui.text { a.detail, wrap = true, grow = 1, min_w = 0, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary" }
					else nil,
			},
		},
	}
end)

return M
