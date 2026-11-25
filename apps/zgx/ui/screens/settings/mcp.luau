-- Settings → MCP: the MCP servers every agent gets in every chat, on any
-- provider (the engine's gateway hands them to each harness). Signed in,
-- their sign-ins are kept by Zeron Vault and follow you to your devices and
-- Cloud; signed out, they stay on this device. The page says which in its
-- line under the title; a server kept on this device alone says so on its
-- row.
--
-- Two lists, as Providers has: your servers, a row each as the Plugins
-- page's (its mark, what it's doing, its settings and its switch), then the
-- servers to add, a click each, and a custom one last. Everything that asks
-- something happens in a dialog: signing in (waiting on the browser, the
-- app a server like Slack wants first, a key), adding a custom server, and
-- a server's own settings.

local ds = require("@ds")
local ds_select = require("@ds/select")
local material = require("@ds/material")
local Menu = require("@ds/menu")
local progress = require("@ds/progress")
local toast = require("@ds/toast")
local mcp = require("@account/mcp")
local host = require("@app/screens/settings/host")
local search = require("@app/screens/settings/search")
local sessions = require("@account/sessions")
local visits = require("@app/screens/settings/visits")

local M = {}

M.ID = "mcp"
M.TITLE = "MCP"

search.index(M.ID, ui.t("settings.mcp.title"), ui.t("settings.mcp.terms"), {
	{ key = "requests", title = ui.t("settings.mcp.requests.title"), terms = ui.t("settings.mcp.requests.terms") },
	{ key = "servers", title = ui.t("settings.mcp.servers.title"), terms = ui.t("settings.mcp.servers.terms") },
	{ key = "add", title = ui.t("settings.mcp.add.title"), terms = ui.t("settings.mcp.add.terms") },
})

-- The engine's word for a server whose OAuth can't register a client
-- itself (zeron_proto::MCP_CLIENT_REQUIRED): an app's own is asked for.
local CLIENT_REQUIRED = "mcp_client_required"
-- (What engines before it said instead.)
local CLIENT_REQUIRED_OLD = "registration_endpoint"

-- The servers with a mark of their own (assets/icons/mcp, Simple Icons'
-- in each brand's colour); any other gets its initial on a hue.
local LOGOS: { [string]: boolean } = {
	linear = true,
	notion = true,
	sentry = true,
	atlassian = true,
	stripe = true,
	supabase = true,
	vercel = true,
	cloudflare = true,
	posthog = true,
}
local HUES = { "violet", "blue", "teal", "amber", "rose", "indigo", "emerald", "orange" }

local function trim(s: string): string
	return (string.gsub(s, "^%s*(.-)%s*$", "%1"))
end

local function message_of(err: any): string?
	if type(err) == "string" and err ~= "" then
		return err
	end
	if type(err) == "table" and type(err.message) == "string" and err.message ~= "" then
		return err.message
	end
	return nil
end

local function wants_client(err: string?): boolean
	return err ~= nil
		and (string.find(err, CLIENT_REQUIRED, 1, true) ~= nil or string.find(err, CLIENT_REQUIRED_OLD, 1, true) ~= nil)
end

-- What went wrong, in words for the person (the engine's own, when it's
-- nothing known, after them).
local function friendly(err: string?): string
	if not err or err == "" then
		return ui.t("settings.mcp.error.generic"):get()
	end
	local lower = string.lower(err)
	if wants_client(err) then
		return ui.t("settings.mcp.error.client"):get()
	elseif string.find(lower, "timed out", 1, true) then
		return ui.t("settings.mcp.error.timed_out"):get()
	elseif string.find(lower, "could not open browser", 1, true) then
		return ui.t("settings.mcp.error.browser"):get()
	elseif string.find(lower, "in use", 1, true) and string.find(lower, "port", 1, true) then
		return ui.t("settings.mcp.error.port"):get()
	elseif lower == "offline" or lower == "closed" then
		return ui.t("settings.mcp.error.offline"):get()
	end
	local first = string.match(err, "^[^\n]*") or err
	if #first > 160 then
		first = string.sub(first, 1, 157) .. "…"
	end
	return ui.t("settings.mcp.error.detail", { detail = first }):get()
end

local function host_of(url: string?): string
	if not url then
		return ""
	end
	return string.match(url, "^%a[%w+.-]*://([^/?#]+)") or url
end

local function format_args(args: { string }?): string
	if not args or #args == 0 then
		return ""
	end
	local parts: { string } = {}
	for _, a in args do
		table.insert(parts, if string.find(a, "%s") then `"{a}"` else a)
	end
	return table.concat(parts, " ")
end

-- Splits a shell-like argument string, keeping what's quoted together.
local function split_args(raw: string): { string }
	local out: { string } = {}
	local i, len = 1, #raw
	while i <= len do
		while i <= len and string.match(string.sub(raw, i, i), "%s") do
			i += 1
		end
		if i > len then
			break
		end
		local buf: { string } = {}
		local quote: string? = nil
		while i <= len do
			local ch = string.sub(raw, i, i)
			if quote then
				if ch == quote then
					quote = nil
				elseif ch == "\\" and quote == '"' and i < len then
					i += 1
					table.insert(buf, string.sub(raw, i, i))
				else
					table.insert(buf, ch)
				end
			elseif ch == '"' or ch == "'" then
				quote = ch
			elseif string.match(ch, "%s") then
				break
			else
				table.insert(buf, ch)
			end
			i += 1
		end
		local token = table.concat(buf)
		if token ~= "" then
			table.insert(out, token)
		end
	end
	return out
end

local function split_scopes(raw: string): { string }
	local out: { string } = {}
	for part in string.gmatch(raw, "[^%s,]+") do
		table.insert(out, part)
	end
	return out
end

-- `Name: value` (headers) or `NAME=value` (environment) lines; the first
-- that isn't one comes back second.
local function parse_lines(raw: string, sep: string): ({ [string]: string }, string?)
	local out: { [string]: string } = {}
	for line in string.gmatch(raw .. "\n", "([^\n]*)\n") do
		local cleaned = trim(line)
		if cleaned ~= "" then
			local at = string.find(cleaned, sep, 1, true)
			local key = if at then trim(string.sub(cleaned, 1, at - 1)) else ""
			if key == "" then
				return out, cleaned
			end
			out[key] = if sep == "=" then string.sub(cleaned, (at :: number) + 1) else trim(string.sub(cleaned, (at :: number) + 1))
		end
	end
	return out, nil
end

local function valid_http_url(raw: string): boolean
	local lower = string.lower(raw)
	local rest = if string.sub(lower, 1, 8) == "https://"
		then string.sub(raw, 9)
		elseif string.sub(lower, 1, 7) == "http://" then string.sub(raw, 8)
		else nil
	return rest ~= nil and rest ~= "" and string.find(rest, "%s") == nil
end

local function snapshot(): mcp.Snapshot
	return mcp.now:get()
end

local function preset_by_id(id: string?): mcp.Preset?
	if not id then
		return nil
	end
	for _, p in snapshot().presets or {} do
		if p.id == id then
			return p
		end
	end
	return nil
end

local function server_by_id(id: string?): mcp.Server?
	if not id then
		return nil
	end
	for _, s in snapshot().servers or {} do
		if s.id == id then
			return s
		end
	end
	return nil
end

local function server_for_preset(p: mcp.Preset): mcp.Server?
	for _, s in snapshot().servers or {} do
		if s.preset == p.id or s.id == p.id then
			return s
		end
	end
	return nil
end

-- ── Accounts ───────────────────────────────────────────────────────────
-- A service can be connected more than once (work and personal): each
-- account its own server, named by the person ("Work"), which agents see as
-- one of their own (`linear_work`) beside the others. A service is a
-- preset's, else a name's, as the engine groups them
-- (crates/engine mcp_servers `assign_agent_names`).

-- A server's name with its account's, where it has one ("Linear · Work").
local function full_name(s: mcp.Server): string
	local label = s.accountLabel
	if label and label ~= "" then
		return ui.t("settings.mcp.account.full_name", { name = s.name, account = label }):get()
	end
	return s.name
end

local function service_of(s: mcp.Server): string
	if s.preset and s.preset ~= "" then
		return "preset:" .. s.preset
	end
	return "name:" .. string.lower(trim(s.name))
end

-- The service's accounts, this one among them, in the list's order.
local function accounts_of(s: mcp.Server): { mcp.Server }
	local key = service_of(s)
	local out: { mcp.Server } = {}
	for _, other in snapshot().servers or {} do
		if service_of(other) == key then
			table.insert(out, other)
		end
	end
	return out
end

-- Whether another account of it can be added: one with a sign-in of its own.
local function takes_accounts(s: mcp.Server): boolean
	return s.transport ~= "stdio" and (s.authKind == "oauth" or s.authKind == "bearer")
end

-- What agents will see an account by, as the engine names it: the
-- service's slug and the account's (`linear_work`). For a preview only:
-- the engine's own (`agentName`) is what a saved server shows.
local AGENT_NAME_MAX, SERVICE_MAX = 32, 20
local function agent_slug(input: string, max: number): string
	local out = string.gsub(string.lower(input), "[^%a%d]+", "_")
	out = string.gsub(out, "^_+", "")
	out = string.sub(out, 1, max)
	out = string.gsub(out, "_+$", "")
	return out
end
local function agent_preview(s: mcp.Server, label: string): string
	local service = agent_slug(if s.preset and s.preset ~= "" then s.preset else s.name, SERVICE_MAX)
	if service == "" then
		service = "mcp"
	elseif service == "zeron" then
		service = "zeron_mcp"
	end
	local account = agent_slug(label, AGENT_NAME_MAX - #service - 1)
	return if account == "" then service else `{service}_{account}`
end

-- A server's mark: its brand's on the marks' tile, else its initial.
local function Logo(icon: string?, name: string, size: number): Node
	if icon and LOGOS[icon] then
		return ui.column {
			w = size,
			h = size,
			shrink = 0,
			rounded = ui.raw(size * 0.25, "a mark tile's radius scales with it"),
			items = "center",
			justify = "center",
			bg = "$zeron.mark.bg",
			border = 1,
			border_color = "$zeron.mark.border",
			aria_hidden = true,
			ui.svg { src = `assets/icons/mcp/{icon}.svg`, w = size * 0.56, h = size * 0.56, shrink = 0 },
		}
	end
	local sum = 0
	for i = 1, #name do
		sum += string.byte(name, i)
	end
	return ds.Tile { label = name, hue = HUES[sum % #HUES + 1], size = size }
end

local function server_logo(s: mcp.Server, size: number): Node
	local p = preset_by_id(s.preset)
	return Logo(if p then p.icon else nil, s.name, size)
end

-- ── Signing in ─────────────────────────────────────────────────────────
-- One dialog for every sign-in, by step: waiting on the browser; the app
-- the server wants first (its client ID and secret, and the address to
-- allow); a key for a server that takes one; or what went wrong.

local connect_open = ui.state("settings_mcp_connect_open", false)
local connect_server = ui.state("settings_mcp_connect_server", nil :: string?)
local connect_preset = ui.state("settings_mcp_connect_preset", nil :: string?)
local connect_step = ui.state("settings_mcp_connect_step", "waiting")
local connect_error = ui.state("settings_mcp_connect_error", "")
local connect_busy = ui.state("settings_mcp_connect_busy", false)
local client_id = ui.state("settings_mcp_client_id", "")
local client_secret = ui.state("settings_mcp_client_secret", "")
local key_value = ui.state("settings_mcp_key_value", "")
-- Adding another account (the "account" step): the account it's another of,
-- the names for the one that has none yet and for the new one, and whether
-- the sign-in under way is a further account's (its browser may still be
-- signed in as the first).
local connect_source = ui.state("settings_mcp_connect_source", nil :: string?)
local account_new = ui.state("settings_mcp_account_new", "")
local connect_another = ui.state("settings_mcp_connect_another", false)
-- Whether this sign-in has been seen under way: a flow's last outcome
-- (done, from an earlier one) doesn't end the one just started.
local armed = false

-- The name the dialog speaks of (a preset's before it's added).
local function connect_name(): string
	local s = server_by_id(connect_server:get())
	if s then
		return full_name(s)
	end
	local source = server_by_id(connect_source:get())
	if source then
		return source.name
	end
	local p = preset_by_id(connect_preset:get())
	return if p then p.name else ""
end

local function connect_icon(): string?
	local s = server_by_id(connect_server:get()) or server_by_id(connect_source:get())
	local p = preset_by_id(if s then s.preset else connect_preset:get())
	return if p then p.icon else nil
end

local function connect_docs(): string?
	local s = server_by_id(connect_server:get()) or server_by_id(connect_source:get())
	local p = preset_by_id(if s then s.preset else connect_preset:get())
	return if p and p.docsUrl ~= "" then p.docsUrl else nil
end

local function settle_error(err: any)
	connect_busy:set(false)
	local text = message_of(err)
	if text == "cancelled" then
		return
	end
	if wants_client(text) then
		connect_error:set("")
		connect_step:set("client")
	else
		connect_error:set(friendly(text))
		connect_step:set("error")
	end
	connect_open:set(true)
end

local function sign_in(id: string, opts: { clientId: string?, clientSecret: string? }?, another: boolean?)
	connect_another:set(another == true)
	connect_source:set(nil)
	connect_server:set(id)
	connect_error:set("")
	connect_step:set("waiting")
	connect_busy:set(false)
	connect_open:set(true)
	armed = false
	mcp.start_auth({
		id = id,
		clientId = if opts then opts.clientId else nil,
		clientSecret = if opts then opts.clientSecret else nil,
	}):after(function() end, settle_error)
end

local function upsert_preset(p: mcp.Preset, extra: { [string]: any }?): any
	local req: { [string]: any } = {
		name = p.name,
		preset = p.id,
		transport = p.transport,
		url = p.url,
		authKind = p.authKind,
		enabled = true,
		localOnly = not snapshot().vaultAvailable,
	}
	for k, v in extra or {} do
		req[k] = v
	end
	return mcp.upsert(req :: any)
end

-- The server an upsert made or changed, from its reply (else the list).
-- (One named by its id; else the one that wasn't there before, never an
-- earlier account of the same preset; else the preset's.)
local function saved_server(reply: any, id: string?, preset: string?, before: { [string]: boolean }): mcp.Server?
	local servers: { mcp.Server } = if type(reply) == "table" and reply.servers then reply.servers else (mcp.now:peek().servers or {})
	if id then
		for _, s in servers do
			if s.id == id then
				return s
			end
		end
	end
	for _, s in servers do
		if not before[s.id] then
			return s
		end
	end
	if preset then
		for _, s in servers do
			if s.preset == preset then
				return s
			end
		end
	end
	return nil
end

local function ids_now(): { [string]: boolean }
	local out: { [string]: boolean } = {}
	for _, s in mcp.now:peek().servers or {} do
		out[s.id] = true
	end
	return out
end

-- Connect, from the list of servers to add.
local function connect(p: mcp.Preset)
	local existing = server_for_preset(p)
	if existing then
		if existing.authKind == "oauth" then
			sign_in(existing.id)
		end
		return
	end
	connect_server:set(nil)
	connect_source:set(nil)
	connect_another:set(false)
	connect_preset:set(p.id)
	connect_error:set("")
	connect_busy:set(false)
	if p.authKind == "bearer" or p.authKind == "headers" then
		key_value:set("")
		connect_step:set("key")
		connect_open:set(true)
		return
	elseif p.authKind == "oauth" and (p :: any).clientRequired then
		client_id:set("")
		client_secret:set("")
		connect_step:set("client")
		connect_open:set(true)
		return
	end
	local before = ids_now()
	upsert_preset(p):after(function(reply: any)
		local s = saved_server(reply, nil, p.id, before)
		if p.authKind == "oauth" and s then
			sign_in(s.id)
		else
			toast.show { title = ui.t("settings.mcp.toast.added", { name = p.name }), tone = "success" }
		end
	end, function(err: any)
		toast.show { title = ui.t("settings.mcp.toast.couldnt_add", { name = p.name }), body = friendly(message_of(err)), tone = "danger" }
	end)
end

-- Another account of `s`'s service: a name for it (or none), then its
-- sign-in, as Providers' Add account.
local function add_account(s: mcp.Server)
	connect_server:set(nil)
	connect_preset:set(s.preset)
	connect_source:set(s.id)
	connect_another:set(true)
	connect_error:set("")
	connect_busy:set(false)
	account_new:set("")
	key_value:set("")
	connect_step:set("account")
	connect_open:set(true)
end

-- What's wrong with the name typed, if anything: nil when it can be used,
-- "" when there's nothing to say yet (a key still to paste).
local function account_problem(): string?
	local source = server_by_id(connect_source:get())
	if not source then
		return ""
	end
	local new = string.lower(trim(account_new:get()))
	for _, other in accounts_of(source) do
		local label = other.accountLabel
		if new ~= "" and label and string.lower(label) == new then
			return ui.t("settings.mcp.account.error_taken", { name = source.name, account = label }):get()
		end
	end
	if source.authKind == "bearer" and trim(key_value:get()) == "" then
		return ""
	end
	return nil
end

-- The new account, made like the one it's another of (its address, its
-- kind of sign-in, where it's kept), and its sign-in started.
local function create_account()
	local source = server_by_id(connect_source:peek())
	if not source or account_problem() ~= nil then
		return
	end
	local req: { [string]: any } = {
		name = source.name,
		preset = source.preset,
		transport = source.transport,
		url = source.url,
		authKind = source.authKind,
		localOnly = source.storage == "local",
		accountLabel = trim(account_new:peek()),
		enabled = true,
	}
	if source.authKind == "bearer" then
		req.bearerToken = trim(key_value:peek())
	end
	local before = ids_now()
	connect_busy:set(true)
	mcp.upsert(req :: any):after(function(reply: any)
		connect_busy:set(false)
		local made = saved_server(reply, nil, nil, before)
		if not made then
			return
		end
		if made.authKind == "oauth" then
			sign_in(made.id, nil, true)
		else
			connect_open:set(false)
			toast.show { title = ui.t("settings.mcp.toast.connected", { name = full_name(made) }), tone = "success" }
		end
	end, settle_error)
end

-- The dialog's way on, by its step.
local function connect_confirm()
	local step = connect_step:peek()
	if connect_busy:peek() then
		return
	end
	if step == "account" then
		create_account()
		return
	elseif step == "error" then
		local id = connect_server:peek()
		local p = preset_by_id(connect_preset:peek())
		if connect_source:peek() and not id then
			connect_step:set("account")
		elseif id then
			sign_in(id, nil, connect_another:peek())
		elseif p then
			connect(p)
		end
		return
	elseif step == "client" then
		local cid, csec = trim(client_id:peek()), trim(client_secret:peek())
		if cid == "" then
			return
		end
		local opts = { clientId = cid, clientSecret = if csec ~= "" then csec else nil }
		local id = connect_server:peek()
		if id then
			sign_in(id, opts)
			return
		end
		local p = preset_by_id(connect_preset:peek())
		if not p then
			return
		end
		connect_busy:set(true)
		local before = ids_now()
		upsert_preset(p, { oauthClientId = opts.clientId, oauthClientSecret = opts.clientSecret }):after(function(reply: any)
			connect_busy:set(false)
			local s = saved_server(reply, nil, p.id, before)
			if s then
				sign_in(s.id, opts)
			end
		end, settle_error)
		return
	elseif step == "key" then
		local p = preset_by_id(connect_preset:peek())
		local value = trim(key_value:peek())
		if not p or value == "" then
			return
		end
		connect_busy:set(true)
		local extra: { [string]: any } = if p.authKind == "headers"
			then { headers = { [p.headerName or "Authorization"] = value } }
			else { bearerToken = value }
		upsert_preset(p, extra):after(function()
			connect_busy:set(false)
			connect_open:set(false)
			toast.show { title = ui.t("settings.mcp.toast.connected", { name = p.name }), tone = "success" }
		end, settle_error)
	end
end

local function cancel_waiting()
	local id = connect_server:peek()
	if id and connect_step:peek() == "waiting" then
		local flow = mcp.auth_flows:peek()[id]
		if flow and flow.phase == "authorizing" then
			mcp.cancel_auth(id, flow.flowId):after(function() end, function() end)
		end
	end
end

local function copy_line(text: () -> string): Node
	local copied = ui.use_state(false)
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		pl = "$space.3",
		pr = "$space.1",
		py = "$space.1",
		rounded = "$radius.lg",
		bg = "$zeron.secondary.bg",
		ui.text {
			text,
			grow = 1,
			min_w = 0,
			font = "$font.mono",
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.primary",
			selectable = true,
		},
		ds.IconButton {
			icon = function()
				return if copied:get() then "check" else "copy"
			end,
			label = ui.t("settings.mcp.client.copy"),
			size = "sm",
			tone = "quiet",
			on_click = function()
				ui.clipboard.write(text())
				copied:set(true)
				ui.after(1400, function()
					copied:set(false)
				end)
			end,
		},
	}
end

local function connect_body(step: string): Node
	if step == "waiting" then
		return ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			px = "$space.4",
			py = "$space.3",
			rounded = "$radius.lg",
			bg = "$zeron.secondary.bg",
			progress.ProgressRing { size = 16, tone = "neutral", label = ui.t("settings.mcp.connect.waiting_label"):get() },
			ui.text {
				function(): string
					if connect_another:get() then
						local s = server_by_id(connect_server:get())
						return ui.t("settings.mcp.account.switch_hint", { name = if s then s.name else "" }):get()
					end
					return ui.t("settings.mcp.connect.waiting_hint"):get()
				end,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
				wrap = true,
			},
		}
	elseif step == "client" then
		return ui.column {
			w_full = true,
			gap = "$space.3",
			ui.column {
				w_full = true,
				gap = "$space.1",
				ui.text {
					ui.t("settings.mcp.client.redirect"),
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					weight = "$font.weight.medium",
					color = "$text.secondary",
				},
				copy_line(function(): string
					local uri = (mcp.now:get() :: any).clientRedirectUri
					return if type(uri) == "string" and uri ~= "" then uri else "http://localhost:38417/callback"
				end),
			},
			ds.TextField {
				value = client_id,
				label = ui.t("settings.mcp.client.id"),
				placeholder = "1234567890.1234567890",
				autofocus = true,
				w_full = true,
			},
			ds.TextField {
				value = client_secret,
				label = ui.t("settings.mcp.client.secret"),
				password = true,
				w_full = true,
			},
			ui.show(function(): boolean
				return connect_docs() ~= nil
			end, ds.Button {
				tone = "ghost",
				size = "sm",
				self = "start",
				on_click = function()
					local url = connect_docs()
					if url then
						host.invoke("open_url", url)
					end
				end,
				ds.Icon { name = "arrow-up-right", size = 14 },
				ui.t("settings.mcp.client.how"),
			}),
		}
	elseif step == "account" then
		local function source(): mcp.Server?
			return server_by_id(connect_source:get())
		end
		return ui.column {
			w_full = true,
			gap = "$space.3",
			ds.TextField {
				value = account_new,
				label = ui.t("settings.mcp.account.name"),
				placeholder = ui.t("settings.mcp.account.name_placeholder"),
				autofocus = true,
				hint = function(): string
					local s = source()
					if not s then
						return ""
					end
					-- (What agents will see it by, once it has a name.)
					local typed = trim(account_new:get())
					if typed == "" then
						return ui.t("settings.mcp.account.name_optional"):get()
					end
					return ui.t("settings.mcp.account.agents_see_one", { agent = agent_preview(s, typed) }):get()
				end,
				error = function(): string?
					local problem = account_problem()
					return if problem and problem ~= "" then problem else nil
				end,
				w_full = true,
			},
			ui.show(function(): boolean
				local s = source()
				return s ~= nil and s.authKind == "bearer"
			end, ds.TextField {
				value = key_value,
				label = ui.t("settings.mcp.account.new_key"),
				password = true,
				w_full = true,
			}),
		}
	elseif step == "key" then
		return ds.TextField {
			value = key_value,
			label = function(): string
				local p = preset_by_id(connect_preset:get())
				return if p and p.headerName and p.headerName ~= "" then p.headerName else ui.t("settings.mcp.key.label"):get()
			end,
			password = true,
			autofocus = true,
			w_full = true,
		}
	end
	return ds.Callout {
		tone = "danger",
		title = function(): string
			return connect_error:get()
		end,
	}
end

local function connect_dialog(): Node
	-- Closed while waiting: the flow it was waiting on is cancelled.
	ui.effect(function()
		if not connect_open:get() then
			cancel_waiting()
		end
	end)
	-- The flow's outcome ends the wait: connected, or why not.
	ui.effect(function()
		local id = connect_server:get()
		local flow = if id then mcp.auth_flows:get()[id] else nil
		if not id or not flow or connect_step:peek() ~= "waiting" or not connect_open:peek() then
			return
		end
		if flow.phase == "authorizing" then
			armed = true
		elseif armed and flow.phase == "done" then
			armed = false
			local name = connect_name()
			connect_open:set(false)
			toast.show { title = ui.t("settings.mcp.toast.connected", { name = name }), tone = "success" }
		elseif armed and flow.phase == "error" then
			armed = false
			settle_error(flow.error)
		end
	end)
	return ds.Dialog {
		open = connect_open,
		size = "sm",
		title = function(): string
			local name = connect_name()
			local step = connect_step:get()
			if step == "waiting" then
				return ui.t("settings.mcp.connect.waiting_title", { name = name }):get()
			elseif step == "error" then
				return ui.t("settings.mcp.connect.error_title", { name = name }):get()
			elseif step == "account" then
				return ui.t("settings.mcp.account.title", { name = name }):get()
			end
			return ui.t("settings.mcp.connect.title", { name = name }):get()
		end,
		body = function(): string
			local name = connect_name()
			local step = connect_step:get()
			if step == "waiting" then
				return ui.t("settings.mcp.connect.waiting_body", { name = name }):get()
			elseif step == "client" then
				return ui.t("settings.mcp.client.body", { name = name }):get()
			elseif step == "key" then
				return ui.t("settings.mcp.key.body", { name = name }):get()
			elseif step == "account" then
				return ui.t("settings.mcp.account.body"):get()
			end
			return ui.t("settings.mcp.connect.error_body"):get()
		end,
		confirm = {
			label = function(): string
				local step = connect_step:get()
				if step == "waiting" then
					return ui.t("settings.mcp.connect.waiting_button"):get()
				elseif step == "error" then
					return ui.t("settings.mcp.connect.try_again"):get()
				elseif step == "key" then
					return ui.t("settings.mcp.connect.connect"):get()
				elseif step == "account" then
					local source = server_by_id(connect_source:get())
					return if source and source.authKind == "oauth"
						then ui.t("settings.mcp.connect.continue"):get()
						else ui.t("settings.mcp.account.add"):get()
				end
				return ui.t("settings.mcp.connect.continue"):get()
			end,
			keep_open = true,
			busy = function(): boolean
				return connect_busy:get() or connect_step:get() == "waiting"
			end,
			disabled = function(): boolean
				local step = connect_step:get()
				if step == "client" then
					return trim(client_id:get()) == ""
				elseif step == "key" then
					return trim(key_value:get()) == ""
				elseif step == "account" then
					return account_problem() ~= nil
				end
				return step == "waiting"
			end,
			on_click = connect_confirm,
		},
		ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			ui.presence(function(): string
				return `{connect_icon() or ""}\0{connect_name()}`
			end, function(key: string): Node
				local icon, name = string.match(key, "^(.-)%z(.*)$")
				return Logo(if icon ~= "" then icon else nil, name or "", 32)
			end),
			ui.column {
				grow = 1,
				min_w = 0,
				gap = "$zeron.space.hair",
				ui.text {
					connect_name,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.text {
					function(): string
						return if snapshot().vaultAvailable
							then ui.t("settings.mcp.connect.synced"):get()
							else ui.t("settings.mcp.connect.local"):get()
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
					wrap = true,
				},
			},
		},
		ui.presence(function(): string
			return connect_step:get()
		end, connect_body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

-- ── A server's settings (and a custom one's adding) ───────────────────

local form_open = ui.state("settings_mcp_form_open", false)
local editing = ui.state("settings_mcp_editing", nil :: string?)
local form_kind = ui.state("settings_mcp_form_kind", "remote")
local form_name = ui.state("settings_mcp_form_name", "")
local form_url = ui.state("settings_mcp_form_url", "")
local form_auth = ui.state("settings_mcp_form_auth", "oauth")
local form_bearer = ui.state("settings_mcp_form_bearer", "")
local form_headers = ui.state("settings_mcp_form_headers", "")
local form_own_client = ui.state("settings_mcp_form_own_client", false)
local form_client_id = ui.state("settings_mcp_form_client_id", "")
local form_client_secret = ui.state("settings_mcp_form_client_secret", "")
local form_scopes = ui.state("settings_mcp_form_scopes", "")
local form_sync = ui.state("settings_mcp_form_sync", true)
local form_command = ui.state("settings_mcp_form_command", "")
local form_args = ui.state("settings_mcp_form_args", "")
local form_env = ui.state("settings_mcp_form_env", "")
local form_error = ui.state("settings_mcp_form_error", "")
local form_busy = ui.state("settings_mcp_form_busy", false)

local remove_open = ui.state("settings_mcp_remove_open", false)
-- What Remove asks about: one account, or a whole service's (`ids`), and
-- what it's called.
local remove_target = ui.state("settings_mcp_remove_target", nil :: { ids: { string }, name: string }?)

local function editing_server(): mcp.Server?
	return server_by_id(editing:get())
end

-- A server added from the list: its address and sign-in are its preset's.
local function editing_preset(): boolean
	local s = editing_server()
	return s ~= nil and s.preset ~= nil and s.preset ~= ""
end

local function open_add()
	editing:set(nil)
	form_kind:set("remote")
	form_name:set("")
	form_url:set("")
	form_auth:set("oauth")
	form_bearer:set("")
	form_headers:set("")
	form_own_client:set(false)
	form_client_id:set("")
	form_client_secret:set("")
	form_scopes:set("")
	form_sync:set(true)
	form_command:set("")
	form_args:set("")
	form_env:set("")
	form_error:set("")
	form_busy:set(false)
	form_open:set(true)
end

local function open_manage(s: mcp.Server)
	open_add()
	editing:set(s.id)
	form_kind:set(if s.transport == "stdio" then "local" else "remote")
	form_name:set(s.name or "")
	form_url:set(s.url or "")
	form_auth:set(s.authKind or "none")
	form_sync:set(s.storage ~= "local")
	form_command:set(s.command or "")
	form_args:set(format_args(s.args))
	local scopes = table.concat(s.scopes or {}, " ")
	form_scopes:set(scopes)
	form_open:set(true)
end

local function submit_form()
	if form_busy:peek() then
		return
	end
	form_error:set("")
	local current = server_by_id(editing:peek())
	local name = trim(form_name:peek())
	if name == "" then
		form_error:set(ui.t("settings.mcp.form.error_name"):get())
		return
	end
	local vault = snapshot().vaultAvailable
	local req: { [string]: any }
	local sign_in_after = false
	local opts: { clientId: string?, clientSecret: string? }? = nil
	if current and current.preset and current.preset ~= "" then
		-- (A listed server: its name and where it's kept, nothing else.)
		req = {
			id = current.id,
			name = name,
			preset = current.preset,
			transport = current.transport,
			url = current.url,
			authKind = current.authKind,
			localOnly = if vault then not form_sync:peek() else true,
		}
	elseif form_kind:peek() == "local" then
		local command = trim(form_command:peek())
		if command == "" then
			form_error:set(ui.t("settings.mcp.form.error_command"):get())
			return
		end
		local env, bad = parse_lines(form_env:peek(), "=")
		if bad then
			form_error:set(ui.t("settings.mcp.form.error_env_line", { line = bad }):get())
			return
		end
		req = {
			id = editing:peek(),
			name = name,
			transport = "stdio",
			command = command,
			args = split_args(form_args:peek()),
			env = env,
			authKind = "none",
			localOnly = true,
		}
	else
		local url = trim(form_url:peek())
		if not valid_http_url(url) then
			form_error:set(ui.t("settings.mcp.form.error_url"):get())
			return
		end
		local auth = form_auth:peek()
		req = {
			id = editing:peek(),
			name = name,
			transport = "http",
			url = url,
			authKind = auth,
			headers = {},
			localOnly = if vault then not form_sync:peek() else true,
		}
		if auth == "bearer" then
			local token = trim(form_bearer:peek())
			if token ~= "" then
				req.bearerToken = token
			elseif not current then
				form_error:set(ui.t("settings.mcp.form.error_token"):get())
				return
			end
		elseif auth == "headers" then
			local headers, bad = parse_lines(form_headers:peek(), ":")
			if bad then
				form_error:set(ui.t("settings.mcp.form.error_header_line", { line = bad }):get())
				return
			end
			req.headers = headers
		elseif auth == "oauth" then
			local cid, csec = trim(form_client_id:peek()), trim(form_client_secret:peek())
			if cid ~= "" then
				req.oauthClientId = cid
				opts = { clientId = cid, clientSecret = if csec ~= "" then csec else nil }
			end
			if csec ~= "" then
				req.oauthClientSecret = csec
			end
			req.oauthScopes = split_scopes(form_scopes:peek())
			sign_in_after = current == nil
		end
	end
	local saved_name = if current then full_name(current) else name
	local before = ids_now()
	form_busy:set(true)
	mcp.upsert(req :: any):after(function(reply: any)
		form_busy:set(false)
		form_open:set(false)
		local s = saved_server(reply, req.id, nil, before)
		if sign_in_after and s then
			sign_in(s.id, opts)
		else
			toast.show {
				title = ui.t(if current then "settings.mcp.toast.saved" else "settings.mcp.toast.added", { name = saved_name }),
				tone = "success",
			}
		end
	end, function(err: any)
		form_busy:set(false)
		form_error:set(friendly(message_of(err)))
	end)
end

local function label_text(text: any): Node
	return ui.text {
		text,
		text_size = "$font.size.sm",
		line_height = "$font.line.sm",
		color = "$text.tertiary",
		wrap = true,
	}
end

local function form_dialog(): Node
	local function custom(): boolean
		return not editing_preset()
	end
	local function remote(): boolean
		return custom() and form_kind:get() == "remote"
	end
	-- Kept in the vault or here: a choice while there's a vault, for what
	-- isn't a command on this computer.
	local function syncable(): boolean
		local s = editing_server()
		local is_local = if s then s.transport == "stdio" else form_kind:get() == "local"
		return snapshot().vaultAvailable and not is_local
	end
	return ds.Dialog {
		open = form_open,
		size = "md",
		hold = function(): boolean
			return form_busy:get()
		end,
		title = function(): string
			local s = editing_server()
			return if s then full_name(s) else ui.t("settings.mcp.form.add_title"):get()
		end,
		body = function(): string
			local s = editing_server()
			if not s then
				return ui.t("settings.mcp.form.add_body"):get()
			end
			if s.transport == "stdio" then
				return `{s.command or ""} {format_args(s.args)}`
			end
			return s.url or ""
		end,
		confirm = {
			label = function(): string
				return if editing:get() then ui.t("settings.mcp.form.save"):get() else ui.t("settings.mcp.form.add"):get()
			end,
			keep_open = true,
			busy = function(): boolean
				return form_busy:get()
			end,
			on_click = submit_form,
		},
		ds.TextField {
			value = form_name,
			label = ui.t("settings.mcp.form.name"),
			placeholder = ui.t("settings.mcp.form.name_placeholder"),
			w_full = true,
		},
		ui.show(function(): boolean
			return custom() and editing:get() == nil
		end, ds.Segmented {
			value = form_kind,
			fill = true,
			label = ui.t("settings.mcp.form.kind"),
			options = {
				{ value = "remote", label = ui.t("settings.mcp.form.kind_remote") },
				{ value = "local", label = ui.t("settings.mcp.form.kind_local") },
			},
		}),
		-- A remote server: its address and how it signs in.
		ui.show(remote, ui.column {
			w_full = true,
			gap = "$space.3",
			ds.TextField {
				value = form_url,
				label = ui.t("settings.mcp.form.url"),
				placeholder = "https://mcp.example.com/mcp",
				inputmode = "url",
				w_full = true,
			},
			ds.Select {
				value = form_auth,
				label = ui.t("settings.mcp.form.auth"),
				w_full = true,
				options = function(): { ds_select.Option }
					return {
						{ value = "oauth", label = ui.t("settings.mcp.form.auth_oauth"):get() },
						{ value = "bearer", label = ui.t("settings.mcp.form.auth_bearer"):get() },
						{ value = "headers", label = ui.t("settings.mcp.form.auth_headers"):get() },
						{ value = "none", label = ui.t("settings.mcp.form.auth_none"):get() },
					}
				end,
			},
			ui.show(function(): boolean
				return form_auth:get() == "bearer"
			end, ds.TextField {
				value = form_bearer,
				label = ui.t("settings.mcp.form.token"),
				hint = function(): string
					local s = editing_server()
					return if s and s.authKind == "bearer" then ui.t("settings.mcp.form.token_kept"):get() else ""
				end,
				password = true,
				w_full = true,
			}),
			ui.show(function(): boolean
				return form_auth:get() == "headers"
			end, ds.TextArea {
				value = form_headers,
				label = ui.t("settings.mcp.form.headers"),
				placeholder = "X-Api-Key: …",
				hint = function(): string
					local s = editing_server()
					local keys = if s then s.headerKeys or {} else {}
					return if #keys > 0
						then ui.t("settings.mcp.form.headers_kept", { keys = table.concat(keys, ", ") }):get()
						else ui.t("settings.mcp.form.headers_hint"):get()
				end,
				rows = 2,
				max_rows = 6,
				mono = true,
				w_full = true,
			}),
			-- Your own OAuth app, for a server that registers none itself.
			ui.show(function(): boolean
				return form_auth:get() == "oauth"
			end, ui.column {
				w_full = true,
				gap = "$space.3",
				ui.show(function(): boolean
					return not form_own_client:get()
				end, ds.Button {
					tone = "ghost",
					size = "sm",
					self = "start",
					on_click = function()
						form_own_client:set(true)
					end,
					ui.t("settings.mcp.form.own_client"),
				}),
				ui.show(form_own_client, ui.column {
					w_full = true,
					gap = "$space.3",
					label_text(ui.t("settings.mcp.form.own_client_hint")),
					copy_line(function(): string
						local uri = (mcp.now:get() :: any).clientRedirectUri
						return if type(uri) == "string" and uri ~= "" then uri else "http://localhost:38417/callback"
					end),
					ds.TextField { value = form_client_id, label = ui.t("settings.mcp.client.id"), w_full = true },
					ds.TextField { value = form_client_secret, label = ui.t("settings.mcp.client.secret"), password = true, w_full = true },
					ds.TextField {
						value = form_scopes,
						label = ui.t("settings.mcp.form.scopes"),
						placeholder = ui.t("settings.mcp.form.scopes_placeholder"),
						w_full = true,
					},
				}),
			}),
		}),
		-- A command on this computer: what runs, with what.
		ui.show(function(): boolean
			return custom() and form_kind:get() == "local"
		end, ui.column {
			w_full = true,
			gap = "$space.3",
			ds.TextField {
				value = form_command,
				label = ui.t("settings.mcp.form.command"),
				placeholder = "npx",
				w_full = true,
			},
			ds.TextField {
				value = form_args,
				label = ui.t("settings.mcp.form.args"),
				placeholder = "-y @modelcontextprotocol/server-filesystem ~/Documents",
				w_full = true,
			},
			ds.TextArea {
				value = form_env,
				label = ui.t("settings.mcp.form.env"),
				placeholder = "API_KEY=…",
				hint = function(): string
					local s = editing_server()
					local keys = if s then s.envKeys or {} else {}
					return if #keys > 0 then ui.t("settings.mcp.form.env_kept", { keys = table.concat(keys, ", ") }):get() else ""
				end,
				rows = 2,
				max_rows = 6,
				mono = true,
				w_full = true,
			},
			label_text(ui.t("settings.mcp.form.local_note")),
		}),
		-- Where it's kept (signing in, more accounts and removing are the
		-- server's card's, on the page).
		ui.show(syncable, ds.Settings.Group {
			ds.Settings.Row {
				label = ui.t("settings.mcp.form.sync"),
				description = ui.t("settings.mcp.form.sync_description"),
				control = ds.Switch { value = form_sync, label = ui.t("settings.mcp.form.sync") },
			},
		}),
		ui.show(function(): boolean
			return form_error:get() ~= ""
		end, ui.text {
			function(): string
				return form_error:get()
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$zeron.tone.danger.ink",
			wrap = true,
		}),
	}
end

local function remove_dialog(): Node
	return ds.Dialog {
		open = remove_open,
		tone = "danger",
		title = function(): string
			local t = remove_target:get()
			return ui.t("settings.mcp.remove.title", { name = if t then t.name else "" }):get()
		end,
		body = function(): string
			local t = remove_target:get()
			local n = if t then #t.ids else 1
			return if n > 1
				then ui.t("settings.mcp.remove.body_all", { n = n }):get()
				else ui.t("settings.mcp.remove.body"):get()
		end,
		confirm = {
			label = ui.t("common.remove"),
			on_click = function()
				local t = remove_target:peek()
				if not t then
					return
				end
				-- (One after another: each answer is the list as it then stands.)
				local function next(i: number)
					local id = t.ids[i]
					if not id then
						toast.show { title = ui.t("settings.mcp.toast.removed", { name = t.name }), tone = "neutral" }
						return
					end
					mcp.delete(id):after(function()
						next(i + 1)
					end, function(err: any)
						toast.show { title = ui.t("settings.mcp.toast.couldnt_remove", { name = t.name }), body = friendly(message_of(err)), tone = "danger" }
					end)
				end
				next(1)
			end,
		},
	}
end

-- Renaming an account: what it's called, here and (slugged) to agents.
local rename_open = ui.state("settings_mcp_rename_open", false)
local rename_id = ui.state("settings_mcp_rename_id", nil :: string?)
local rename_value = ui.state("settings_mcp_rename_value", "")
local rename_busy = ui.state("settings_mcp_rename_busy", false)
local rename_error = ui.state("settings_mcp_rename_error", "")

local function rename(s: mcp.Server)
	rename_id:set(s.id)
	rename_value:set(s.accountLabel or "")
	rename_error:set("")
	rename_busy:set(false)
	rename_open:set(true)
end

local function rename_problem(): string?
	local s = server_by_id(rename_id:get())
	if not s then
		return nil
	end
	local typed = string.lower(trim(rename_value:get()))
	for _, other in accounts_of(s) do
		local label = other.accountLabel
		if other.id ~= s.id and typed ~= "" and label and string.lower(label) == typed then
			return ui.t("settings.mcp.account.error_taken", { name = s.name, account = label }):get()
		end
	end
	return nil
end

local function save_rename()
	local s = server_by_id(rename_id:peek())
	if not s or rename_busy:peek() or rename_problem() then
		return
	end
	rename_busy:set(true)
	rename_error:set("")
	mcp.upsert({
		id = s.id,
		name = s.name,
		preset = s.preset,
		transport = s.transport,
		url = s.url,
		command = s.command,
		args = s.args,
		authKind = s.authKind,
		localOnly = s.storage == "local",
		accountLabel = trim(rename_value:peek()),
	} :: any):after(function()
		rename_busy:set(false)
		rename_open:set(false)
	end, function(err: any)
		rename_busy:set(false)
		rename_error:set(friendly(message_of(err)))
	end)
end

local function rename_dialog(): Node
	return ds.Dialog {
		open = rename_open,
		size = "sm",
		title = ui.t("settings.mcp.rename.title"),
		body = function(): string
			local s = server_by_id(rename_id:get())
			return if s then ui.t("settings.mcp.rename.body", { name = s.name }):get() else ""
		end,
		hold = function(): boolean
			return rename_busy:get()
		end,
		confirm = {
			label = ui.t("settings.mcp.rename.save"),
			keep_open = true,
			busy = function(): boolean
				return rename_busy:get()
			end,
			disabled = function(): boolean
				return rename_problem() ~= nil
			end,
			on_click = save_rename,
		},
		build = function(): Node
			return ds.TextField {
				value = rename_value,
				label = ui.t("settings.mcp.account.name"),
				placeholder = ui.t("settings.mcp.account.name_placeholder"),
				autofocus = true,
				select_on_focus = true,
				hint = function(): string
					local s = server_by_id(rename_id:get())
					local typed = trim(rename_value:get())
					if not s then
						return ""
					end
					-- (What it'll be: alone, a service keeps its own name.)
					local agent = if #accounts_of(s) < 2 then (s.agentName or "") else agent_preview(s, typed)
					return if typed == "" and #accounts_of(s) > 1
						then ui.t("settings.mcp.rename.unnamed_hint"):get()
						elseif agent ~= "" then ui.t("settings.mcp.account.agents_see_one", { agent = agent }):get()
						else ""
				end,
				error = function(): string?
					local problem = rename_problem()
					if problem then
						return problem
					end
					local e = rename_error:get()
					return if e ~= "" then e else nil
				end,
				w_full = true,
			}
		end,
	}
end

-- ── The page ───────────────────────────────────────────────────────────

-- What a server is doing, for its row.
local function state_of(s: mcp.Server): string
	if not s.enabled then
		return "off"
	end
	local flow = mcp.auth_flows:get()[s.id]
	if flow and flow.phase == "authorizing" then
		return "waiting"
	end
	local err = if flow and flow.phase == "error" then flow.error else s.lastError
	if s.authKind == "oauth" and s.status ~= "connected" and wants_client(err) then
		return "client"
	end
	if s.status == "connected" then
		return "connected"
	end
	if flow and flow.phase == "error" then
		return "error"
	end
	if s.authKind == "oauth" and (s.status == "needsAuth" or s.status == "reauthRequired") then
		return if s.status == "reauthRequired" then "expired" else "signin"
	end
	if s.status == "error" then
		return "error"
	end
	return "ready"
end

local function where_of(s: mcp.Server): string
	if s.transport == "stdio" then
		return ui.t("settings.mcp.row.command", { command = s.command or "" }):get()
	end
	return host_of(s.url)
end

-- An account's line: how it stands (the service's address is its card's).
local function describe(s: mcp.Server): string
	local st = state_of(s)
	local line: string
	if st == "connected" or st == "ready" then
		line = ui.t("settings.mcp.row.connected"):get()
	elseif st == "waiting" then
		line = ui.t("settings.mcp.row.waiting"):get()
	elseif st == "signin" then
		line = ui.t("settings.mcp.row.signin"):get()
	elseif st == "expired" then
		line = ui.t("settings.mcp.row.expired"):get()
	elseif st == "client" then
		line = ui.t("settings.mcp.row.client"):get()
	elseif st == "error" then
		local flow = mcp.auth_flows:get()[s.id]
		line = friendly(if flow and flow.error then flow.error else s.lastError)
	else
		line = ui.t("settings.mcp.row.off_short"):get()
	end
	if s.storage == "local" and snapshot().vaultAvailable then
		line = ui.t("settings.mcp.row.this_device", { line = line }):get()
	end
	return line
end

-- An account's name on its row: the one it was given, else what it is.
local function account_title(s: mcp.Server): string
	local label = s.accountLabel
	if label and label ~= "" then
		return label
	end
	return if #accounts_of(s) > 1 then ui.t("settings.mcp.row.unnamed"):get() else ui.t("settings.mcp.row.your_account"):get()
end

-- The ⋯ at a row's end and its menu, as Providers' accounts have.
local MenuButton = ui.component(function(props: { label: () -> string, items: (close: () -> ()) -> { Node } }): Node
	local open = ui.use_state(false)
	local function close()
		open:set(false)
	end
	local trigger = ds.IconButton {
		icon = "more-horizontal",
		label = props.label,
		size = "md",
		tone = "quiet",
		on_click = function()
			open:set(not open:peek())
		end,
	}
	return ui.row {
		shrink = 0,
		ui.menu(Menu.panel {
			open = open,
			anchor = trigger,
			placement = { side = "bottom", align = "end", offset = 4 },
			w = 232,
			ui.column {
				w_full = true,
				p = "$space.1",
				gap = "$zeron.list.gap",
				table.unpack(props.items(close)),
			},
		}),
	}
end)

-- Sign in where it's wanted; the spinner while it's under way.
local function sign_in_control(now: State<mcp.Server>): Node
	return ui.switch(function(): string
		local s = now:get()
		local st = state_of(s)
		if st == "waiting" then
			return "waiting"
		elseif s.authKind == "oauth" and (st == "signin" or st == "expired" or st == "client" or st == "error") then
			return "sign_in"
		end
		return "none"
	end, {
		waiting = ui.div { px = "$space.2", progress.ProgressRing { size = 14, tone = "neutral", label = ui.t("settings.mcp.row.waiting"):get() } },
		sign_in = ds.Button {
			ui.t("settings.mcp.row.sign_in"),
			tone = "secondary",
			size = "sm",
			on_click = function()
				local s = now:peek()
				sign_in(s.id, nil, #accounts_of(s) > 1)
			end,
		},
		none = ui.div {},
	})
end

local AccountRow = ui.component(function(props: { now: State<mcp.Server> }): Node
	local now = props.now
	return ds.Settings.Row {
		label = function(): string
			return account_title(now:get())
		end,
		description = function(): string
			return describe(now:get())
		end,
		lead = ds.Avatar {
			seed = now:peek().id,
			size = 32,
			label = function(): string
				return account_title(now:get())
			end :: any,
		},
		-- Right after its name: what agents call it, and what needs you.
		badge = ui.row {
			gap = "$space.1",
			items = "center",
			shrink = 0,
			ui.show(function(): boolean
				local agent = now:get().agentName
				return agent ~= nil and agent ~= ""
			end, ds.Badge {
				function(): string
					return now:get().agentName or ""
				end,
				tone = "neutral",
				mono = true,
				class = "mcp-agent-name",
				tooltip = ui.t("settings.mcp.row.agent_name_tip"):get(),
			}),
			ui.show(function(): boolean
				local st = state_of(now:get())
				return st == "error" or st == "client"
			end, ds.Badge { ui.t("settings.mcp.row.needs_attention"), tone = "danger" }),
		},
		truncate = true,
		control = ui.row {
			gap = "$space.2",
			items = "center",
			sign_in_control(now),
			MenuButton {
				label = function(): string
					return ui.t("settings.mcp.row.account_actions", { name = full_name(now:get()) }):get()
				end,
				items = function(close: () -> ()): { Node }
					local function signed_in(): boolean
						local s = now:get()
						return s.authKind == "oauth" and s.status == "connected"
					end
					return {
						Menu.Row {
							icon = "pen",
							label = ui.t("settings.mcp.menu.rename"),
							on_click = function()
								close()
								rename(now:peek())
							end,
						},
						ui.show(signed_in, Menu.Row {
							icon = "refresh",
							label = ui.t("settings.mcp.menu.sign_in_again"),
							on_click = function()
								close()
								local s = now:peek()
								sign_in(s.id, nil, #accounts_of(s) > 1)
							end,
						}),
						ui.show(signed_in, Menu.Row {
							icon = "close-circle",
							label = ui.t("settings.mcp.menu.sign_out"),
							on_click = function()
								close()
								local s = now:peek()
								mcp.disconnect_auth(s.id):after(function()
									toast.show { title = ui.t("settings.mcp.toast.signed_out", { name = full_name(s) }), tone = "neutral" }
								end, function(err: any)
									toast.show { title = friendly(message_of(err)), tone = "danger" }
								end)
							end,
						}),
						Menu.Rule(),
						Menu.Row {
							icon = "trash-bin-minimalistic",
							label = ui.t("settings.mcp.menu.remove_account"),
							danger = true,
							on_click = function()
								close()
								local s = now:peek()
								remove_target:set({ ids = { s.id }, name = full_name(s) })
								remove_open:set(true)
							end,
						},
					}
				end,
			},
		},
	}
end)

-- A service: its card, as Providers' agents are. Its head names it, says
-- where it is, and turns it on or off for agents (all its accounts); its
-- accounts follow, each its own row; then Add account, for a service that
-- signs in. One that doesn't (Context7, a command here) is its head alone.
type Service = { key: string, servers: { mcp.Server } }

local ServiceCard = ui.component(function(props: { now: State<Service> }): Node
	local now = props.now
	local function head(): mcp.Server
		return now:get().servers[1]
	end
	local function signs_in(): boolean
		local s = head()
		return s ~= nil and takes_accounts(s)
	end
	local on = ui.use_state(false)
	ui.effect(function()
		local any = false
		for _, s in now:get().servers do
			any = any or s.enabled
		end
		if on:peek() ~= any then
			on:set(any)
		end
	end)
	local accounts = ui.computed(function(): { mcp.Server }
		return if signs_in() then now:get().servers else {}
	end)
	local first = now:peek().servers[1]
	return ui.column {
		w_full = true,
		rounded = "$radius.xl",
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
		overflow = "hidden",
		class = "mcp-service",
		ds.Settings.Row {
			label = function(): string
				local s = head()
				return if s then s.name else ""
			end,
			description = function(): string
				local s = head()
				if not s then
					return ""
				end
				if not signs_in() then
					-- (No accounts below to say how it stands: said here.)
					local st = state_of(s)
					if st == "connected" or st == "ready" then
						return where_of(s)
					end
					return describe(s)
				end
				local n = #now:get().servers
				return if n > 1
					then ui.t("settings.mcp.row.where_accounts", { where = where_of(s), n = n }):get()
					else where_of(s)
			end,
			lead = server_logo(first, 32),
			truncate = true,
			control = ui.row {
				gap = "$space.2",
				items = "center",
				ds.Switch {
					value = on,
					label = function(): string
						local s = head()
						return if s then s.name else ""
					end,
					on_change = function(value: boolean)
						for _, s in now:peek().servers do
							if s.enabled ~= value then
								mcp.set_enabled(s.id, value):after(function() end, function(err: any)
									on:set(not value)
									toast.show { title = ui.t("settings.mcp.toast.couldnt_switch", { name = s.name }), body = friendly(message_of(err)), tone = "danger" }
								end)
							end
						end
					end,
				},
				MenuButton {
					label = function(): string
						local s = head()
						return ui.t("settings.mcp.row.server_actions", { name = if s then s.name else "" }):get()
					end,
					items = function(close: () -> ()): { Node }
						local function custom(): boolean
							local s = head()
							return s ~= nil and (s.preset == nil or s.preset == "")
						end
						return {
							ui.show(custom, Menu.Row {
								icon = "settings-minimalistic",
								label = ui.t("settings.mcp.menu.edit"),
								on_click = function()
									close()
									local s = head()
									if s then
										open_manage(s)
									end
								end,
							}),
							ui.show(custom, Menu.Rule()),
							Menu.Row {
								icon = "trash-bin-minimalistic",
								label = function(): string
									local s = head()
									return ui.t("settings.mcp.menu.remove_server", { name = if s then s.name else "" }):get()
								end,
								danger = true,
								on_click = function()
									close()
									local ids: { string } = {}
									for _, s in now:peek().servers do
										table.insert(ids, s.id)
									end
									local s = head()
									remove_target:set({ ids = ids, name = if s then s.name else "" })
									remove_open:set(true)
								end,
							},
						}
					end,
				},
			},
		},
		ui.each(accounts, function(_s: mcp.Server, _i: number, row: State<mcp.Server>): Node
			return ui.column {
				w_full = true,
				ds.Settings.Rule(),
				AccountRow { now = row },
			}
		end, {
			key = function(s: mcp.Server): string
				return s.id
			end,
			w_full = true,
		}),
		ui.show(signs_in, ui.column {
			w_full = true,
			ds.Settings.Rule(),
			ds.Settings.Row {
				label = ui.t("settings.mcp.row.add_account"),
				lead = ds.ProviderCard.disc(ds.Icon { name = "plus", size = 16 }),
				on_click = function()
					local s = head()
					if s then
						add_account(s)
					end
				end,
			},
		}),
	}
end)

-- ── What agents asked for ──────────────────────────────────────────────
-- An agent can't change the servers: it asks (Zeron MCP's
-- request_mcp_change), and the request waits here for you. Allowed, it's
-- done as this page would do it, a sign-in opening as Connect's does.

local function pending_requests(): { mcp.Request }
	local out: { mcp.Request } = {}
	for _, r in snapshot().requests or {} do
		if r.status == "pending" then
			table.insert(out, r)
		end
	end
	return out
end

local function request_title(r: mcp.Request): string
	if r.action == "remove" then
		return ui.t("settings.mcp.requests.remove", { name = r.title }):get()
	elseif r.action == "setEnabled" then
		return ui.t(if r.enabled then "settings.mcp.requests.enable" else "settings.mcp.requests.disable", { name = r.title }):get()
	end
	return ui.t("settings.mcp.requests.add", { name = r.title }):get()
end

-- Who asked, and why, in their words.
local function request_line(r: mcp.Request): string
	local chat: any = nil
	for _, c in sessions.now:get().chats or {} do
		if c.id == r.chatId then
			chat = c
		end
	end
	local who = if chat
		then ui.t("settings.mcp.requests.from_chat", { agent = chat.agent or ui.t("settings.mcp.requests.an_agent"):get(), chat = chat.title }):get()
		else ui.t("settings.mcp.requests.from_agent"):get()
	local reason = r.reason
	return if reason and reason ~= "" then ui.t("settings.mcp.requests.because", { who = who, reason = reason }):get() else who
end

-- What exactly would run or be reached: shown whole before it's allowed.
local function request_detail(r: mcp.Request): string?
	if r.action ~= "add" then
		return nil
	end
	if r.command then
		return ui.t("settings.mcp.requests.runs", { command = `{r.command} {format_args(r.args)}` }):get()
	elseif r.url then
		return r.url
	end
	return nil
end

local function answer(r: mcp.Request, approve: boolean, busy: State<boolean>)
	if busy:peek() then
		return
	end
	busy:set(true)
	mcp.resolve_request(r.id, approve):after(function(view: any)
		busy:set(false)
		if not approve then
			return
		end
		if type(view) ~= "table" or view.status == "failed" then
			toast.show {
				title = ui.t("settings.mcp.requests.couldnt", { name = r.title }),
				body = friendly(if type(view) == "table" then view.error else nil),
				tone = "danger",
			}
			return
		end
		local s = server_by_id(view.serverId)
		if s and s.authKind == "oauth" and s.status ~= "connected" and s.enabled then
			sign_in(s.id, nil, #accounts_of(s) > 1)
		else
			toast.show { title = ui.t("settings.mcp.requests.done", { name = r.title }), tone = "success" }
		end
	end, function(err: any)
		busy:set(false)
		toast.show { title = ui.t("settings.mcp.requests.couldnt", { name = r.title }), body = friendly(message_of(err)), tone = "danger" }
	end)
end

local RequestRow = ui.component(function(props: { now: State<mcp.Request> }): Node
	local now = props.now
	local busy = ui.use_state(false)
	local first = now:peek()
	local preset = preset_by_id(first.preset)
	return ds.Settings.Row {
		label = function(): string
			return request_title(now:get())
		end,
		description = function(): string
			return request_line(now:get())
		end,
		detail = function(): string?
			return request_detail(now:get())
		end,
		lead = if preset
			then Logo(preset.icon, preset.name, 32)
			else ds.ProviderCard.disc(ds.Icon { name = if first.command then "terminal" else "bot", size = 16 }),
		control = ui.row {
			gap = "$space.2",
			items = "center",
			ds.Button {
				ui.t("settings.mcp.requests.decline"),
				tone = "ghost",
				size = "sm",
				disabled = busy,
				on_click = function()
					answer(now:peek(), false, busy)
				end,
			},
			ds.Button {
				ui.t("settings.mcp.requests.allow"),
				tone = "primary",
				size = "sm",
				busy = busy,
				on_click = function()
					answer(now:peek(), true, busy)
				end,
			},
		},
	}
end)

-- The servers, a service's accounts together (the engine's order).
local function services(): { Service }
	local out: { Service } = {}
	local at: { [string]: Service } = {}
	for _, s in snapshot().servers or {} do
		local key = service_of(s)
		local svc = at[key]
		if not svc then
			svc = { key = key, servers = {} }
			at[key] = svc
			table.insert(out, svc)
		end
		table.insert(svc.servers, s)
	end
	return out
end

local function preset_row(p: mcp.Preset): Node
	return ds.Settings.Row {
		label = p.name,
		description = p.description,
		lead = Logo(p.icon, p.name, 32),
		control = ds.Button {
			ui.t("settings.mcp.add.connect"),
			tone = "secondary",
			size = "sm",
			on_click = function()
				connect(p)
			end,
		},
	}
end

function M.view(): Node
	visits.once("mcp", function()
		mcp.refresh():after(function() end, function() end)
	end)

	local servers = ui.computed(function(): { mcp.Server }
		return snapshot().servers or {}
	end)
	-- The servers still to add: those of the list not added yet.
	local to_add = ui.computed(function(): { mcp.Preset }
		local out: { mcp.Preset } = {}
		for _, p in snapshot().presets or {} do
			if not server_for_preset(p) then
				table.insert(out, p)
			end
		end
		return out
	end)

	-- (Found by a search for a custom server: its dialog.)
	ui.effect(function()
		if search.lit:get() == "add" and not form_open:peek() and #to_add:peek() == 0 then
			open_add()
		end
	end)

	-- (A section with nothing in it takes no room, its search anchor's neither:
	-- the anchor is inside what shows it, never round it.)
	local requests = ui.computed(pending_requests)
	local waiting = ui.show(
		function(): boolean
			return #requests:get() > 0
		end,
		search.anchor(
			"requests",
			ds.Settings.Section {
				title = ui.t("settings.mcp.requests.title"),
				description = ui.t("settings.mcp.requests.description"),
				ds.Settings.Each(requests, function(_r: mcp.Request, _i: number, row: State<mcp.Request>): Node
					return RequestRow { now = row }
				end, {
					key = function(r: mcp.Request): string
						return r.id
					end,
				}),
			}
		)
	)

	local yours = ui.show(
		function(): boolean
			return #servers:get() > 0
		end,
		search.anchor(
			"servers",
			ds.Settings.Section {
				title = ui.t("settings.mcp.servers.title"),
				-- A card a service, as Providers has an agent's.
				ui.column {
					w_full = true,
					gap = "$space.3",
					ui.each(ui.computed(services), function(_svc: Service, _i: number, row: State<Service>): Node
						return ServiceCard { now = row }
					end, {
						key = function(svc: Service): string
							return svc.key
						end,
						w_full = true,
					}),
				},
			}
		)
	)

	local add = search.anchor(
		"add",
		ds.Settings.Section {
			title = ui.t("settings.mcp.add.title"),
			ds.Settings.Group {
				ui.column {
					w_full = true,
					ui.each(to_add, function(p: mcp.Preset): Node
						return ui.column {
							w_full = true,
							ui.show(function(): boolean
								local first = to_add:get()[1]
								return first ~= nil and first.id ~= p.id
							end, ds.Settings.Rule()),
							preset_row(p),
						}
					end, {
						key = function(p: mcp.Preset): string
							return p.id
						end,
						w_full = true,
					}),
					ui.show(function(): boolean
						return #to_add:get() > 0
					end, ds.Settings.Rule()),
					ds.Settings.Row {
						label = ui.t("settings.mcp.add.custom"),
						description = ui.t("settings.mcp.add.custom_description"),
						lead = ds.ProviderCard.disc(ds.Icon { name = "plus", size = 16 }),
						on_click = open_add,
					},
				},
			},
		}
	)

	return ds.Settings.Page {
		title = ui.t("settings.mcp.title"),
		description = function(): string
			return if snapshot().vaultAvailable
				then ui.t("settings.mcp.description_synced"):get()
				else ui.t("settings.mcp.description_local"):get()
		end,
		waiting,
		yours,
		add,
		connect_dialog(),
		form_dialog(),
		remove_dialog(),
		rename_dialog(),
	}
end

return M
