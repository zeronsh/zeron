--!strict
-- A plugin's deploy keys, on its publishing page (screens/plugins/publish):
-- what a CI job (a GitHub Action) publishes its new versions with instead
-- of your session. A section with New key… on its title's line; each key a
-- row: its name on a line of its own (wrapping, as a row's name does), when
-- it was last used and when it ends under it (within 14 days of its end,
-- how soon, the line in the warning's ink), and at the row's end the key
-- shortened (`M.short`, as the store's site shows it) in the code face, a
-- column down the rows, then Revoke. Revoked keys aren't listed. A new one is made by the
-- engine (`CreateDeployKey`) after a fresh sign-in in the browser when the
-- store asks for one; its secret is shown here once, to copy, and kept
-- nowhere.
--
-- A key can only publish versions of this plugin, and every version still
-- waits for review: what it can't do is said where it's made.
--
--   keys.Section { plugin, here, published }  (`published`: whether a version
--                                             of it has been sent, a key's
--                                             first condition)

local ds = require("@ds")
local Markdown = require("@ds/markdown")
local locale = require("@ds/locale")
local store = require("@app/screens/plugins/store")

local M = {}

-- How long a new key lasts: never ending, or days.
local EXPIRY: { { value: string, days: number? } } = {
	{ value = "90", days = 90 },
	{ value = "30", days = 30 },
	{ value = "365", days = 365 },
	{ value = "never", days = nil },
}

local function day_of(at: any): string?
	return if type(at) == "number" and at > 0 then locale.date(at / 1000) else nil
end

-- How soon a key ends, in whole days from today (0: today), if it's within
-- `SOON` of it; nil otherwise. (The store's site says the same.)
local SOON = 14
local function ends_in(at: any): number?
	if type(at) ~= "number" or at <= 0 then
		return nil
	end
	local now = os.date("*t") :: any
	local today = os.time({ year = now.year, month = now.month, day = now.day, hour = 0 })
	local days = math.floor((at / 1000 - today) / 86400)
	return if days <= SOON then math.max(0, days) else nil
end

-- A key as it's shown everywhere it's shown short (the store's site too):
-- its first eight characters, "…", its last four; the whole in a tooltip.
function M.short(key: string): string
	if #key <= 14 then
		return key
	end
	return `{string.sub(key, 1, 8)}…{string.sub(key, -4)}`
end

M.Section = ui.component(function(props: { plugin: string, here: any, published: () -> boolean }): Node
	local id = props.plugin
	-- "loading", "ready", or why not.
	local phase = ui.use_state("loading")
	local list = ui.use_state({} :: { any })
	local making = ui.use_state(false)
	local name = ui.use_state("")
	local expiry = ui.use_state("90")
	-- Making one: "idle", "busy" (asked; perhaps signing in), or why not.
	local made_phase = ui.use_state("idle")
	local secret = ui.use_state(nil :: string?)
	local secret_open = ui.use_state(false)
	-- (Its secret held only while its dialog is up, however that closes.)
	ui.effect(function()
		if not secret_open:get() and secret:peek() ~= nil then
			secret:set(nil)
		end
	end)
	local revoking = ui.use_state(nil :: any)
	local revoke_open = ui.use_state(false)

	local function load()
		store.keys(id):after(function(r: any)
			local out = {}
			for _, k in (if type(r) == "table" and type(r.keys) == "table" then r.keys else {}) :: { any } do
				if type(k) == "table" and type(k.key) == "string" and k.revoked_at == nil then
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
	-- Gone from the page (another page, a link): a sign-in still waited for
	-- is given up, so no key is made whose secret nobody would see.
	ui.on_cleanup(function()
		if made_phase:peek() == "busy" then
			store.cancel_step_up()
		end
	end)

	local function create()
		local n = string.match(name:peek(), "^%s*(.-)%s*$") or ""
		if n == "" or made_phase:peek() == "busy" then
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
		store.create_key({ id = id, name = n, expires = expires }):after(function(r: any)
			made_phase:set("idle")
			making:set(false)
			name:set("")
			if type(r) == "table" and type(r.secret) == "string" and r.secret ~= "" then
				secret:set(r.secret)
				secret_open:set(true)
			end
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
				return if type(k.name) == "string" and k.name ~= "" then k.name else M.short(k.key)
			end,
			description = function(): string
				local k = now:get()
				local used = day_of(k.last_used_at)
				local ends = day_of(k.expires_at)
				-- About to end: how soon, after whether it's still in use (so
				-- whether it must be replaced first).
				local n = soon()
				local ending = if n == nil
					then (if ends then ui.t("plugins.store.key.ends", { date = ends }):get() else ui.t("plugins.store.key.no_end"):get())
					elseif n == 0 then ui.t("plugins.store.key.ends_today"):get()
					elseif n == 1 then ui.t("plugins.store.key.ends_tomorrow"):get()
					else ui.t("plugins.store.key.ends_in", { n = n }):get()
				return table.concat({
					if used then ui.t("plugins.store.key.last_used", { date = used }):get() else ui.t("plugins.store.key.not_used"):get(),
					ending,
				}, " · ")
			end,
			-- (About to end, the line is in the warning's ink, one status
			-- line; only its "Ends…" once the line can be a rich one:
			-- zgx-asks/rich-span-boundary-break.)
			description_ink = function(): string
				return if soon() then "$zeron.tone.warning.ink" else "$text.tertiary"
			end,
			-- At the row's end: the key, short, a column down the rows (it
			-- ends 12 before Revoke, which ends on the content edge), then
			-- Revoke, a row's action as the versions' are.
			control = ui.row {
				gap = "$space.3",
				items = "center",
				ui.text {
					function(): string
						return M.short(now:get().key)
					end,
					class = "key-short",
					-- (A key's short form as Settings shows a signing key's.)
					font = "$font.mono",
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
					-- (A row is its key's: the key never changes under it.)
					tooltip = first.key,
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
				return if props.published() and #list:get() > 0 then "list" else "none"
			end, function(shown: string): Node
				if shown == "list" then
					return ds.Settings.Each(list, key_row, {
						key = function(k: any): string
							return k.key
						end,
					})
				end
				-- None yet: why, in the card where they'd be.
				return ds.Settings.Group {
					ui.text {
						id = "keys-none",
						function(): string
							return if props.published() then ui.t("plugins.store.keys_none"):get() else ui.t("plugins.store.keys_publish_first"):get()
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
			-- versions after the first, which comes from this computer.)
			trail = ui.div {
				-- (Its words on the content edge past its own padding, as
				-- Settings' section actions.)
				mr = ui.raw(-((ui.theme.get("zeron.space.snug") :: number?) or 10), "the ghost's own padding"),
				ds.Button {
					id = "keys-new",
					tone = "ghost",
					size = "sm",
					disabled = function(): boolean
						return not props.published()
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
		-- A new key: its name and how long it lasts; then, perhaps, a fresh
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
					return (string.match(name:get(), "%S")) == nil
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
					return require("@app/screens/plugins/publish").failure(p)
				end,
				w_full = true,
				wrap = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$zeron.tone.danger.ink",
			}),
		},
		-- Its secret, once: copy it now.
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
			-- The secret as the app shows a line of code to copy: a chat
			-- reply's code block (mono, scrolling sideways, never broken, its
			-- own Copy), drawn by ds/markdown from one fence. (A deploy key's
			-- characters never close it.)
			ui.div {
				id = "key-secret",
				w_full = true,
				Markdown {
					text = function(): string
						return "```\n" .. (secret:get() or "") .. "\n```"
					end,
				},
			},
		},
		ds.Dialog {
			open = revoke_open,
			title = function(): string
				local k = revoking:get()
				return ui.t("plugins.store.key.revoke_title", { name = if k then (k.name or k.key) else "" }):get()
			end,
			body = ui.t("plugins.store.key.revoke_body"),
			tone = "danger",
			confirm = {
				label = ui.t("plugins.store.key.revoke"),
				on_click = function()
					local k = revoking:peek()
					revoking:set(nil)
					if k then
						store.revoke_key(id, k.key):after(function()
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
