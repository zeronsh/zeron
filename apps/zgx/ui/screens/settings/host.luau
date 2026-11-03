-- Settings' calls to the host (src/settings.rs, src/auth.rs, src/agents.rs),
-- through one place, so a fixture can answer them as the host would
-- (ui/settings_stub.luau). `call` returns the host's promise: `:after(ok,
-- err)`, `err.message` an outcome word.

local M = {}

function M.call(name: string, arg: any?): any
	return ui.call(name, arg)
end

function M.invoke(name: string, arg: any?)
	ui.invoke(name, arg)
end

-- Files the person picks (`ui.open_files`): a promise of a list (empty if
-- they cancel).
function M.open_files(opts: FilePickerOptions): Promise<{ File }>
	return ui.open_files(opts)
end

return M
