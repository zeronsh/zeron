-- Settings → Browser → Browsing: the panel's browser tab
-- (screens/panel/browser), in three sections. Links and search: where a
-- chat's links open, what the address bar searches with (a search address
-- of your own too). Zoom: how big pages show, and each site you zoomed
-- yourself, one by one, with its percentage and a way back. Developer:
-- whether pages may be inspected. This Mac's, kept across launches
-- (screens/panel/browser_data), read by the tab as they change. History is
-- the Browser heading's other page (browser_history.luau).
--
-- Every select is one width, so the controls end on one edge and start on
-- another.
--
-- Sign-ins: each account's pages keep their cookies and site data in a
-- store of their own on this Mac, kept across launches, so a site stays
-- signed in. Clearing that store, a site's data or the cache, and each
-- site's permissions wait on zgx (zgx-asks/web-site-data,
-- web-permissions): no row here until they can work.

local ds = require("@ds")
local IconButton = require("@ds/icon_button").IconButton
local prefs = require("@app/screens/settings/prefs")
local search = require("@app/screens/settings/search")
local data = require("@app/screens/panel/browser_data")

local M = {}

M.ID = "browser"
M.TITLE = "Browsing"

search.index(M.ID, ui.t("settings.browser.page_title"), ui.t("settings.browser.terms"), {
	{ key = "open_links", title = ui.t("settings.general.open_links.label"), terms = ui.t("settings.general.open_links.terms") },
	{ key = "search_engine", title = ui.t("settings.browser.search_engine.label"), terms = ui.t("settings.browser.search_engine.terms") },
	{ key = "default_zoom", title = ui.t("settings.browser.default_zoom.label"), section = ui.t("settings.browser.zoom"), terms = ui.t("settings.browser.default_zoom.terms") },
	{ key = "inspectable", title = ui.t("settings.browser.inspectable.label"), section = ui.t("settings.browser.developer"), terms = ui.t("settings.browser.inspectable.terms") },
})

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

-- The selects' one width.
local SELECT_W = 168
-- A small button's side padding (ds/button `sm`).
local SNUG = (ui.theme.get("zeron.space.snug") :: number?) or 8

local custom_text = ui.state("settings_browser_custom_text", data.custom_engine:peek())

-- The sites you zoomed yourself, by host (followed).
local sites = ui.computed(data.site_zoom_list)

-- A site's row: its icon (from a visit, else its initial), its host, the
-- zoom it keeps, and the way back to the default.
local function site_row(item: { host: string, zoom: number }, _i: number, now: State<any>): Node
	return ds.Settings.Row {
		truncate = true,
		lead = ds.Tile {
			size = 20,
			label = data.site_name(item.host),
			src = function(): string?
				local profile = data.profile()
				return if profile then data.icon_of(profile, (now:get() :: any).host) else nil
			end,
		},
		label = function(): string
			return (now:get() :: any).host
		end,
		control = ui.row {
			items = "center",
			gap = "$space.2",
			ui.text {
				function(): string
					return data.percent((now:get() :: any).zoom)
				end,
				literal = true,
				tabular = true,
				text_size = "$font.size.sm",
				color = "$text.secondary",
			},
			IconButton {
				icon = "close",
				size = "sm",
				tone = "quiet",
				label = ui.t("settings.browser.site_zoom.reset_one", { site = item.host }),
				on_click = function()
					data.set_zoom(item.host, data.default_zoom:peek())
				end,
			},
		},
	}
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.browser.page_title"),
		description = ui.t("settings.browser.description"),
		ds.Settings.Section {
			title = ui.t("settings.browser.links_and_search"),
			ds.Settings.Group {
				row("open_links", {
					label = ui.t("settings.general.open_links.label"),
					description = function()
						if prefs.open_links:get() == "external" then
							return ui.t("settings.general.links_in.external"):get()
						end
						return ui.t("settings.general.links_in.tab"):get()
					end,
					control = ds.Select { value = prefs.open_links, name = ui.t("settings.general.open_links.label"), w = SELECT_W, options = prefs.OPEN_LINKS },
				}),
				ui.column {
					w_full = true,
					row("search_engine", {
						label = ui.t("settings.browser.search_engine.label"),
						description = ui.t("settings.browser.search_engine.description"),
						control = ui.div {
							id = "settings-browser-engine",
							ds.Select { value = data.engine, name = ui.t("settings.browser.search_engine.label"), w = SELECT_W, options = data.engine_options },
						},
					}),
					ui.show(function(): boolean
						return data.engine:get() == "custom"
					end, ui.column {
						w_full = true,
						ds.Settings.Rule(),
						ds.Settings.Row {
							label = ui.t("settings.browser.search_engine.custom_label"),
							description = function(): string
								local t = data.custom_engine:get()
								if t ~= "" and not data.valid_template(t) then
									return ui.t("settings.browser.search_engine.custom_invalid"):get()
								end
								return ui.t("settings.browser.search_engine.custom_hint"):get()
							end,
							stacked = true,
							control = ds.TextField {
								id = "settings-browser-custom-engine",
								value = custom_text,
								name = ui.t("settings.browser.search_engine.custom_label"),
								placeholder = "https://example.com/search?q=%s",
								inputmode = "url",
								w_full = true,
								on_commit = function(text: string)
									data.custom_engine:set(string.match(text, "^%s*(.-)%s*$") or "")
								end,
							},
						},
					}),
				},
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.browser.zoom"),
			ds.Settings.Group {
				row("default_zoom", {
					label = ui.t("settings.browser.default_zoom.label"),
					description = ui.t("settings.browser.default_zoom.description", { zoom_in = ui.shortcut_text("cmd+="), zoom_out = ui.shortcut_text("cmd+-") }),
					control = ds.Select { value = data.default_zoom, name = ui.t("settings.browser.default_zoom.label"), w = SELECT_W, options = data.zoom_options },
				}),
			},
			-- Each site you zoomed, under the default, and all of them back
			-- at once from the heading over them.
			ui.show(function(): boolean
				return #sites:get() > 0
			end, ui.column {
				id = "settings-browser-sites",
				w_full = true,
				gap = "$space.2",
				pt = "$space.2",
				ui.row {
					w_full = true,
					-- (The words on the rows' edge; Reset all's ink on the
					-- controls' edge, past its own padding.)
					pl = ds.Settings.inset(),
					pr = ds.Settings.inset(SNUG),
					items = "center",
					ds.SectionTitle {
						function(): string
							return ui.t("settings.browser.site_zoom.label", { n = #sites:get() }):get()
						end,
						trail = ds.Button {
							id = "settings-browser-reset-zoom",
							tone = "ghost",
							size = "sm",
							on_click = function()
								data.reset_zooms()
							end,
							ui.text { ui.t("settings.browser.site_zoom.reset") },
						},
					},
				},
				ds.Settings.Each(sites, site_row, {
					key = function(item: any): string
						return item.host
					end,
				}),
			}),
		},
		ds.Settings.Section {
			title = ui.t("settings.browser.developer"),
			ds.Settings.Group {
				row("inspectable", {
					label = ui.t("settings.browser.inspectable.label"),
					description = ui.t("settings.browser.inspectable.description"),
					control = ds.Switch { value = data.inspectable, label = ui.t("settings.browser.inspectable.label") },
				}),
			},
		},
	}
end

return M
