-- Settings → Account's signing keys: the keys your devices sign the plugins
-- and templates you make with (the engine's `PluginSigningKey`, through
-- src/settings.rs). A card of them, this Mac's first: the device's name, the
-- key's short fingerprint and the day it was added, and Revoke behind a
-- confirm. A revoked key stays, quietly, with the day it went. One added on
-- another device since this one last looked is "New" for the visit that
-- first shows it. None yet: the card says how one is made.
--
--   keys.section()     the section, for the Account page
--   keys.load(visit)   asks for them (`visit`: a visit's first, which lets
--                      the last visit's "New" go)

local host = require("@app/screens/settings/host")
local ds = require("@ds")
local locale = require("@ds/locale")
local toast = require("@ds/toast")
local store = require("@app/screens/settings/store")

local M = {}

export type Key = {
	key: string,
	fingerprint: string?,
	device_name: string?,
	device_id: string?,
	this_device: boolean?,
	-- Seconds.
	created_at: number?,
	revoked_at: number?,
}

M.keys = ui.state("settings_signing_keys", {} :: { Key })
-- "loading" | "ready" | "failed"
M.status = ui.state("settings_signing_keys_status", "loading")
-- Those to badge "New" this visit, by key.
M.fresh = ui.state("settings_signing_keys_fresh", {} :: { [string]: boolean })
-- Those already shown as new once, kept across launches.
local seen = ui.persist("account_seen_signing_keys", {} :: { [string]: boolean })

-- This Mac's first, then the live ones newest first, then the revoked.
local function ordered(list: { Key }): { Key }
	local out = table.clone(list)
	local function rank(k: Key): number
		return if k.this_device and not k.revoked_at then 0 elseif not k.revoked_at then 1 else 2
	end
	table.sort(out, function(a: Key, b: Key): boolean
		if rank(a) ~= rank(b) then
			return rank(a) < rank(b)
		end
		return (a.created_at or 0) > (b.created_at or 0)
	end)
	return out
end

-- Each load's answer counts only while it's the latest.
local asked = 0
function M.load(visit: boolean?)
	if visit then
		M.fresh:set({})
	end
	asked += 1
	local mine = asked
	if #M.keys:peek() == 0 then
		M.status:set("loading")
	end
	host.call("account.signing_keys"):after(function(answer: any)
		if mine ~= asked then
			return
		end
		local fresh: { [string]: boolean } = {}
		local was = seen:peek()
		for _, k in answer.new_keys or {} do
			if not was[k] then
				fresh[k] = true
				seen:set_key(k, true)
			end
		end
		ui.batch(function()
			-- (Still new from earlier this visit: kept until the next.)
			for k in M.fresh:peek() do
				fresh[k] = true
			end
			M.fresh:set(fresh)
			M.keys:set(ordered(answer.keys or {}))
			M.status:set("ready")
		end)
	end, function()
		if mine == asked then
			M.status:set("failed")
		end
	end)
end

-- Revoking: the key asked about, until it's answered or let go.
local asking = ui.state("settings_signing_key_revoke", "")
local confirming = ui.state("settings_signing_key_confirm", false)
local revoking = ui.state("settings_signing_key_revoking", false)

local function revoke()
	local key = asking:peek()
	if key == "" or revoking:peek() then
		return
	end
	revoking:set(true)
	host.call("account.revoke_key", key):after(function()
		ui.batch(function()
			revoking:set(false)
			confirming:set(false)
			asking:set("")
			local now = os.time()
			local list = table.clone(M.keys:peek())
			for i, k in list do
				if k.key == key then
					local r = table.clone(k)
					r.revoked_at = now
					list[i] = r
				end
			end
			M.keys:set(ordered(list))
		end)
		M.load()
	end, function(err: any)
		revoking:set(false)
		confirming:set(false)
		asking:set("")
		toast.show {
			title = ui.t("settings.profile.keys.revoke_failed"):get(),
			body = store.says(if type(err) == "table" or type(err) == "userdata" then err.message else tostring(err)),
			tone = "danger",
		}
	end)
end

-- A fingerprint's first four groups ("ab12 cd34 ef56 7890").
local function short(fingerprint: string?): string
	local groups = {}
	for g in string.gmatch(fingerprint or "", "%S+") do
		table.insert(groups, g)
		if #groups == 4 then
			break
		end
	end
	return table.concat(groups, " ")
end

local function row(_k: Key, _i: number, now: State<Key>): Node
	local function k(): Key
		return now:get()
	end
	local function revoked(): boolean
		return k().revoked_at ~= nil
	end
	return ui.div {
		w_full = true,
		-- (Revoked: there, quietly.)
		opacity = function(): number
			return if revoked() then 0.55 else 1
		end,
		ds.Settings.Row {
			icon = "shield-check",
			label = function(): string
				local x = k()
				if x.this_device then
					return ui.t("settings.profile.keys.this_mac"):get()
				end
				local name = x.device_name or ""
				return if name ~= "" and name ~= x.device_id then name else ui.t("settings.profile.keys.another_device"):get()
			end,
			badge = ui.show(function(): boolean
				return M.fresh:get()[k().key] == true and not revoked()
			end, ds.Badge { ui.t("settings.profile.keys.new") }),
			description = function(): string
				local x = k()
				if x.revoked_at then
					return ui.t("settings.profile.keys.revoked", { date = locale.date(x.revoked_at) }):get()
				end
				return ui.t("settings.profile.keys.added", { date = locale.date(x.created_at or 0) }):get()
			end,
			truncate = true,
			control = ui.row {
				gap = "$space.3",
				items = "center",
				ui.text {
					function(): string
						return short(k().fingerprint)
					end,
					font = "$font.mono",
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
				},
				ui.show(
					function(): boolean
						return not revoked()
					end,
					ds.Button {
						ui.t("settings.profile.keys.revoke"),
						tone = "ghost",
						size = "sm",
						on_click = function()
							asking:set(k().key)
							confirming:set(true)
						end,
					}
				),
			},
		},
	}
end

function M.section(): Node
	return ds.Settings.Section {
		title = ui.t("settings.profile.keys.label"),
		description = ui.t("settings.profile.keys.description"),
		ui.presence(function(): string
			local s = M.status:get()
			if #M.keys:get() > 0 then
				return "keys"
			end
			return if s == "ready" then "none" elseif s == "failed" then "failed" else "loading"
		end, function(state: string): Node
			if state == "keys" then
				return ds.Settings.Each(M.keys, row, {
					key = function(x: Key): string
						return x.key
					end,
					wash = true,
				})
			elseif state == "none" then
				return ds.Settings.Group {
					ds.Settings.Row {
						icon = "shield-check",
						label = ui.t("settings.profile.keys.none"),
						description = ui.t("settings.profile.keys.none_how"),
					},
				}
			elseif state == "failed" then
				return ds.Settings.Group {
					ds.Settings.Row {
						icon = "shield-check",
						label = ui.t("settings.profile.keys.failed"),
						control = ds.Button { ui.t("settings.profile.keys.retry"), tone = "secondary", size = "sm", on_click = function()
							M.load()
						end },
					},
				}
			end
			return ui.div {}
		end),
		ds.Dialog {
			open = confirming,
			title = ui.t("settings.profile.keys.revoke_title"),
			body = ui.t("settings.profile.keys.revoke_body"),
			tone = "danger",
			confirm = {
				label = ui.t("settings.profile.keys.revoke"),
				keep_open = true,
				busy = function(): boolean
					return revoking:get()
				end,
				on_click = revoke,
			},
		},
	}
end

return M
