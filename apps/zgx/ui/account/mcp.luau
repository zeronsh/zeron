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
--
-- And the agents outside Zeron let in to this device (engine outside_agents.rs):
--
--   mcp.outside:get()          { loaded, grants, asks }
--   mcp.resolve_outside(id, allow)  ResolveOutsideAgent
--   mcp.revoke_outside(id)     RevokeOutsideAgent

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

-- A change an agent asked for (engine mcp_servers/requests.rs): waiting for
-- the person, who alone answers it here (`resolve_request`).
export type Request = {
	id: string,
	action: "add" | "remove" | "setEnabled",
	preset: string?,
	name: string?,
	url: string?,
	authKind: AuthKind?,
	command: string?,
	args: { string }?,
	accountLabel: string?,
	server: string?,
	enabled: boolean?,
	reason: string?,
	chatId: string?,
	title: string,
	status: "pending" | "approved" | "declined" | "failed",
	serverId: string?,
	error: string?,
	createdAt: number,
}

export type Snapshot = {
	loaded: boolean,
	storage: Storage,
	vaultAvailable: boolean,
	presets: { Preset },
	servers: { Server },
	requests: { Request }?,
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

-- The person's answer to an agent's request: allow (done as the page does
-- it) or decline.
function M.resolve_request(id: string, approve: boolean): any
	return ui.call("mcp.resolve_request", { id = id, approve = approve })
end

function M.disconnect_auth(id: string): any
	return ui.call("mcp.disconnect_auth", { id = id })
end

-- An agent outside Zeron this device let in. `client` is the name it
-- registered with (its own say-so); `redirectHost`, where its sign-in returned.
export type OutsideGrant = {
	id: string,
	client: string,
	redirectHost: string,
	-- Milliseconds.
	createdAt: number,
}

-- One asking now, from the browser where the person picked this device.
export type OutsideAsk = {
	id: string,
	client: string,
	redirectHost: string,
	at: number,
}

export type Outside = {
	loaded: boolean,
	grants: { OutsideGrant },
	asks: { OutsideAsk },
}

M.outside = ui.shared("mcp.outside", { loaded = false, grants = {}, asks = {} } :: Outside)

function M.resolve_outside(id: string, allow: boolean): any
	return ui.call("mcp.resolve_outside", { id = id, allow = allow })
end

function M.revoke_outside(id: string): any
	return ui.call("mcp.revoke_outside", { id = id })
end

return M
