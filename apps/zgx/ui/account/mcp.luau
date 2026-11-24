-- Managed MCP servers, as the host reads them from the engine (src/mcp.rs):
-- presets, configured servers, Vault vs local storage status, and interactive
-- OAuth flows.
--
--   mcp.now:get()              { loaded, storage, vaultAvailable, presets, servers }
--   mcp.auth_flows:get()       { [serverId] = { phase, flowId?, error? } }
--   mcp.refresh()              RefreshMcpServers
--   mcp.upsert(req)            UpsertMcpServer
--   mcp.set_enabled(id, on)    SetMcpServerEnabled
--   mcp.delete(id)             DeleteMcpServer
--   mcp.start_auth(req)        StartMcpAuth + browser open + PollMcpAuth loop
--   mcp.cancel_auth(id, flow)  CancelMcpAuth
--   mcp.disconnect_auth(id)    DisconnectMcpAuth

local M = {}

export type Transport = "http" | "sse" | "stdio"
export type AuthKind = "none" | "oauth" | "bearer" | "headers"
export type Storage = "vault" | "local"
export type ServerStatus = "connected" | "needsAuth" | "authorizing" | "reauthRequired" | "error" | "disabled"

export type Preset = {
	id: string,
	name: string,
	description: string,
	category: string,
	url: string,
	transport: Transport,
	authKind: AuthKind,
	docsUrl: string,
	icon: string,
	headerName: string?,
}

export type Server = {
	id: string,
	name: string,
	preset: string?,
	transport: Transport,
	url: string?,
	command: string?,
	args: { string },
	envKeys: { string },
	authKind: AuthKind,
	headerKeys: { string },
	enabled: boolean,
	storage: Storage,
	status: ServerStatus,
	-- The account's name, as the person gave it ("Work").
	accountLabel: string?,
	-- The name agents see it by: `linear`, or `linear_work` beside another account.
	agentName: string?,
	lastError: string?,
	scopes: { string },
	updatedAt: number,
}

export type Snapshot = {
	loaded: boolean,
	storage: Storage,
	vaultAvailable: boolean,
	presets: { Preset },
	servers: { Server },
}

export type AuthFlow = {
	phase: "authorizing" | "error" | "done",
	flowId: string?,
	error: string?,
}

export type UpsertRequest = {
	id: string?,
	name: string,
	preset: string?,
	transport: Transport,
	url: string?,
	command: string?,
	args: { string }?,
	env: { [string]: string }?,
	authKind: AuthKind,
	bearerToken: string?,
	headers: { [string]: string }?,
	enabled: boolean?,
	localOnly: boolean?,
	oauthClientId: string?,
	oauthClientSecret: string?,
	oauthScopes: { string }?,
	-- The account's name: nil keeps it, "" clears it.
	accountLabel: string?,
}

export type StartAuthRequest = {
	id: string,
	clientId: string?,
	clientSecret: string?,
	scopes: { string }?,
}

M.now = ui.shared("mcp.servers", {
	loaded = false,
	storage = "local",
	vaultAvailable = false,
	presets = {},
	servers = {},
} :: Snapshot)

M.auth_flows = ui.shared("mcp.auth", {} :: { [string]: AuthFlow })

function M.refresh(): any
	return ui.call("mcp.refresh", nil)
end

function M.upsert(req: UpsertRequest): any
	return ui.call("mcp.upsert", req)
end

function M.set_enabled(id: string, enabled: boolean): any
	return ui.call("mcp.set_enabled", { id = id, enabled = enabled })
end

function M.delete(id: string): any
	return ui.call("mcp.delete", { id = id })
end

function M.start_auth(req: StartAuthRequest | string): any
	local body: StartAuthRequest = if type(req) == "string" then { id = req } else req
	return ui.call("mcp.start_auth", body)
end

function M.cancel_auth(id: string, flow_id: string?): any
	return ui.call("mcp.cancel_auth", { id = id, flowId = flow_id })
end

function M.disconnect_auth(id: string): any
	return ui.call("mcp.disconnect_auth", { id = id })
end

return M
