--!strict
-- New task: one box ("What should it do?") and whose it is, the
-- orchestrators as chips (none chosen: whoever takes it, as a dump is
-- routed). It goes to that orchestrator as a dump; its card appears in
-- Planned or Running when the engine has it.
--
--   NewTask.Dialog { open = state, orchestrators = fn, chosen = fn -> id?,
--     on_send = fn(text, orchestrator_id?) -> promise? }

local Dialog = require("@ds/dialog")
local TextArea = require("@ds/text_area")
local hue = require("@self/orch_hue")
local locale = require("@ds/locale")
local control = require("@ds/control")

local M = {}

export type Props = {
	open: State<boolean>,
	orchestrators: () -> { any },
	-- The orchestrator chosen as it opens (the board's filter).
	chosen: (() -> string?)?,
	on_send: (text: string, orchestrator_id: string?) -> any,
}

M.Dialog = ui.component(function(props: Props): Node
	local text = ui.use_state("")
	local whose = ui.use_state(nil :: string?)
	-- (As it opens: empty, on the board's own orchestrator.)
	ui.effect(function()
		if props.open:get() then
			text:set("")
			whose:set(if props.chosen then props.chosen() else nil)
		end
	end)
	local function send()
		local words = string.gsub(text:peek(), "^%s+", "")
		words = string.gsub(words, "%s+$", "")
		if words == "" then
			return
		end
		props.on_send(words, whose:peek())
		props.open:set(false)
	end
	local function chip(o: any): Node
		local function on(): boolean
			return whose:get() == o.id
		end
		return ui.button {
			class = "orch-new-chip",
			label = o.name,
			selected = on,
			h = 26,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			shrink = 0,
			rounded = "$radius.full",
			bg = function(): string
				return if on() then "$zeron.secondary.hover" else "$zeron.secondary.bg"
			end,
			hover = { bg = "$zeron.secondary.hover" },
			active = { bg = "$zeron.secondary.active" },
			outline = function(): number
				return if on() then 1 else 0
			end,
			outline_color = "$accent",
			outline_offset = -1,
			focus = control.ring,
			on_click = function()
				whose:set(if on() then nil else o.id)
			end,
			hue.Dot { hue = o.hue, size = 7 },
			ui.text { o.name, max_w = 200, truncate = true, text_size = "$font.size.sm", color = "$text.primary" },
		}
	end
	return Dialog.Dialog {
		open = props.open,
		title = locale.t("orch.board.new_task"),
		size = "md",
		confirm = {
			label = locale.t("orch.board.new_task_send"),
			on_click = send,
			keep_open = true,
			disabled = function(): boolean
				return string.match(text:get(), "%S") == nil
			end,
		},
		build = function(): Node
			local list: { any } = {}
			for _, o in props.orchestrators() do
				if not o.archived and o.kind ~= "brain_dump" then
					table.insert(list, o)
				end
			end
			local chips: { [any]: any } = {
				id = "orch-new-whose",
				w_full = true,
				gap = "$space.1",
				items = "center",
				flex_wrap = true,
			}
			for _, o in list do
				table.insert(chips, chip(o))
			end
			return ui.column {
				id = "orch-new-task",
				w_full = true,
				gap = "$space.3",
				TextArea {
					value = text,
					name = locale.t("orch.board.new_task_what"),
					placeholder = locale.t("orch.board.new_task_what"),
					rows = 3,
					max_rows = 10,
					autofocus = true,
					w_full = true,
					on_submit = function()
						send()
					end,
				},
				if #list > 0 then ui.row(chips) else nil,
			}
		end,
	}
end)

return M
