--!strict
-- What an element gives plugins of a file or a reply's block: a token the
-- host makes (src/plugins/tokens.rs), never the path or the text. zgx
-- copies whatever an element provides into any plugin that asks, so a
-- plugin finds what's behind a token only through its own grants (the
-- SDK's `zeron.context("file")`, `zeron.need("code")`).
--
--   provide = { file = token.file(path) }
--   provide = { code = token.block({ lang = lang, source = text }) }
--
-- (Lent to plugins with the rest of ds: there, where the host's maker
-- isn't theirs to call, a plugin's own element gives its own value.)

local M = {}

local function made(name: string, value: any): any
	if value == nil then
		return nil
	end
	local ok, token = pcall(ui.call_sync, name, value)
	return if ok then token else value
end

-- A file's token, by its path.
function M.file(path: string?): string?
	return made("zeron.file_token", path)
end

-- A block's token, by what it holds (`{ lang, source }`, or a formula's TeX).
function M.block(value: any): string?
	return made("zeron.block_token", value)
end

return M
