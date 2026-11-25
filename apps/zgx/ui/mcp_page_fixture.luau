-- The shell (ui/shell_fixture.luau) with Settings → MCP's snapshot: what
-- tests/settings_mcp.test.luau measures. It starts with no server added;
-- `mcp_page.add` adds one, `mcp_page.ask` has an agent ask for one.

local mcp = require("@account/mcp")

local LINEAR = { id = "linear", name = "Linear", description = "Issues and projects.", category = "", url = "https://mcp.linear.app/mcp", transport = "http", authKind = "oauth", docsUrl = "", icon = "linear" }
local NOTION = { id = "notion", name = "Notion", description = "Pages and databases.", category = "", url = "https://mcp.notion.com/mcp", transport = "http", authKind = "oauth", docsUrl = "", icon = "notion" }

local function show(servers: { any }, requests: { any })
	mcp.now:set({ loaded = true, storage = "vault", vaultAvailable = true, presets = { LINEAR, NOTION }, servers = servers, requests = requests } :: any)
end

local pending: any = { after = function() end }
for _, name in { "refresh", "upsert", "set_enabled", "delete", "start_auth", "cancel_auth", "disconnect_auth" } do
	(mcp :: any)[name] = function()
		return pending
	end
end

ui.command {
	id = "mcp_page.add",
	title = "mcp_page.add",
	run = function()
		show({
			{ id = "linear", name = "Linear", preset = "linear", transport = "http", url = LINEAR.url, args = {}, envKeys = {}, authKind = "oauth", headerKeys = {}, enabled = true, storage = "vault", status = "connected", scopes = {}, agentName = "linear", updatedAt = 1 },
		}, {})
	end,
}
ui.command {
	id = "mcp_page.ask",
	title = "mcp_page.ask",
	run = function()
		show({}, {
			{ id = "r1", title = "Add Notion", status = "pending", createdAt = 1, action = "add", preset = "notion" },
		})
	end,
}
ui.command {
	id = "mcp_page.none",
	title = "mcp_page.none",
	run = function()
		show({}, {})
	end,
}
show({}, {})

return require("@app/shell_fixture")
