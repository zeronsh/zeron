--!strict
-- The first of Orchestrators' app-side modules, before its screens move in:
-- it proves the path they take. Required from index.luau as the app's own
-- code, it reaches the app (`@app/…`, `@account/…`, `@ds/…`) and the
-- plugin's other files by relative paths.

local plugins = require("@app/account/plugins")
local words = require("./words")

local M = {}

-- What a gated hook shows while the plugin is on.
function M.label(): string
	return words.proof
end

-- Whether the app's own record says it's on (read through `@account`).
function M.on(): boolean
	return plugins.on("orchestrators")
end

return M
