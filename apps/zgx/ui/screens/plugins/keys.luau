--!strict
-- The account's deploy tokens, at the end of Yours: what a CI job (a
-- GitHub Action) publishes new versions with instead of your session, each
-- for the plugins it was made for (its scope: all of yours, those you
-- publish later too, or the ones chosen), fixed when it's made. A section
-- with New token… at its title's end; each token a row: its name on a line
-- of its own, when it was last used (and on which plugin) and when it ends
-- under it (within 14 days of its end, how soon, the line in the warning's
-- ink), its scope under that, and at the row's end its short id (the
-- store's, as its site shows it) in the code face, a column down the rows,
-- then Revoke. Revoked tokens aren't listed. A new one is made by the store
-- after a fresh sign-in in the browser begun for it; the token is shown
-- here once, to copy, and kept nowhere.
--
-- A token never publishes a plugin's first version: that comes from a
-- signed-in device.
--
--   keys.Section { plugins }  (`plugins`: yours in the store, { id, name },
--                             what a scope chooses from; none, no key yet)

local ds = require("@ds")
local field = require("@ds/field")
local locale = require("@ds/locale")
local auth = require("@account/auth")
local store = require("@app/screens/plugins/store")
local token_block = require("@app/screens/plugins/token_block")

local M = {}

-- A new token, shown once, and whether its dialog is up: the app's, not
-- the page's, so one made while Yours was left (another page, a link)
-- still shows as Yours comes back. Kept nowhere else, and let go of as its
-- dialog closes.
local made = ui.state("plugins_deploy_token_made", nil :: string?)
local made_open = ui.state("plugins_deploy_token_made_open", false)

-- How long a new token lasts: never ending, or days.
local EXPIRY: { { value: string, days: number? } } = {
	{ value = "90", days = 90 },
	{ value = "30", days = 30 },
	{ value = "365", days = 365 },
	{ value = "never", days = nil },
}

local function day_of(at: any): string?
	return if type(at) == "number" and at > 0 then locale.date(at / 1000) else nil
end

-- Whether a token has ended: the store lists one until it's revoked.
local function has_ended(at: any): boolean
	return type(at) == "number" and at > 0 and at <= os.time() * 1000
end

-- How soon a token ends, in whole days from today (0: today), if it's within
-- `SOON` of it; nil otherwise. (The store's site says the same.)
local SOON = 14
local function ends_in(at: any): number?
	if type(at) ~= "number" or at <= 0 then
		return nil
	end
	-- (Today's start as the clock now less the time since local midnight:
	-- zgx reads a date table as UTC while `os.date` says local time, so a
	-- table would put today a time zone off.)
	local t = os.date("*t") :: any
	local today = os.time() - (t.hour * 3600 + t.min * 60 + t.sec)
	local days = math.floor((at / 1000 - today) / 86400)
	return if days <= SOON then math.max(0, days) else nil
end

-- A token as it's shown everywhere (the store's site too): its short id,
-- the store's (the first eight characters after `zdt_`). The token itself
-- is never listed.
function M.short(token: any): string
	return if type(token) == "table" and type(token.short) == "string" then token.short else ""
end

export type Owned = { id: string, name: string }

-- A token's scope in words: all of yours, or the plugins it names (by name
-- where they're yours; two by name, more by how many).
-- Which of yours a token publishes, by name ("A, B and 2 more"; `whole`:
-- every one, "A, B, C and D"), or all of them.
local function scope_names(scope: any, plugins: { Owned }): { string }
	local names: { string } = {}
	for _, id in (if type(scope) == "table" and type(scope.plugins) == "table" then scope.plugins else {}) :: { any } do
		local name = id
		for _, p in plugins do
			if p.id == id then
				name = p.name
			end
		end
		table.insert(names, name)
	end
	return names
end

local function scope_words(scope: any, plugins: { Owned }, whole: boolean?): string
	if type(scope) ~= "table" or scope.all == true then
		return ui.t("plugins.store.key.scope.all_now", { n = #plugins }):get()
	end
	if whole then
		local names = scope_names(scope, plugins)
		if #names > 2 then
			local last = table.remove(names) :: string
			return ui.t("plugins.store.key.scope.every", { list = table.concat(names, ui.t("plugins.words.list_sep"):get()), last = last }):get()
		end
	end
	local names: { string } = {}
	for _, id in (if type(scope.plugins) == "table" then scope.plugins else {}) :: { any } do
		local name = id
		for _, p in plugins do
			if p.id == id then
				name = p.name
			end
		end
		table.insert(names, name)
	end
	if #names == 1 then
		return ui.t("plugins.store.key.scope.one", { name = names[1] }):get()
	elseif #names == 2 then
		return ui.t("plugins.store.key.scope.two", { a = names[1], b = names[2] }):get()
	end
	return ui.t("plugins.store.key.scope.more", { a = names[1], b = names[2], n = #names - 2 }):get()
end

M.Section = ui.component(function(props: { plugins: () -> { Owned } }): Node
	local function published(): boolean
		return #props.plugins() > 0
	end
	-- "loading", "ready", or why not.
	local phase = ui.use_state("loading")
	-- The tokens whose every plugin is shown by name, in place.
	local whole_scope = ui.use_state({} :: { [string]: boolean })
	local list = ui.use_state({} :: { any })
	local making = ui.use_state(false)
	local name = ui.use_state("")
	local expiry = ui.use_state("90")
	-- Its scope: "all" of yours, or "chosen", those ticked.
	local scope_kind = ui.use_state("all")
	local chosen = ui.use_state({} :: { [string]: boolean })
	-- Making one: "idle", "busy" (asked; perhaps signing in), or why not.
	local made_phase = ui.use_state("idle")
	local secret = made
	local secret_open = made_open
	-- Copied: its line under the block, while the dialog is up.
	local copied_once = ui.use_state(false)
	-- (Its secret held only while its dialog is up, however that closes.)
	ui.effect(function()
		if not secret_open:get() and secret:peek() ~= nil then
			secret:set(nil)
			copied_once:set(false)
		end
	end)
	local revoking = ui.use_state(nil :: any)
	local revoke_open = ui.use_state(false)

	local function load()
		store.tokens():after(function(r: any)
			local out = {}
			for _, k in (if type(r) == "table" and type(r.tokens) == "table" then r.tokens else {}) :: { any } do
				if type(k) == "table" and type(k.id) == "string" and k.revoked_at == nil then
					table.insert(out, k)
				end
			end
			list:set(out)
			phase:set("ready")
		end, function(err: any)
			local m = if type(err) == "table" then err.message else err
			phase:set(if type(m) == "string" and m ~= "" then m else "failed")
		end)
	end
	ui.on_mount(load)
	-- Another account signed in while Yours stays open: its own tokens, not
	-- the last one's.
	local account: string? = nil
	local first = true
	ui.effect(function()
		local user: any = auth.user:get()
		local id = if type(user) == "table" and type(user.id) == "string" then user.id else nil
		if not first and id ~= account then
			list:set({})
			phase:set("loading")
			load()
		end
		first = false
		account = id
	end)
	-- Gone from the page (another page, a link): a sign-in still waited for
	-- is given up, so no token is made that nobody would see.
	ui.on_cleanup(function()
		if made_phase:peek() == "busy" then
			store.cancel_step_up()
		end
	end)

	local function chosen_ids(): { string }
		local out: { string } = {}
		local picked = chosen:get()
		for _, p in props.plugins() do
			if picked[p.id] then
				table.insert(out, p.id)
			end
		end
		return out
	end
	local function ready_to_make(): boolean
		return (string.match(name:get(), "%S")) ~= nil and (scope_kind:get() == "all" or #chosen_ids() > 0)
	end
	local function create()
		local n = string.match(name:peek(), "^%s*(.-)%s*$") or ""
		if n == "" or made_phase:peek() == "busy" or not ready_to_make() then
			return
		end
		local days: number? = nil
		for _, e in EXPIRY do
			if e.value == expiry:peek() then
				days = e.days
			end
		end
		made_phase:set("busy")
		local expires = if days then (os.time() + days * 86400) * 1000 else nil
		local scope: store.Scope = if scope_kind:peek() == "all" then { all = true } else { plugins = chosen_ids() }
		store.create_token({ name = n, scope = scope, expires = expires }):after(function(r: any)
			-- (`secret`: the token, `zdt_…`, this once; `token`, its record.
			-- None when Zeron's own window made it, and shows it there: once
			-- a template that isn't Zeron's has run, src/plugins/act_consent.)
			-- Kept first, the app's: shown as Yours comes back if it was
			-- left meanwhile.)
			if type(r) == "table" and type(r.secret) == "string" and r.secret ~= "" then
				secret:set(r.secret)
				secret_open:set(true)
			end
			made_phase:set("idle")
			making:set(false)
			name:set("")
			scope_kind:set("all")
			chosen:set({})
			load()
		end, function(err: any)
			local m = if type(err) == "table" then err.message else err
			made_phase:set(if m == "cancelled" then "idle" elseif type(m) == "string" and m ~= "" then m else "failed")
		end)
	end

	local function key_row(first: any, _i: number, now: State<any>): Node
		local function soon(): number?
			return ends_in(now:get().expires_at)
		end
		return ds.Settings.Row {
			label = function(): string
				local k = now:get()
				return if type(k.name) == "string" and k.name ~= "" then k.name else M.short(k)
			end,
			description = function(): string
				local k = now:get()
				local used = day_of(k.last_used_at)
				-- (Where it was last used, by the plugin's name where it's yours.)
				local on: string? = nil
				if type(k.last_used_plugin) == "string" and k.last_used_plugin ~= "" then
					on = k.last_used_plugin
					for _, p in props.plugins() do
						if p.id == on then
							on = p.name
						end
					end
				end
				local ends = day_of(k.expires_at)
				-- About to end: how soon, after whether it's still in use (so
				-- whether it must be replaced first).
				local n = soon()
				-- (Ended, and listed until it's revoked: when.)
				local ending = if has_ended(k.expires_at) and ends
					then ui.t("plugins.store.key.ended", { date = ends }):get()
					elseif n == nil
					then (if ends then ui.t("plugins.store.key.ends", { date = ends }):get() else ui.t("plugins.store.key.no_end"):get())
					elseif n == 0 then ui.t("plugins.store.key.ends_today"):get()
					elseif n == 1 then ui.t("plugins.store.key.ends_tomorrow"):get()
					else ui.t("plugins.store.key.ends_in", { n = n }):get()
				return table.concat({
					if used and on then ui.t("plugins.store.key.last_used_on", { date = used, plugin = on }):get()
						elseif used then ui.t("plugins.store.key.last_used", { date = used }):get()
						elseif day_of(k.created_at) then ui.t("plugins.store.key.not_used_made", { date = day_of(k.created_at) }):get()
						else ui.t("plugins.store.key.not_used"):get(),
					ending,
				}, " · ")
			end,
			-- Which of yours it publishes, wrapping, never cut: past two, by
			-- the first two and how many more, and every one in place with
			-- the line's own link (a key's plugins are fixed once it's made).
			detail = function(): string
				return scope_words(now:get().scope, props.plugins(), whole_scope:get()[first.id] == true)
			end,
			detail_link = if type(first.scope) == "table" and first.scope.all ~= true and type(first.scope.plugins) == "table" and #first.scope.plugins > 2
				then {
					-- (Acting in place: a button.)
					role = "button",
					label = function(): string
						return if whole_scope:get()[first.id] then ui.t("plugins.store.key.scope.fewer"):get() else ui.t("plugins.store.key.scope.show_all"):get()
					end,
					on_click = function()
						local now_open = table.clone(whole_scope:peek())
						now_open[first.id] = if now_open[first.id] then nil else true
						whole_scope:set(now_open)
					end,
				}
				else nil,
			-- (About to end, the line is in the warning's ink, one status
			-- line; only its "Ends…" once the line can be a rich one:
			-- zgx-asks/rich-span-boundary-break.)
			description_ink = function(): string
				return if soon() then "$zeron.tone.warning.ink" else "$text.tertiary"
			end,
			-- At the row's end: the token's short id, a column down the rows
			-- (it ends 12 before Revoke, which ends on the content edge), then
			-- Revoke, a row's action as the versions' are.
			control = ui.row {
				gap = "$space.3",
				items = "center",
				ui.text {
					function(): string
						return M.short(now:get())
					end,
					class = "key-short",
					-- (A token's short id as Settings shows a signing key's.)
					font = "$font.mono",
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
				},
				ds.Button {
					tone = "secondary",
					size = "sm",
					on_click = function()
						revoking:set(now:peek())
						revoke_open:set(true)
					end,
					ui.t("plugins.store.key.revoke"),
				},
			},
		}
	end

	local function body(key: string): Node
		if key == "ready" then
			-- (Nothing sent yet: what comes first, whatever's listed.)
			return ui.presence(function(): string
				return if published() and #list:get() > 0 then "list" else "none"
			end, function(shown: string): Node
				if shown == "list" then
					return ds.Settings.Each(list, key_row, {
						key = function(k: any): string
							return k.id
						end,
					})
				end
				-- None yet: why, in the card where they'd be.
				return ds.Settings.Group {
					ui.text {
						id = "keys-none",
						function(): string
							return if published() then ui.t("plugins.store.keys_none"):get() else ui.t("plugins.store.keys_publish_first"):get()
						end,
						w_full = true,
						px = "$space.4",
						py = "$space.3",
						wrap = true,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.secondary",
					},
				}
			end, { w_full = true })
		elseif key == "loading" then
			return ds.SkeletonWaiting {
				build = function(still: boolean): Node
					return ds.Settings.Group { ds.SkeletonRow { lead = 0, title = 120, line = 220, trail = 60, still = still } }
				end,
			}
		end
		local publish = require("@app/screens/plugins/publish")
		local function again()
			phase:set("loading")
			load()
		end
		if store.signed_out(key) then
			return publish.sign_in_callout(ui.t("plugins.store.sign_in_to_see_keys"), again)
		end
		return ds.Callout {
			tone = "neutral",
			title = ui.t("plugins.store.keys_failed"),
			body = publish.failure(key),
			action = { label = ui.t("plugins.store.try_again"), on_click = again },
		}
	end

	local expiry_options = {}
	for _, e in EXPIRY do
		table.insert(expiry_options, { value = e.value, label = ui.t(`plugins.store.key.expiry.{e.value}`) })
	end

	return ui.column {
		w_full = true,
		ds.Settings.Section {
			title = ui.t("plugins.store.keys"),
			description = ui.t("plugins.store.keys_body"),
			-- (The section's own action, at its title's end; a key sends the
			-- versions after a plugin's first, which comes from a device.)
			trail = ui.div {
				-- (Its words on the content edge past its own padding, as
				-- Settings' section actions.)
				mr = ui.raw(-((ui.theme.get("zeron.space.snug") :: number?) or 10), "the ghost's own padding"),
				ds.Button {
					id = "keys-new",
					tone = "ghost",
					size = "sm",
					disabled = function(): boolean
						return not published()
					end,
					on_click = function()
						made_phase:set("idle")
						making:set(true)
					end,
					ui.t("plugins.store.key.new"),
				},
			},
			ui.presence(function(): string
				return phase:get()
			end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		},
		-- A new token: its name and how long it lasts; then, perhaps, a fresh
		-- sign-in in the browser, waited for here.
		ds.Dialog {
			open = making,
			title = ui.t("plugins.store.key.new_title"),
			body = ui.t("plugins.store.key.new_body"),
			size = "sm",
			hold = function(): boolean
				return made_phase:get() == "busy"
			end,
			confirm = {
				label = ui.t("plugins.store.key.create"),
				keep_open = true,
				busy = function(): boolean
					return made_phase:get() == "busy"
				end,
				disabled = function(): boolean
					return not ready_to_make()
				end,
				on_click = create,
			},
			ds.TextField {
				value = name,
				label = ui.t("plugins.store.key.name"),
				placeholder = ui.t("plugins.store.key.name_placeholder"),
				select_on_focus = true,
				w_full = true,
			},
			ds.Select { value = expiry, label = ui.t("plugins.store.key.expiry_label"), options = expiry_options, w_full = true },
			-- What it publishes: all of yours (those you publish later too),
			-- or the ones ticked. Fixed once it's made. Labelled as the
			-- Name and Ends fields above it are (ds/field).
			field.wrap {
				label = ui.t("plugins.store.key.scope.label"),
				w_full = true,
				box = ui.column {
					id = "key-scope",
					w_full = true,
					gap = "$space.2",
					ds.Segmented {
						value = scope_kind,
						label = ui.t("plugins.store.key.scope.label"),
						options = {
							{ value = "all", label = ui.t("plugins.store.key.scope.all") },
							{ value = "chosen", label = ui.t("plugins.store.key.scope.chosen") },
						},
					},
					ui.show(function(): boolean
						return scope_kind:get() == "chosen"
					end, ui.column {
						w_full = true,
						gap = "$space.1",
						ui.each(ui.computed(function(): { Owned }
							return props.plugins()
						end), function(p: Owned): Node
							local on = ui.computed(function(): boolean
								return chosen:get()[p.id] == true
							end)
							return ds.Checkbox {
								value = function(): boolean
									return on:get()
								end,
								on_change = function(v: boolean)
									local next = table.clone(chosen:peek())
									next[p.id] = if v then true else nil
									chosen:set(next)
								end,
								label = p.name,
							}
						end, {
							key = function(p: Owned): string
								return p.id
							end,
						}),
					}),
				},
			},
			ui.show(function(): boolean
				return made_phase:get() == "busy"
			end, ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.text {
					ui.t("plugins.store.key.signing_in"),
					grow = 1,
					min_w = 0,
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
				},
				ds.Button {
					tone = "ghost",
					size = "sm",
					on_click = function()
						store.cancel_step_up()
					end,
					ui.t("plugins.store.key.stop_waiting"),
				},
			}),
			ui.show(function(): boolean
				local p = made_phase:get()
				return p ~= "idle" and p ~= "busy"
			end, ui.text {
				function(): string
					local p = made_phase:get()
					if p == "timed_out" then
						return ui.t("plugins.store.key.timed_out"):get()
					end
					-- (Made too many today: the store's limit, said plainly.)
					if p == "rate_limited" or string.match(p, "^rate_limited: ") then
						return ui.t("plugins.store.key.made_today"):get()
					end
					return require("@app/screens/plugins/publish").failure(p)
				end,
				w_full = true,
				wrap = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$zeron.tone.danger.ink",
			}),
		},
		-- The token, once: copy it now.
		ds.Dialog {
			open = secret_open,
			title = ui.t("plugins.store.key.secret_title"),
			body = ui.t("plugins.store.key.secret_body"),
			cancel = false,
			dismissable = false,
			-- (Nothing closes it but its button: Escape doesn't either.)
			hold = function(): boolean
				return secret:get() ~= nil
			end,
			confirm = {
				label = ui.t("plugins.store.key.done"),
				on_click = function()
					secret:set(nil)
					secret_open:set(false)
				end,
			},
			-- The token, to copy (screens/plugins/token_block).
			token_block.Block {
				secret = function(): string?
					return secret:get()
				end,
				copy = function(token: string): any
					return store.copy_secret(token)
				end,
				copied = copied_once,
			},
		},
		ds.Dialog {
			open = revoke_open,
			title = function(): string
				local k = revoking:get()
				return ui.t("plugins.store.key.revoke_title", { name = if k then (k.name or M.short(k)) else "" }):get()
			end,
			body = ui.t("plugins.store.key.revoke_body"),
			tone = "danger",
			confirm = {
				label = ui.t("plugins.store.key.revoke"),
				on_click = function()
					local k = revoking:peek()
					revoking:set(nil)
					if k then
						store.revoke_token(k.id):after(function()
							load()
						end, function(err: any)
							local m = if type(err) == "table" then err.message else err
							ds.toast.show {
								title = ui.t("plugins.store.key.revoke_failed"),
								body = require("@app/screens/plugins/publish").failure(m),
								tone = "warning",
							}
						end)
					end
				end,
			},
		},
	}
end)

return M
