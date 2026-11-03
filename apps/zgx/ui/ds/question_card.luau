--!strict
-- A question an agent asked without stopping for it (the engine's
-- `ask_question`), as a card the person answers when they like: over the
-- chat's composer (screens/chat/questions) and on the panel's home.
--
-- Open, it says what was asked (markdown), what the agent added for
-- context (quieter, folded past a few lines), its options as buttons, a
-- field for an answer of your own (Enter sends it) and × to dismiss it.
-- Settled (answered or dismissed), it folds to one line: the question,
-- then the answer or "Dismissed".
--
--   QuestionCard.Card { question = () -> Question, on_answer, on_dismiss,
--                       title? = () -> string?, failed? = () -> string?,
--                       on_title? }
--   QuestionCard.Settled { question = () -> Question }
--
-- The card is on the menus' glass, as the trays behind the composer are:
-- what waits on the person, over the page.

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Button = require("@self/button")
local TextField = require("@self/text_field")
local Markdown = require("@self/markdown")
local FadeText = require("@self/fade_text")
local material = require("@self/material")
local locale = require("@self/locale")

local M = {}

export type Question = {
	id: string,
	chat: string,
	question: string,
	options: { string },
	context: string?,
	status: string,
	answer: string?,
}

export type CardProps = {
	question: () -> Question,
	on_answer: (text: string) -> (),
	on_dismiss: () -> (),
	-- Whose question it is (the panel's home lists every chat's): its
	-- chat's title, a link to it with `on_title`.
	title: (() -> string?)?,
	on_title: (() -> ())?,
	-- Why the last answer didn't go.
	failed: (() -> string?)?,
	id: string?,
}

-- Context longer than this folds, its first lines showing.
local FOLD_AT = 280

-- The card's glass: the trays' (ds/composer_dock), whole corners.
local function glass(props: ContainerProps): Node
	props.w_full = true
	props.rounded = "$zeron.composer.tray.radius"
	props.border = 1
	props.border_color = "$zeron.composer.tray.edge"
	props.bg = function()
		return if material.frosted:get() then "$zeron.composer.tray.tint" else "$zeron.composer.tray.solid"
	end
	props.backdrop_blur = function()
		return if material.frosted:get() then "$zeron.composer.tray.blur" else 0
	end
	return ui.column(props)
end

-- Its kind, small and tracked, as the composer's question panel heads its
-- own (ds/composer_ask).
local function kind(): Node
	return ui.text {
		function(): string
			return string.upper(locale.s("ds.question_card.kind"))
		end,
		shrink = 0,
		text_size = "$font.size.xs",
		line_height = "$font.line.xs",
		weight = "$font.weight.medium",
		letter_spacing = 0.6,
		color = "$text.tertiary",
	}
end

M.Card = ui.component(function(props: CardProps): Node
	local q = props.question
	local draft = ui.use_state("")
	local unfolded = ui.use_state(false)
	local function send()
		local text = draft:peek()
		if string.find(text, "%S") then
			props.on_answer(text)
			draft:set("")
		end
	end
	local title = props.title
	local head = ui.row {
		h = 24,
		w_full = true,
		items = "center",
		gap = "$space.2",
		kind(),
		if title
			then ui.row {
				grow = 1,
				min_w = 0,
				items = "center",
				cursor = if props.on_title then "pointer" else nil,
				role = if props.on_title then "link" else nil,
				on_click = props.on_title,
				FadeText {
					function(): string
						return title() or ""
					end,
					grow = 1,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			}
			else ui.div { grow = 1 },
		IconButton {
			icon = "close",
			label = locale.t("ds.question_card.dismiss"),
			tooltip = locale.s("ds.question_card.dismiss"),
			size = "sm",
			tone = "quiet",
			on_click = props.on_dismiss,
		},
	}
	local context = ui.show(
		function(): boolean
			return (q().context or "") ~= ""
		end,
		ui.column {
			w_full = true,
			gap = "$zeron.space.hair",
			ui.text {
				function(): string
					local c = q().context or ""
					if #c > FOLD_AT and not unfolded:get() then
						return string.sub(c, 1, utf8.offset(c, FOLD_AT) or FOLD_AT) .. "…"
					end
					return c
				end,
				wrap = true,
				selectable = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.tertiary",
			},
			ui.show(
				function(): boolean
					return #(q().context or "") > FOLD_AT
				end,
				ui.row {
					ui.text {
						function(): string
							return locale.s(if unfolded:get() then "ds.question_card.less" else "ds.question_card.more")
						end,
						role = "button",
						cursor = "pointer",
						text_size = "$font.size.sm",
						color = "$text.secondary",
						hover = { color = "$text.primary" },
						on_click = function()
							unfolded:set(not unfolded:peek())
						end,
					},
				}
			),
		}
	)
	local options = ui.show(
		function(): boolean
			return #q().options > 0
		end,
		ui.row {
			class = "question-options",
			w_full = true,
			wrap = true,
			gap = "$space.2",
			role = "group",
			label = locale.t("ds.question_card.options"),
			ui.each(ui.computed(function(): { string }
				return q().options
			end), function(label: string): Node
				return Button {
					label,
					class = "question-option",
					tone = "secondary",
					size = "sm",
					on_click = function()
						props.on_answer(label)
					end,
				}
			end, {
				key = function(label: string): string
					return label
				end,
				wrap = true,
				gap = "$space.2",
			}),
		}
	)
	local failed = props.failed
	return glass {
		id = props.id,
		class = "question-card",
		role = "group",
		label = function(): string
			return q().question
		end,
		p = "$space.3",
		gap = "$space.2",
		head,
		Markdown {
			text = function(): string
				return q().question
			end,
			small = true,
		},
		context,
		options,
		TextField {
			value = draft,
			name = locale.t("ds.question_card.answer"),
			placeholder = locale.t("ds.question_card.answer_placeholder"),
			size = "sm",
			w_full = true,
			on_commit = function(_text: string)
				send()
			end,
			trailing = IconButton {
				icon = "arrow-up",
				label = locale.t("ds.question_card.send"),
				tooltip = locale.s("ds.question_card.send"),
				size = "sm",
				tone = "quiet",
				on_click = send,
			},
		},
		if failed
			then ui.show(
				function(): boolean
					return failed() ~= nil
				end,
				ui.text {
					function(): string
						return locale.s("ds.question_card.failed", { why = failed() or "" })
					end,
					wrap = true,
					text_size = "$font.size.sm",
					color = "$zeron.status.failed",
				}
			)
			else nil,
	}
end)

-- Settled: one line, the question then what became of it.
function M.Settled(props: { question: () -> Question, id: string? }): Node
	local q = props.question
	local function answered(): boolean
		return q().status == "answered"
	end
	return ui.row {
		id = props.id,
		class = "question-settled",
		w_full = true,
		h = 28,
		px = "$space.3",
		gap = "$space.2",
		items = "center",
		role = "group",
		label = function(): string
			local said = if answered() then q().answer or "" else locale.s("ds.question_card.dismissed")
			return `{q().question} → {said}`
		end,
		ui.div {
			shrink = 0,
			color = "$text.tertiary",
			Icon {
				name = function(): string
					return if answered() then "check" else "close"
				end,
				size = 12,
			},
		},
		FadeText {
			function(): string
				return (string.gsub(q().question, "%s+", " "))
			end,
			grow = 1,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		},
		ui.text {
			function(): string
				return if answered() then q().answer or "" else locale.s("ds.question_card.dismissed")
			end,
			shrink = 0,
			max_w = 200,
			truncate = true,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.secondary",
		},
	}
end

return M
