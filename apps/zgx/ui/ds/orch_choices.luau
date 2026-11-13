-- A decision, compact: its options as chips, the one recommended ringed in
-- the accent with a small star, and a way to see all of it (the overlay).
-- A chip answers at once (ds/orch_decide's pick): the chips give way to
-- "Answered · <its label>" and Undo while the pick can be taken back.
-- With no options, a line to write the answer in. Needs you's rows and a
-- board card waiting on a decision both show it.
--
--   Choices {
--     ask = function() return ask end,
--     on_answer = function(ask, choice) end,   -- sent, past its Undo
--     on_expand = function(ask) end,           -- the whole of it
--     line = true,                             -- no options: a line to write in
--     size = "sm",                             -- "xs" on a card
--   }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local TextField = require("@self/text_field")
local locale = require("@self/locale")
local control = require("@self/control")
local decide = require("@self/orch_decide")

export type Props = {
	ask: () -> any,
	on_answer: (ask: any, choice: decide.Choice) -> (),
	on_expand: ((ask: any) -> ())?,
	line: boolean?,
	size: ("xs" | "sm")?,
	class: string?,
}

-- A chip's height and its words' size, by size.
local H = { xs = 22, sm = 24 }
local TEXT = { xs = "$font.size.xs", sm = "$font.size.sm" }

local function chip(props: Props, o: decide.Option): Node
	local size = props.size or "sm"
	return ui.button {
		class = if o.recommended then "orch-choice orch-choice-recommended" else "orch-choice",
		label = if o.recommended then locale.s("ds.orch.decide.recommended_named", { label = o.label }) else o.label,
		tooltip = if o.detail then o.detail else nil,
		tooltip_delay = 400,
		h = H[size],
		px = if size == "xs" then "$space.2" else ui.raw(10, "a chip's inset"),
		gap = "$zeron.space.tight",
		items = "center",
		shrink = 0,
		max_w = 320,
		rounded = "$radius.full",
		bg = "$zeron.secondary.bg",
		hover = { bg = "$zeron.secondary.hover" },
		active = { bg = "$zeron.secondary.active" },
		outline = if o.recommended then 1 else nil,
		outline_color = if o.recommended then "$accent" else nil,
		outline_offset = if o.recommended then -1 else nil,
		focus = control.ring,
		cursor = "pointer",
		on_click = function()
			local a = props.ask()
			if a then
				decide.pick(a, { option = o.index }, props.on_answer)
			end
		end,
		if o.recommended then ui.div { color = "$accent", shrink = 0, Icon { name = "star-bold", size = 10 } } else nil,
		ui.text {
			o.label,
			literal = true,
			shrink = 0,
			text_size = TEXT[size],
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
	}
end

return ui.component(function(props: Props): Node
	local draft = ui.use_state("")
	local size = props.size or "sm"
	local function id(): string?
		local a = props.ask()
		return a and a.id
	end
	local pending = ui.computed(function(): decide.Pending?
		return decide.pending_of(id())
	end)
	local opts = ui.computed(function(): { decide.Option }
		return decide.options(props.ask())
	end)
	local function expand(): Node?
		if not props.on_expand then
			return nil
		end
		return IconButton {
			class = "orch-choice-expand",
			icon = "expand-arrows",
			label = locale.t("ds.orch.decide.expand"),
			size = "sm",
			tone = "quiet",
			on_click = function()
				local a = props.ask()
				if a and props.on_expand then
					props.on_expand(a)
				end
			end,
		}
	end
	return ui.switch(function(): string
		if pending:get() then
			return "answered"
		end
		return if #opts:get() > 0 then "chips" elseif props.line then "line" else "none"
	end, {
		-- Picked: what was said, and Undo while it can be.
		answered = ui.row {
			class = "orch-answered",
			w_full = true,
			h = H[size],
			gap = "$space.2",
			items = "center",
			ui.div { color = "$zeron.status.done", shrink = 0, Icon { name = "check", size = 12 } },
			ui.text { locale.t("ds.orch.decide.answered"), shrink = 0, text_size = TEXT[size], weight = "$font.weight.medium", color = "$text.secondary" },
			ui.text { "·", shrink = 0, text_size = TEXT[size], color = "$text.tertiary" },
			ui.text {
				function(): string
					local p = pending:get()
					return if p then p.label else ""
				end,
				literal = true,
				truncate = true,
				min_w = 0,
				grow = 1,
				text_size = TEXT[size],
				color = "$text.secondary",
			},
			ui.show(function(): boolean
				local p = pending:get()
				return p ~= nil and not p.sent
			end, ui.button {
				class = "orch-undo",
				label = locale.t("ds.orch.decide.undo"),
				h = H[size],
				px = "$space.2",
				shrink = 0,
				rounded = "$radius.md",
				bg = "#00000000",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				focus = control.ring,
				on_click = function()
					local i = id()
					if i then
						decide.undo(i)
					end
				end,
				ui.text { locale.t("ds.orch.decide.undo"), text_size = TEXT[size], weight = "$font.weight.medium", color = "$accent" },
			}),
		},
		chips = ui.row {
			class = props.class or "orch-choices",
			w_full = true,
			gap = "$space.2",
			items = "start",
			ui.each(opts, function(o: decide.Option): Node
				return chip(props, o)
			end, {
				key = function(o: decide.Option): string
					return `{o.index}:{o.label}`
				end,
				direction = "row",
				flex_wrap = true,
				gap = ui.raw(6, "between chips"),
				items = "center",
				grow = 1,
				min_w = 0,
			}),
			expand(),
		},
		line = ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div {
				grow = 1,
				min_w = 0,
				TextField {
					value = draft,
					size = "sm",
					w_full = true,
					name = locale.t("ds.orch.ask.answer"),
					placeholder = locale.t("ds.orch.ask.answer"),
					on_commit = function(text: string)
						local a = props.ask()
						if a and string.match(text, "%S") then
							decide.pick(a, { text = text }, props.on_answer)
							draft:set("")
						end
					end,
				},
			},
			expand(),
		},
		none = ui.row { w_full = true, ui.div { grow = 1 }, expand() },
	})
end)
