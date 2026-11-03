-- An agent's sign-in that asks for typed input (a key pasted, a choice
-- made), in a terminal: the panel's terminal (screens/panel/terminal_api,
-- src/terminal.rs) on the agent's device, its home folder, its sign-in
-- command typed in for you. Settings → Providers opens it from the card's
-- failed sign-in ("Open a terminal"); Done closes the terminal and the card
-- looks again.
--
--   sign_in_terminal.show { id, name, command, device? }
--   sign_in_terminal.Dialog(on_done)   once, beside the page

local ds = require("@ds")
local api = require("@app/screens/panel/terminal_api")

local M = {}

export type Who = { id: string, name: string, command: string, device: string?, device_name: string }

local open = ui.state("settings_sign_in_terminal_open", false)
local who = ui.state("settings_sign_in_terminal_who", nil :: Who?)
-- Each opening its own terminal (zgx keeps a session by its key).
local opened = ui.state("settings_sign_in_terminal_n", 0)

function M.show(w: Who)
	who:set(w)
	opened:set(opened:peek() + 1)
	open:set(true)
end

-- The terminal, opened as the dialog opens: its bridge started, then, once
-- the engine's terminal is claimed, the sign-in typed into it.
local Terminal = ui.component(function(props: { who: Who, key: string, on_terminal: (id: string?) -> () }): Node
	local w = props.who
	-- The bridge's command once there is one; why it couldn't open.
	local bridge = ui.use_state("")
	local failed = ui.use_state("")
	ui.after(0, function()
		api.start({ chat = `acp-sign-in:{w.id}`, device = w.device }):after(function(r: any)
			bridge:set(r.command)
			api.claimed(r.claim):after(function(terminal: string)
				props.on_terminal(terminal)
				api.write(terminal, w.device, w.command .. "\n"):after(function() end, function() end)
			end, function(err: any)
				failed:set(if type(err) == "table" then tostring(err.message) else tostring(err))
			end)
		end, function(err: any)
			local m = if type(err) == "table" then tostring(err.message) else tostring(err)
			failed:set(if m == "offline" then ui.t("panel.terminal.offline"):get() else m)
		end)
	end)
	local function kind(): string
		if failed:get() ~= "" then
			return "failed"
		end
		return if bridge:get() ~= "" then "ready" else "opening"
	end
	return ui.column {
		w_full = true,
		h = 320,
		rounded = "$radius.md",
		overflow = "hidden",
		bg = "$zeron.secondary.bg",
		ui.presence(kind, function(k: string): Node
			if k == "failed" then
				return ds.EmptyState {
					icon = "terminal",
					size = "sm",
					fill = true,
					title = ui.t("panel.terminal.failed") :: any,
					body = failed:peek(),
				}
			elseif k == "opening" then
				return ui.column { w_full = true, grow = 1 }
			end
			local term = api.term(props.key, { command = bridge:peek(), cwd = "~" })
			ui.after(0, function()
				if term and term.focus then
					term:focus()
				end
			end)
			-- (Its rows placed from its own box: `relative`, padded outside it.)
			return ui.column {
				w_full = true,
				grow = 1,
				min_h = 0,
				p = "$space.2",
				api.view {
					term = term,
					label = ui.t("settings.providers.add.terminal.label", { name = w.name }),
					relative = true,
					w_full = true,
					grow = 1,
					min_h = 0,
				},
			}
		end, { w_full = true, grow = 1, min_h = 0 }),
	}
end)

-- The dialog, beside the page: `on_done(id)` once it closes (the agent's
-- id), for the card to look again.
function M.Dialog(on_done: (id: string) -> ()): Node
	-- The terminal it opened, ended as it closes.
	local terminal: string? = nil
	local session: string? = nil
	local was_open = open:peek()
	ui.effect(function()
		local now = open:get()
		if was_open and not now then
			local w = who:peek()
			if session then
				local ok, t = pcall(api.term, session)
				if ok and t then
					pcall(function()
						t:close()
					end)
				end
			end
			if terminal and w then
				api.close(terminal, w.device):after(function() end, function() end)
			end
			terminal, session = nil, nil
			if w then
				on_done(w.id)
			end
		end
		was_open = now
	end)
	return ds.Dialog {
		open = open,
		size = "lg",
		title = function(): string
			local w = who:get()
			return if w then ui.t("settings.providers.add.terminal.title", { name = w.name }):get() else ""
		end,
		body = function(): string
			local w = who:get()
			return if w then ui.t("settings.providers.add.terminal.body", { name = w.name, device = w.device_name }):get() else ""
		end,
		cancel = false,
		confirm = { label = ui.t("settings.providers.done") },
		build = function(): Node
			local w = who:peek()
			if not w then
				return ui.div {}
			end
			local key = `acp-sign-in:{w.id}:{opened:peek()}`
			session = key
			return Terminal {
				who = w,
				key = key,
				on_terminal = function(id: string?)
					terminal = id
				end,
			}
		end,
	}
end

return M
