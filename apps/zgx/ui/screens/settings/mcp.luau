-- Settings → MCP: managed external MCP servers, Zeron Vault vs local storage
-- status, interactive OAuth sign-in, custom remote/stdio server configuration,
-- and the built-in popular MCP server catalog.

local ds = require("@ds")
local ds_icon = require("@ds/icon")
local ds_select = require("@ds/select")
local toast = require("@ds/toast")
local mcp = require("@account/mcp")
local search = require("@app/screens/settings/search")
local visits = require("@app/screens/settings/visits")

local M = {}

M.ID = "mcp"
M.TITLE = "MCP"

search.index(M.ID, ui.t("settings.mcp.title"), ui.t("settings.mcp.terms"), {
	{ key = "storage", title = ui.t("settings.mcp.storage.title"), terms = ui.t("settings.mcp.storage.terms") },
	{ key = "custom", title = ui.t("settings.mcp.custom.title"), terms = ui.t("settings.mcp.custom.terms") },
	{ key = "configured", title = ui.t("settings.mcp.configured.title"), terms = ui.t("settings.mcp.configured.terms") },
	{ key = "presets", title = ui.t("settings.mcp.presets.title"), terms = ui.t("settings.mcp.presets.terms") },
})

local function trim(s: string): string
	return (string.gsub(s, "^%s*(.-)%s*$", "%1"))
end

local function error_text(err: any): string?
	if type(err) == "string" and err ~= "" then
		return err
	end
	if type(err) == "table" and type(err.message) == "string" and err.message ~= "" then
		return err.message
	end
	return nil
end

local function show_error(err: any)
	toast.show {
		title = ui.t("settings.mcp.toast.failed"),
		body = error_text(err),
		tone = "danger",
	}
end

-- Splits a shell-like argument string into tokens, respecting single/double quotes.
local function split_args(raw: string): { string }
	local out: { string } = {}
	local i = 1
	local len = #raw
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
			else
				if ch == '"' or ch == "'" then
					quote = ch
				elseif string.match(ch, "%s") then
					break
				else
					table.insert(buf, ch)
				end
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
		if part ~= "" then
			table.insert(out, part)
		end
	end
	return out
end

local function parse_headers(raw: string): ({ [string]: string }, string?)
	local out: { [string]: string } = {}
	for line in string.gmatch(raw .. "\n", "([^\n]*)\n") do
		local cleaned = trim(line)
		if cleaned ~= "" then
			local colon = string.find(cleaned, ":", 1, true)
			if not colon then
				return out, cleaned
			end
			local name = trim(string.sub(cleaned, 1, colon - 1))
			local value = trim(string.sub(cleaned, colon + 1))
			if name == "" then
				return out, cleaned
			end
			out[name] = value
		end
	end
	return out, nil
end

local function parse_env(raw: string): ({ [string]: string }, string?)
	local out: { [string]: string } = {}
	for line in string.gmatch(raw .. "\n", "([^\n]*)\n") do
		local cleaned = trim(line)
		if cleaned ~= "" then
			local eq = string.find(cleaned, "=", 1, true)
			if not eq then
				return out, cleaned
			end
			local key = trim(string.sub(cleaned, 1, eq - 1))
			local value = string.sub(cleaned, eq + 1)
			if key == "" then
				return out, cleaned
			end
			out[key] = value
		end
	end
	return out, nil
end

local function valid_http_url(raw: string): boolean
	local lower = string.lower(raw)
	local rest: string? = nil
	if string.sub(lower, 1, 8) == "https://" then
		rest = string.sub(raw, 9)
	elseif string.sub(lower, 1, 7) == "http://" then
		rest = string.sub(raw, 8)
	end
	if not rest or rest == "" then
		return false
	end
	return string.find(rest, "%s") == nil
end

local function format_args(args: { string }?): string
	if not args or #args == 0 then
		return ""
	end
	local parts: { string } = {}
	for _, a in args do
		if string.find(a, "%s") then
			table.insert(parts, `"{a}"`)
		else
			table.insert(parts, a)
		end
	end
	return table.concat(parts, " ")
end

local function effective_status(server: mcp.Server, flow: mcp.AuthFlow?): string
	if not server.enabled then
		return "disabled"
	end
	if flow then
		if flow.phase == "authorizing" then
			return "authorizing"
		elseif flow.phase == "error" then
			return "error"
		end
	end
	return server.status or "needsAuth"
end

local function status_badge(status: string): Node
	if status == "connected" then
		return ds.Badge { ui.t("settings.mcp.status.connected"), tone = "success", dot = true, shape = "pill" }
	elseif status == "authorizing" then
		return ds.Badge { ui.t("settings.mcp.status.authorizing"), tone = "info", dot = true, shape = "pill" }
	elseif status == "reauthRequired" then
		return ds.Badge { ui.t("settings.mcp.status.reauth_required"), tone = "warning", dot = true, shape = "pill" }
	elseif status == "error" then
		return ds.Badge { ui.t("settings.mcp.status.error"), tone = "danger", dot = true, shape = "pill" }
	elseif status == "disabled" then
		return ds.Badge { ui.t("settings.mcp.status.disabled"), tone = "neutral", shape = "pill" }
	end
	return ds.Badge { ui.t("settings.mcp.status.needs_auth"), tone = "warning", dot = true, shape = "pill" }
end

local function storage_badge(storage: string): Node
	if storage == "vault" then
		return ds.Badge { ui.t("settings.mcp.badge.vault"), tone = "accent", icon = "shield-check", shape = "pill" }
	end
	return ds.Badge { ui.t("settings.mcp.badge.local"), tone = "neutral", icon = "laptop", shape = "pill" }
end

local function server_subtitle(server: mcp.Server): string
	if server.transport == "stdio" then
		local cmd = server.command or ""
		local arg_text = format_args(server.args)
		if arg_text ~= "" then
			return `stdio · {cmd} {arg_text}`
		end
		return `stdio · {cmd}`
	end
	local kind = if server.transport == "sse" then "SSE" else "HTTP"
	local url = server.url or ""
	if url ~= "" then
		return `{kind} · {url}`
	end
	return kind
end

local function find_upserted_server(
	snapshot: any,
	before_ids: { [string]: boolean },
	target_id: string?,
	preset_id: string?,
	name: string,
	url: string?
): mcp.Server?
	local servers: { mcp.Server }? = if type(snapshot) == "table" then snapshot.servers else nil
	if not servers then
		servers = mcp.now:peek().servers
	end
	if not servers then
		return nil
	end
	if target_id and target_id ~= "" then
		for _, s in servers do
			if s.id == target_id then
				return s
			end
		end
	end
	if preset_id and preset_id ~= "" then
		for _, s in servers do
			if s.preset == preset_id or s.id == preset_id then
				return s
			end
		end
	end
	for _, s in servers do
		if not before_ids[s.id] then
			return s
		end
	end
	local best: mcp.Server? = nil
	for _, s in servers do
		if s.name == name and (url == nil or s.url == url) then
			if not best or (s.updatedAt or 0) >= (best.updatedAt or 0) then
				best = s
			end
		end
	end
	return best
end

function M.view(): Node
	visits.once("mcp", function()
		mcp.refresh():after(function() end, function() end)
	end)

	local refreshing = ui.use_state(false)
	local preset_query = ui.state("settings_mcp_preset_query", "")
	local preset_busy = ui.use_state({} :: { [string]: boolean })

	-- Custom / Edit server form state.
	local form_open = ui.use_state(false)
	local form_saving = ui.use_state(false)
	local editing_id = ui.use_state(nil :: string?)
	local editing_preset = ui.use_state(nil :: string?)
	local editing_has_bearer = ui.use_state(false)
	local editing_header_keys = ui.use_state({} :: { string })
	local editing_env_keys = ui.use_state({} :: { string })
	local form_transport = ui.use_state("http" :: mcp.Transport)
	local form_name = ui.use_state("")
	local form_url = ui.use_state("")
	local form_auth_kind = ui.use_state("oauth" :: mcp.AuthKind)
	local form_bearer = ui.use_state("")
	local form_headers = ui.use_state("")
	local form_oauth_advanced = ui.use_state(false)
	local form_client_id = ui.use_state("")
	local form_client_secret = ui.use_state("")
	local form_scopes = ui.use_state("")
	local form_local_only = ui.use_state(false)
	local form_command = ui.use_state("")
	local form_args = ui.use_state("")
	local form_env = ui.use_state("")
	local form_error = ui.use_state(nil :: string?)

	-- Remove confirmation dialog state.
	local remove_open = ui.use_state(false)
	local remove_target = ui.use_state(nil :: { id: string, name: string }?)
	local remove_busy = ui.use_state(false)

	local function reset_form()
		editing_id:set(nil)
		editing_preset:set(nil)
		editing_has_bearer:set(false)
		editing_header_keys:set({})
		editing_env_keys:set({})
		form_transport:set("http")
		form_name:set("")
		form_url:set("")
		form_auth_kind:set("oauth")
		form_bearer:set("")
		form_headers:set("")
		form_oauth_advanced:set(false)
		form_client_id:set("")
		form_client_secret:set("")
		form_scopes:set("")
		form_local_only:set(false)
		form_command:set("")
		form_args:set("")
		form_env:set("")
		form_error:set(nil)
	end

	local function open_add_form()
		if form_open:peek() and editing_id:peek() == nil and editing_preset:peek() == nil then
			form_open:set(false)
			return
		end
		reset_form()
		form_open:set(true)
	end

	local function open_edit_form(server: mcp.Server)
		reset_form()
		editing_id:set(server.id)
		editing_preset:set(server.preset)
		editing_has_bearer:set(server.authKind == "bearer")
		editing_header_keys:set(server.headerKeys or {})
		editing_env_keys:set(server.envKeys or {})
		form_transport:set(server.transport)
		form_name:set(server.name or "")
		form_url:set(server.url or "")
		form_auth_kind:set(server.authKind or "none")
		form_local_only:set(server.storage == "local")
		form_command:set(server.command or "")
		form_args:set(format_args(server.args))
		local scopes_text = table.concat(server.scopes or {}, " ")
		form_scopes:set(scopes_text)
		form_oauth_advanced:set(scopes_text ~= "")
		form_open:set(true)
	end

	local function open_preset_form(preset: mcp.Preset)
		reset_form()
		editing_preset:set(preset.id)
		form_transport:set(preset.transport)
		form_name:set(preset.name)
		form_url:set(preset.url)
		form_auth_kind:set(preset.authKind)
		if preset.authKind == "headers" and preset.headerName and preset.headerName ~= "" then
			form_headers:set(`{preset.headerName}: `)
		end
		form_open:set(true)
	end

	local function begin_oauth(server_id: string, server_name: string, opts: { clientId: string?, clientSecret: string?, scopes: { string }? }?)
		local req: mcp.StartAuthRequest = {
			id = server_id,
			clientId = if opts then opts.clientId else nil,
			clientSecret = if opts then opts.clientSecret else nil,
			scopes = if opts then opts.scopes else nil,
		}
		mcp.start_auth(req):after(function()
			toast.show {
				title = ui.t("settings.mcp.toast.auth_started", { name = server_name }),
				tone = "info",
			}
		end, function(err: any)
			show_error(err)
		end)
	end

	local function submit_form(and_sign_in: boolean)
		if form_saving:peek() then
			return
		end
		form_error:set(nil)

		local name = trim(form_name:peek())
		if name == "" then
			form_error:set(ui.t("settings.mcp.form.error_name"):get())
			return
		end

		local transport = form_transport:peek()
		local req: mcp.UpsertRequest
		local oauth_client_id: string? = nil
		local oauth_client_secret: string? = nil
		local oauth_scopes: { string } = {}
		local target_url: string? = nil

		if transport == "stdio" then
			local command = trim(form_command:peek())
			if command == "" then
				form_error:set(ui.t("settings.mcp.form.error_command"):get())
				return
			end
			local env_map, bad_env = parse_env(form_env:peek())
			if bad_env then
				form_error:set(ui.t("settings.mcp.form.error_env_line", { line = bad_env }):get())
				return
			end
			req = {
				id = editing_id:peek(),
				name = name,
				preset = editing_preset:peek(),
				transport = "stdio",
				command = command,
				args = split_args(form_args:peek()),
				env = env_map,
				authKind = "none",
				localOnly = true,
			}
		else
			local url = trim(form_url:peek())
			if not valid_http_url(url) then
				form_error:set(ui.t("settings.mcp.form.error_url"):get())
				return
			end
			target_url = url

			local auth_kind = form_auth_kind:peek()
			local bearer_token: string? = nil
			local headers_map: { [string]: string } = {}

			if auth_kind == "bearer" then
				local tok = trim(form_bearer:peek())
				if tok ~= "" then
					bearer_token = tok
				end
			elseif auth_kind == "headers" then
				local parsed, bad_header = parse_headers(form_headers:peek())
				if bad_header then
					form_error:set(ui.t("settings.mcp.form.error_header_line", { line = bad_header }):get())
					return
				end
				headers_map = parsed
			elseif auth_kind == "oauth" then
				local cid = trim(form_client_id:peek())
				local csec = trim(form_client_secret:peek())
				oauth_client_id = if cid ~= "" then cid else nil
				oauth_client_secret = if csec ~= "" then csec else nil
				oauth_scopes = split_scopes(form_scopes:peek())
			end

			local vault_available = mcp.now:peek().vaultAvailable
			req = {
				id = editing_id:peek(),
				name = name,
				preset = editing_preset:peek(),
				transport = transport,
				url = url,
				authKind = auth_kind,
				bearerToken = bearer_token,
				headers = headers_map,
				localOnly = if vault_available then form_local_only:peek() else true,
				oauthClientId = oauth_client_id,
				oauthClientSecret = oauth_client_secret,
				oauthScopes = oauth_scopes,
			}
		end

		local before_ids: { [string]: boolean } = {}
		for _, s in mcp.now:peek().servers or {} do
			before_ids[s.id] = true
		end
		local was_editing = editing_id:peek() ~= nil

		form_saving:set(true)
		mcp.upsert(req):after(function(reply: any)
			form_saving:set(false)
			form_open:set(false)
			local saved = find_upserted_server(reply, before_ids, req.id, req.preset, name, target_url)
			if and_sign_in and saved and req.authKind == "oauth" then
				begin_oauth(saved.id, saved.name, {
					clientId = oauth_client_id,
					clientSecret = oauth_client_secret,
					scopes = if #oauth_scopes > 0 then oauth_scopes else nil,
				})
			else
				toast.show {
					title = ui.t(if was_editing then "settings.mcp.toast.saved" else "settings.mcp.toast.added", { name = name }),
					tone = "success",
				}
			end
		end, function(err: any)
			form_saving:set(false)
			form_error:set(error_text(err) or ui.t("settings.mcp.toast.failed"):get())
		end)
	end

	local function connect_preset(preset: mcp.Preset)
		if preset.authKind == "bearer" or preset.authKind == "headers" then
			open_preset_form(preset)
			return
		end
		local busy_map = preset_busy:peek()
		if busy_map[preset.id] then
			return
		end
		preset_busy:set_key(preset.id, true)

		local before_ids: { [string]: boolean } = {}
		for _, s in mcp.now:peek().servers or {} do
			before_ids[s.id] = true
		end

		local vault_available = mcp.now:peek().vaultAvailable
		local req: mcp.UpsertRequest = {
			name = preset.name,
			preset = preset.id,
			transport = preset.transport,
			url = preset.url,
			authKind = preset.authKind,
			enabled = true,
			localOnly = not vault_available,
		}
		mcp.upsert(req):after(function(reply: any)
			preset_busy:set_key(preset.id, nil :: any)

			local created = find_upserted_server(reply, before_ids, nil, preset.id, preset.name, preset.url)
			if preset.authKind == "oauth" and created then
				begin_oauth(created.id, created.name, nil)
			else
				toast.show {
					title = ui.t("settings.mcp.toast.added", { name = preset.name }),
					tone = "success",
				}
			end
		end, function(err: any)
			preset_busy:set_key(preset.id, nil :: any)
			show_error(err)
		end)
	end

	local function server_for_preset(preset: mcp.Preset): mcp.Server?
		for _, s in mcp.now:get().servers or {} do
			if s.preset == preset.id or s.id == preset.id or (s.url ~= nil and s.url == preset.url) then
				return s
			end
		end
		return nil
	end

	local servers_list = ui.computed(function(): { mcp.Server }
		return mcp.now:get().servers or {}
	end)

	local filtered_presets = ui.computed(function(): { mcp.Preset }
		local presets = mcp.now:get().presets or {}
		local q = string.lower(trim(preset_query:get()))
		if q == "" then
			return presets
		end
		local out: { mcp.Preset } = {}
		for _, p in presets do
			local hay = string.lower(`{p.name} {p.category} {p.description} {p.id}`)
			if string.find(hay, q, 1, true) ~= nil then
				table.insert(out, p)
			end
		end
		return out
	end)

	-- 1. Storage status section.
	local storage_section = search.anchor(
		"storage",
		ds.Settings.Section {
			title = ui.t("settings.mcp.storage.title"),
			ds.Settings.Group {
				ui.presence(function(): string
					return if mcp.now:get().vaultAvailable then "vault" else "local"
				end, function(mode: string): Node
					if mode == "vault" then
						return ds.Settings.Row {
							icon = "shield-check",
							label = ui.t("settings.mcp.storage.vault_title"),
							description = ui.t("settings.mcp.storage.vault_description"),
							control = ds.Badge { ui.t("settings.mcp.storage.synced_badge"), tone = "success", dot = true, shape = "pill" },
						}
					end
					return ds.Settings.Row {
						icon = "laptop",
						label = ui.t("settings.mcp.storage.local_title"),
						description = ui.t("settings.mcp.storage.local_description"),
						control = ds.Badge { ui.t("settings.mcp.storage.local_badge"), tone = "neutral", shape = "pill" },
					}
				end, { w_full = true }),
			},
		}
	)

	ui.effect(function()
		if search.lit:get() == "custom" and not form_open:peek() then
			reset_form()
			form_open:set(true)
		end
	end)

	-- 2. Collapsible Add / Edit Custom MCP Server form.
	local custom_form_section = ui.show(form_open, function(): Node
		return search.anchor(
			"custom",
			ds.Settings.Section {
				title = ui.t("settings.mcp.custom.title"),
				description = ui.t("settings.mcp.form.description"),
				ds.Settings.Group {
				ui.column {
					w_full = true,
					px = "$space.4",
					py = "$space.4",
					gap = "$space.3",
					ui.text {
						function(): string
							return ui.t(if editing_id:get() ~= nil then "settings.mcp.form.edit_title" else "settings.mcp.form.add_title"):get()
						end,
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						weight = "$font.weight.semibold",
						color = "$text.primary",
					},
					ds.Select {
						value = form_transport,
						label = ui.t("settings.mcp.form.transport"),
						w_full = true,
						options = function(): { ds_select.Option }
							return {
								{ value = "http", label = ui.t("settings.mcp.form.transport_http"):get() },
								{ value = "sse", label = ui.t("settings.mcp.form.transport_sse"):get() },
								{ value = "stdio", label = ui.t("settings.mcp.form.transport_stdio"):get() },
							}
						end,
					},
					ds.TextField {
						value = form_name,
						label = ui.t("settings.mcp.form.name"),
						placeholder = ui.t("settings.mcp.form.name_placeholder"),
						w_full = true,
					},
					-- Remote (HTTP / SSE) fields.
					ui.show(function(): boolean
						return form_transport:get() ~= "stdio"
					end, ui.column {
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
							value = form_auth_kind,
							label = ui.t("settings.mcp.form.auth_kind"),
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
						-- Bearer token input.
						ui.show(function(): boolean
							return form_auth_kind:get() == "bearer"
						end, ds.TextField {
							value = form_bearer,
							label = ui.t("settings.mcp.form.bearer_token"),
							placeholder = ui.t("settings.mcp.form.bearer_placeholder"),
							hint = function(): string
								if editing_id:get() ~= nil and editing_has_bearer:get() then
									return ui.t("settings.mcp.form.bearer_keep_hint"):get()
								end
								return ""
							end,
							password = true,
							w_full = true,
						}),
						-- Custom headers multi-line input.
						ui.show(function(): boolean
							return form_auth_kind:get() == "headers"
						end, ds.TextArea {
							value = form_headers,
							label = ui.t("settings.mcp.form.headers"),
							placeholder = "X-Api-Key: your-key",
							hint = function(): string
								local keys = editing_header_keys:get()
								if editing_id:get() ~= nil and #keys > 0 then
									return ui.t("settings.mcp.form.headers_keep_hint", { keys = table.concat(keys, ", ") }):get()
								end
								return ui.t("settings.mcp.form.headers_hint"):get()
							end,
							rows = 3,
							max_rows = 6,
							mono = true,
							w_full = true,
						}),
						-- Collapsible Advanced OAuth overrides.
						ui.show(function(): boolean
							return form_auth_kind:get() == "oauth"
						end, ui.column {
							w_full = true,
							gap = "$space.2",
							ds.Button {
								tone = "ghost",
								size = "sm",
								self = "start",
								on_click = function()
									form_oauth_advanced:toggle()
								end,
								ds.Icon {
									name = function(): ds_icon.Name
										return if form_oauth_advanced:get() then "alt-arrow-down" else "alt-arrow-right"
									end,
									size = 14,
								},
								ui.t("settings.mcp.form.oauth_advanced"),
							},
							ui.show(form_oauth_advanced, ui.column {
								w_full = true,
								gap = "$space.3",
								pl = "$space.3",
								ui.text {
									ui.t("settings.mcp.form.oauth_advanced_hint"),
									text_size = "$font.size.sm",
									line_height = "$font.line.sm",
									color = "$text.tertiary",
									wrap = true,
								},
								ds.TextField {
									value = form_client_id,
									label = ui.t("settings.mcp.form.oauth_client_id"),
									placeholder = ui.t("settings.mcp.form.oauth_client_id_placeholder"),
									w_full = true,
								},
								ds.TextField {
									value = form_client_secret,
									label = ui.t("settings.mcp.form.oauth_client_secret"),
									placeholder = ui.t("settings.mcp.form.oauth_client_secret_placeholder"),
									password = true,
									w_full = true,
								},
								ds.TextField {
									value = form_scopes,
									label = ui.t("settings.mcp.form.oauth_scopes"),
									placeholder = ui.t("settings.mcp.form.oauth_scopes_placeholder"),
									w_full = true,
								},
							}),
						}),
						-- Vault local-only override when signed in with Vault available.
						ui.show(function(): boolean
							return mcp.now:get().vaultAvailable
						end, ui.row {
							w_full = true,
							items = "center",
							gap = "$space.3",
							pt = "$space.1",
							ui.column {
								grow = 1,
								min_w = 0,
								gap = "$zeron.space.hair",
								ui.text {
									ui.t("settings.mcp.form.local_only"),
									text_size = "$font.size.md",
									line_height = "$font.line.md",
									weight = "$font.weight.medium",
									color = "$text.primary",
									wrap = true,
								},
								ui.text {
									ui.t("settings.mcp.form.local_only_description"),
									text_size = "$font.size.sm",
									line_height = "$font.line.sm",
									color = "$text.tertiary",
									wrap = true,
								},
							},
							ds.Switch {
								value = form_local_only,
								label = ui.t("settings.mcp.form.local_only"),
							},
						}),
					}),
					-- Local (stdio) fields.
					ui.show(function(): boolean
						return form_transport:get() == "stdio"
					end, ui.column {
						w_full = true,
						gap = "$space.3",
						ds.TextField {
							value = form_command,
							label = ui.t("settings.mcp.form.command"),
							placeholder = "npx",
							hint = ui.t("settings.mcp.form.command_hint"),
							w_full = true,
						},
						ds.TextField {
							value = form_args,
							label = ui.t("settings.mcp.form.args"),
							placeholder = "-y @modelcontextprotocol/server-filesystem /path",
							hint = ui.t("settings.mcp.form.args_hint"),
							w_full = true,
						},
						ds.TextArea {
							value = form_env,
							label = ui.t("settings.mcp.form.env"),
							placeholder = "API_KEY=your-secret",
							hint = function(): string
								local keys = editing_env_keys:get()
								if editing_id:get() ~= nil and #keys > 0 then
									return ui.t("settings.mcp.form.env_keep_hint", { keys = table.concat(keys, ", ") }):get()
								end
								return ui.t("settings.mcp.form.env_hint"):get()
							end,
							rows = 3,
							max_rows = 6,
							mono = true,
							w_full = true,
						},
						ui.row {
							w_full = true,
							items = "center",
							gap = "$space.2",
							color = "$text.tertiary",
							ds.Icon { name = "laptop", size = 14 },
							ui.text {
								ui.t("settings.mcp.form.stdio_local_note"),
								grow = 1,
								min_w = 0,
								text_size = "$font.size.sm",
								line_height = "$font.line.sm",
								color = "$text.tertiary",
								wrap = true,
							},
						},
					}),
					-- Inline validation / save error.
					ui.show(function(): boolean
						return form_error:get() ~= nil
					end, ui.text {
						function(): string
							return form_error:get() or ""
						end,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$zeron.tone.danger.ink",
						wrap = true,
					}),
					-- Form action buttons.
					ui.row {
						w_full = true,
						justify = "end",
						items = "center",
						gap = "$space.2",
						pt = "$space.2",
						ds.Button {
							ui.t("common.cancel"),
							tone = "ghost",
							size = "sm",
							disabled = form_saving,
							on_click = function()
								form_open:set(false)
							end,
						},
						ds.Button {
							ui.t("settings.mcp.form.save"),
							tone = "secondary",
							size = "sm",
							busy = form_saving,
							on_click = function()
								submit_form(false)
							end,
						},
						ui.show(function(): boolean
							return editing_id:get() == nil and form_transport:get() ~= "stdio" and form_auth_kind:get() == "oauth"
						end, ds.Button {
							ui.t("settings.mcp.form.save_and_sign_in"),
							tone = "primary",
							size = "sm",
							busy = form_saving,
							on_click = function()
								submit_form(true)
							end,
						}),
					},
				},
			},
		}
		)
	end)

	-- 3. Configured MCP Servers section.
	local function configured_row(initial: mcp.Server, _: number, server_state: State<mcp.Server>): Node
		local enabled_state = ui.use_state(initial.enabled)
		ui.effect(function()
			local current = server_state:get()
			if enabled_state:peek() ~= current.enabled then
				enabled_state:set(current.enabled)
			end
		end)

		local function current_flow(): mcp.AuthFlow?
			return mcp.auth_flows:get()[server_state:get().id]
		end

		local function current_status(): string
			return effective_status(server_state:get(), current_flow())
		end

		local function current_error(): string?
			local flow = current_flow()
			if flow and flow.phase == "error" and flow.error and flow.error ~= "" then
				return flow.error
			end
			local s = server_state:get()
			if s.lastError and s.lastError ~= "" then
				return s.lastError
			end
			return nil
		end

		return ui.column {
			w_full = true,
			px = "$space.4",
			py = "$space.3",
			gap = "$space.2",
			-- Top row: server identity + badges + enable switch.
			ui.row {
				w_full = true,
				items = "center",
				gap = "$space.3",
				flex_wrap = true,
				ui.column {
					grow = 1,
					min_w = 0,
					gap = "$zeron.space.hair",
					ui.row {
						gap = "$space.2",
						items = "center",
						flex_wrap = true,
						ui.text {
							function(): string
								return server_state:get().name
							end,
							text_size = "$font.size.md",
							line_height = "$font.line.md",
							weight = "$font.weight.medium",
							color = "$text.primary",
							wrap = true,
						},
						ui.presence(function(): string
							return server_state:get().storage or "local"
						end, storage_badge),
						ui.presence(current_status, status_badge),
					},
					ui.text {
						function(): string
							return server_subtitle(server_state:get())
						end,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.tertiary",
						wrap = true,
					},
				},
				ui.row {
					shrink = 0,
					items = "center",
					gap = "$space.2",
					-- OAuth primary action: Sign in / Reconnect / Cancel / Disconnect.
					ui.presence(function(): string
						local s = server_state:get()
						if s.authKind ~= "oauth" or not s.enabled then
							return "none"
						end
						local st = current_status()
						if st == "authorizing" then
							return "cancel"
						elseif st == "connected" then
							return "disconnect"
						elseif st == "reauthRequired" or st == "error" then
							return "reconnect"
						end
						return "sign_in"
					end, function(kind: string): Node
						if kind == "cancel" then
							return ds.Button {
								ui.t("settings.mcp.action.cancel"),
								tone = "secondary",
								size = "xs",
								on_click = function()
									local s = server_state:peek()
									local flow = mcp.auth_flows:peek()[s.id]
									mcp.cancel_auth(s.id, if flow then flow.flowId else nil):after(function() end, show_error)
								end,
							}
						elseif kind == "disconnect" then
							return ds.Button {
								ui.t("settings.mcp.action.disconnect"),
								tone = "ghost",
								size = "xs",
								on_click = function()
									local s = server_state:peek()
									mcp.disconnect_auth(s.id):after(function()
										toast.show {
											title = ui.t("settings.mcp.toast.disconnected", { name = s.name }),
											tone = "neutral",
										}
									end, show_error)
								end,
							}
						elseif kind == "reconnect" then
							return ds.Button {
								ui.t("settings.mcp.action.reconnect"),
								tone = "secondary",
								size = "xs",
								on_click = function()
									local s = server_state:peek()
									begin_oauth(s.id, s.name, nil)
								end,
							}
						elseif kind == "sign_in" then
							return ds.Button {
								ui.t("settings.mcp.action.sign_in"),
								tone = "primary",
								size = "xs",
								on_click = function()
									local s = server_state:peek()
									begin_oauth(s.id, s.name, nil)
								end,
							}
						end
						return ui.div {}
					end),
					ds.IconButton {
						icon = "pen",
						label = ui.t("common.edit"),
						size = "sm",
						tone = "quiet",
						on_click = function()
							open_edit_form(server_state:peek())
						end,
					},
					ds.IconButton {
						icon = "trash-bin-minimalistic",
						label = ui.t("common.remove"),
						size = "sm",
						tone = "danger",
						on_click = function()
							local s = server_state:peek()
							remove_target:set({ id = s.id, name = s.name })
							remove_open:set(true)
						end,
					},
					ds.Switch {
						value = enabled_state,
						label = ui.t("settings.mcp.action.enable_label", {
							name = function(): string
								return server_state:get().name
							end,
						}),
						on_change = function(on: boolean)
							local s = server_state:peek()
							mcp.set_enabled(s.id, on):after(function() end, function(err: any)
								enabled_state:set(not on)
								show_error(err)
							end)
						end,
					},
				},
			},
			-- Optional account label.
			ui.show(function(): boolean
				local label = server_state:get().accountLabel
				return label ~= nil and label ~= ""
			end, function(): Node
				return ui.text {
					function(): string
						local label = server_state:get().accountLabel or ""
						return ui.t("settings.mcp.account_label", { account = label }):get()
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
					wrap = true,
				}
			end),
			-- Optional error message.
			ui.show(function(): boolean
				return current_error() ~= nil
			end, function(): Node
				return ui.text {
					function(): string
						return current_error() or ""
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$zeron.tone.danger.ink",
					wrap = true,
				}
			end),
		}
	end

	local configured_section = search.anchor(
		"configured",
		ds.Settings.Section {
			title = ui.t("settings.mcp.configured.title"),
			description = ui.t("settings.mcp.configured.description"),
			ui.show(function(): boolean
				return #servers_list:get() == 0
			end, ds.Settings.Group {
				ds.EmptyState {
					icon = "remote-server",
					size = "sm",
					title = ui.t("settings.mcp.configured.empty_title"),
					body = ui.t("settings.mcp.configured.empty_body"),
					action = {
						label = ui.t("settings.mcp.add_custom"),
						icon = "plus",
						on_click = open_add_form,
					},
				},
			}),
			ds.Settings.Each(servers_list, configured_row, {
				key = function(s: mcp.Server): string
					return s.id
				end,
			}),
		}
	)

	-- 4. Popular MCP Servers catalog section.
	local function preset_row(_: mcp.Preset, _: number, preset_state: State<mcp.Preset>): Node
		return ui.row {
			w_full = true,
			px = "$space.4",
			py = "$space.3",
			gap = "$space.3",
			items = "center",
			flex_wrap = true,
			ui.column {
				grow = 1,
				min_w = 0,
				gap = "$zeron.space.hair",
				ui.row {
					gap = "$space.2",
					items = "center",
					flex_wrap = true,
					ui.text {
						function(): string
							return preset_state:get().name
						end,
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						weight = "$font.weight.medium",
						color = "$text.primary",
					},
					ui.show(function(): boolean
						local cat = preset_state:get().category
						return cat ~= nil and cat ~= ""
					end, function(): Node
						return ds.Badge {
							function(): string
								return preset_state:get().category or ""
							end,
							tone = "neutral",
							shape = "pill",
						}
					end),
				},
				ui.text {
					function(): string
						return preset_state:get().description or ""
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
					wrap = true,
				},
			},
			ui.row {
				shrink = 0,
				items = "center",
				gap = "$space.2",
				ui.presence(function(): string
					local p = preset_state:get()
					local existing = server_for_preset(p)
					if not existing then
						return if p.authKind == "bearer" or p.authKind == "headers" then "configure" else "connect"
					end
					local st = effective_status(existing, mcp.auth_flows:get()[existing.id])
					if st == "connected" then
						return "connected"
					elseif st == "authorizing" then
						return "authorizing"
					elseif existing.authKind == "oauth" and existing.enabled and (st == "needsAuth" or st == "reauthRequired" or st == "error") then
						return "added_needs_auth"
					end
					return "added"
				end, function(state_kind: string): Node
					if state_kind == "connected" then
						return ds.Badge { ui.t("settings.mcp.status.connected"), tone = "success", dot = true, shape = "pill" }
					elseif state_kind == "authorizing" then
						return ui.row {
							gap = "$space.2",
							items = "center",
							ds.Badge { ui.t("settings.mcp.status.authorizing"), tone = "info", dot = true, shape = "pill" },
							ds.Button {
								ui.t("settings.mcp.action.cancel"),
								tone = "ghost",
								size = "xs",
								on_click = function()
									local existing = server_for_preset(preset_state:peek())
									if existing then
										local flow = mcp.auth_flows:peek()[existing.id]
										mcp.cancel_auth(existing.id, if flow then flow.flowId else nil):after(function() end, show_error)
									end
								end,
							},
						}
					elseif state_kind == "added_needs_auth" then
						return ui.row {
							gap = "$space.2",
							items = "center",
							ds.Badge { ui.t("settings.mcp.presets.added"), tone = "neutral", shape = "pill" },
							ds.Button {
								ui.t("settings.mcp.action.sign_in"),
								tone = "primary",
								size = "xs",
								on_click = function()
									local existing = server_for_preset(preset_state:peek())
									if existing then
										begin_oauth(existing.id, existing.name, nil)
									end
								end,
							},
						}
					elseif state_kind == "added" then
						return ds.Badge { ui.t("settings.mcp.presets.added"), tone = "neutral", shape = "pill" }
					elseif state_kind == "configure" then
						return ds.Button {
							ui.t("settings.mcp.presets.configure"),
							tone = "secondary",
							size = "sm",
							on_click = function()
								open_preset_form(preset_state:peek())
							end,
						}
					end
					return ds.Button {
						ui.t("settings.mcp.presets.connect"),
						tone = "secondary",
						size = "sm",
						busy = function(): boolean
							return preset_busy:get()[preset_state:get().id] == true
						end,
						on_click = function()
							connect_preset(preset_state:peek())
						end,
					}
				end),
			},
		}
	end

	local presets_section = search.anchor(
		"presets",
		ds.Settings.Section {
			title = ui.t("settings.mcp.presets.title"),
			description = ui.t("settings.mcp.presets.description"),
			ds.SearchField {
				value = preset_query,
				placeholder = ui.t("settings.mcp.presets.search"),
				results = function(): number
					return #filtered_presets:get()
				end,
				size = "sm",
				w_full = true,
			},
			ui.show(function(): boolean
				return #filtered_presets:get() == 0 and trim(preset_query:get()) ~= ""
			end, ds.Settings.Group {
				ds.EmptyState {
					icon = "search",
					size = "sm",
					title = ui.t("settings.mcp.presets.no_match_title", { q = preset_query }),
					body = ui.t("settings.mcp.presets.no_match_body"),
					action = {
						label = ui.t("settings.mcp.presets.clear_search"),
						on_click = function()
							preset_query:set("")
						end,
					},
				},
			}),
			ds.Settings.Each(filtered_presets, preset_row, {
				key = function(p: mcp.Preset): string
					return p.id
				end,
				wash = true,
			}),
		}
	)

	-- Remove confirmation dialog.
	local remove_dialog = ds.Dialog {
		open = remove_open,
		tone = "danger",
		title = function(): string
			local t = remove_target:get()
			return ui.t("settings.mcp.remove_dialog.title", { name = if t then t.name else "" }):get()
		end,
		body = ui.t("settings.mcp.remove_dialog.body"),
		hold = function(): boolean
			return remove_busy:get()
		end,
		confirm = {
			label = ui.t("common.remove"),
			keep_open = true,
			busy = function(): boolean
				return remove_busy:get()
			end,
			on_click = function()
				local target = remove_target:peek()
				if not target or remove_busy:peek() then
					return
				end
				remove_busy:set(true)
				mcp.delete(target.id):after(function()
					remove_busy:set(false)
					remove_open:set(false)
					if editing_id:peek() == target.id then
						form_open:set(false)
					end
					toast.show {
						title = ui.t("settings.mcp.toast.removed", { name = target.name }),
						tone = "neutral",
					}
				end, function(err: any)
					remove_busy:set(false)
					show_error(err)
				end)
			end,
		},
	}

	return ds.Settings.Page {
		title = ui.t("settings.mcp.title"),
		description = ui.t("settings.mcp.description"),
		trail = ui.row {
			gap = "$space.2",
			items = "center",
			ds.Button {
				tone = "ghost",
				size = "sm",
				busy = refreshing,
				on_click = function()
					if refreshing:peek() then
						return
					end
					refreshing:set(true)
					mcp.refresh():after(function()
						refreshing:set(false)
					end, function(err: any)
						refreshing:set(false)
						show_error(err)
					end)
				end,
				ds.Icon { name = "refresh", size = 14 },
				ui.t("settings.mcp.refresh"),
			},
			ds.Button {
				tone = "secondary",
				size = "sm",
				on_click = open_add_form,
				ds.Icon { name = "plus", size = 14 },
				ui.t("settings.mcp.add_custom"),
			},
		},
		storage_section,
		custom_form_section,
		configured_section,
		presets_section,
		remove_dialog,
	}
end

return M
