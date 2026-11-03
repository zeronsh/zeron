-- A terminal tab's host (src/terminal.rs) and zgx's terminal, behind one
-- module so tests can stand in for both (ui/panel_fixture.luau).
--
--   api.start(target)              a new terminal's bridge, which opens it at
--                                  its own size: { command, claim }
--   api.claimed(claim)             that terminal's id, once it's open
--   api.command(id, device)        the bridge's command line, for ui.term
--   api.status(id, device)         { busy, exited }
--   api.close(id, device)          the engine's terminal ended
--   api.write(id, device, text)    typed into it, as from the keyboard
--   api.term(key, options?)        zgx's session for `key` (ui.term)
--   api.view(props)                zgx's terminal view (ui.terminal)

local M = {}

function M.start(target: any): any
	return ui.call("terminal.start", { target = target })
end

function M.claimed(claim: string): any
	return ui.call("terminal.claimed", { claim = claim })
end

function M.command(id: string, device: string?): any
	return ui.call("terminal.command", { id = id, device = device })
end

function M.status(id: string, device: string?): any
	return ui.call("terminal.status", { id = id, device = device })
end

function M.close(id: string, device: string?): any
	return ui.call("terminal.close", { id = id, device = device })
end

function M.write(id: string, device: string?, text: string): any
	return ui.call("terminal.write", { id = id, device = device, text = text })
end

function M.term(key: string, options: any?): any
	if options then
		return (ui :: any).term(key, options)
	end
	return (ui :: any).term(key)
end

function M.view(props: any): Node
	return (ui :: any).terminal(props)
end

return M
