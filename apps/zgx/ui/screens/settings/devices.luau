-- Settings → Devices: the computers your sessions sync across, the
-- shipping app's Devices (crates/ui settings/devices.rs): this one first,
-- each with its system, the Zeron it runs, and whether it's there now or
-- when it last was; and renaming one, which every device then shows. Cloud's
-- machines aren't listed (they come and go with their sessions).
--
-- The list is the engine's (account/sessions `now.devices`), a keyed
-- `Settings.Each`, so a device coming online, renamed, or arriving changes
-- its row alone.

local ds = require("@ds")
local toast = require("@ds/toast")
local status = require("@ds/status")
local sessions = require("@account/sessions")
local search = require("@app/screens/settings/search")

local M = {}

M.ID = "devices"
M.TITLE = "Devices"

search.index(M.ID, ui.t("settings.devices.title"), ui.t("settings.devices.terms"), {
	{ key = "devices", title = ui.t("settings.devices.yours.label"), terms = ui.t("settings.devices.yours.terms") },
	{ key = "devices", title = ui.t("settings.devices.rename_one.label"), terms = ui.t("settings.devices.rename_one.terms") },
})

local PLATFORMS: { [string]: string } = {
	macos = "macOS",
	linux = "Linux",
	windows = "Windows",
	ios = "iOS",
	android = "Android",
}

-- What the line under a device's name says: its system, its Zeron, and
-- whether it's there (in the locale shown: read where it's drawn).
function M.describe(d: sessions.Device, now: number): string
	local parts = { PLATFORMS[d.platform] or d.platform }
	if d.version then
		table.insert(parts, `v{d.version}`)
	end
	-- (This device says so in its badge, as the gallery's device rows do.)
	if d.here then
		return table.concat(parts, " · ")
	elseif d.online then
		table.insert(parts, ui.t("settings.devices.online"):get())
	elseif d.seen then
		table.insert(parts, ui.t("settings.devices.last_seen", { ago = status.ago(math.max(0, now - d.seen)) }):get())
	else
		table.insert(parts, ui.t("settings.devices.offline"):get())
	end
	return table.concat(parts, " · ")
end

-- Renaming: which device (its id; "" none), and the name being typed.
local renaming = ui.state("settings_device_renaming", "")
local renaming_open = ui.state("settings_device_renaming_open", false)
local new_name = ui.state("settings_device_name", "")

local function device(id: string): sessions.Device?
	for _, d in sessions.now:get().devices or {} do
		if d.id == id then
			return d
		end
	end
	return nil
end

local function rename()
	local id = renaming:peek()
	local name = string.match(new_name:peek(), "^%s*(.-)%s*$") or ""
	if id == "" or name == "" then
		return
	end
	sessions.rename_device(id, name):after(function() end, function(err: any)
		toast.show {
			title = ui.t("settings.devices.couldnt_rename"):get(),
			body = if err.message == "offline" then ui.t("settings.engine.offline"):get() else ui.t("settings.devices.try_later"):get(),
			tone = "danger",
		}
	end)
end

-- A device's glyph, by its system (Settings' pages draw devices with it).
function M.icon(platform: string): any
	return if platform == "linux" then "remote-server" elseif platform == "ios" or platform == "android" then "smartphone" else "laptop"
end
local icon_of = M.icon

-- Which computer a per-device page shows (Providers, Plugins): a choice at
-- its title's end once there's more than this one, this one first, one
-- that's away saying so. `shown` is the device shown (nil: this one);
-- choosing one calls `choose` with its id. Built once: its options follow
-- the devices as they come, go and are renamed, and the choice follows
-- `shown`.
function M.picker(shown: () -> string?, choose: (id: string) -> ()): Node
	local function computers(): { sessions.Device }
		return sessions.computers(sessions.now:get().devices, true)
	end
	local function here(): string
		for _, d in sessions.now:get().devices or {} do
			if d.here then
				return d.id
			end
		end
		return ""
	end
	return ui.show(
		function(): boolean
			return #computers() > 1
		end,
		ui.component(function(): Node
			local chosen = ui.use_state(shown() or here())
			ui.effect(function()
				local id = shown() or here()
				if id ~= chosen:peek() then
					chosen:set(id)
				end
			end)
			return ds.Select {
				value = chosen,
				name = ui.t("settings.providers.device"),
				size = "sm",
				w = 200,
				options = function(): { any }
					local options = {}
					for _, d in computers() do
						table.insert(options, {
							value = d.id,
							label = d.name,
							icon = icon_of(d.platform),
							description = if not d.here and d.online == false then ui.t("settings.devices.offline") else nil,
						})
					end
					return options
				end,
				on_change = choose,
			}
		end)({})
	)
end

function M.view(): Node
	local list = ui.computed(function(): { sessions.Device }
		return sessions.computers(sessions.now:get().devices)
	end)
	-- (Minutes tick the "last seen" lines over, once a minute.)
	local clock = ui.use_state(os.time())
	ui.every(60000, function()
		clock:set(os.time())
		return true
	end)
	return ui.column {
		w_full = true,
		items = "center",
		ds.Settings.Page {
			title = ui.t("settings.devices.title"),
			description = ui.t("settings.devices.description"),
			search.anchor(
				"devices",
				ds.Settings.Section {
					ui.show(function(): boolean
						return #list:get() == 0
					end, ds.EmptyState {
						icon = "laptop",
						title = ui.t("settings.devices.empty_title"),
						body = ui.t("settings.devices.empty_body"),
						size = "sm",
					}),
					ds.Settings.Each(list, function(_d: sessions.Device, _i: number, d: State<sessions.Device>): Node
						return ds.Settings.Row {
							label = function(): string
								return d:get().name
							end,
							description = function(): string
								return M.describe(d:get(), clock:get())
							end,
							icon = icon_of(d:peek().platform),
							badge = if d:peek().here then ds.Badge { ui.t("settings.devices.this_device"), size = "sm" } else nil,
							truncate = true,
							control = ds.Button {
								ui.t("settings.devices.rename"),
								tone = "secondary",
								size = "sm",
								label = ui.t("settings.devices.rename_name", { name = d:peek().name }),
								on_click = function()
									renaming:set(d:peek().id)
									new_name:set(d:peek().name)
									renaming_open:set(true)
								end,
							},
						}
					end, {
						key = function(d: sessions.Device): string
							return d.id
						end,
					}),
				}
			),
		},
		ds.Dialog {
			open = renaming_open,
			title = ui.t("settings.devices.dialog.title"),
			body = (function(): string
				local d = device(renaming:get())
				return if d and d.here then ui.t("settings.devices.dialog.here"):get() else ui.t("settings.devices.dialog.other"):get()
			end) :: any,
			size = "sm",
			confirm = { label = ui.t("settings.devices.rename") :: any, on_click = rename },
			ds.TextField {
				value = new_name,
				name = ui.t("settings.devices.dialog.field"),
				w_full = true,
				autofocus = true,
				select_on_focus = true,
				on_commit = function()
					rename()
					renaming_open:set(false)
				end,
			},
		},
	}
end

return M
