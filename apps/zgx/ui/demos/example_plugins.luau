-- Settings: plugins. What's installed, what's new, and what could be: a
-- real page's search, tabs, updates, and its loading, empty and failed states.
-- Browse is the store's catalog on the page's own pieces: the featured
-- (`Settings.Feature`, a cover over each), the catalog's card with its one
-- quiet row of filters (category tabs, the order at the right) and a
-- keyword narrowed to beside its search (`Settings.CatalogCard`), its rows
-- built as they show (`Settings.ListRows`);
-- a plugin opens a page under it (`Settings.SubPage`, `Settings.Back`).

local ds = require("@ds")
local Markdown = require("@ds/markdown")
local kit = require("@demos/example_kit")

local tab = ui.state("ex_pl_tab", "installed")
local query = ui.state("ex_pl_query", "")
local shown = ui.state("ex_pl_state", "ready")
local removing = ui.state("ex_pl_removing", false)
local category = ui.state("ex_pl_category", "")
local order = ui.state("ex_pl_order", "popular")
local keyword = ui.state("ex_pl_keyword", "design")
local opened = ui.state("ex_pl_opened", nil :: string?)
local enabled = {
	linear = ui.state("ex_pl_linear", true),
	sentry = ui.state("ex_pl_sentry", true),
	postgres = ui.state("ex_pl_postgres", false),
	playwright = ui.state("ex_pl_playwright", true),
}

local INSTALLED = {
	{ id = "linear", name = "Linear", by = "Zeron", hue = "indigo", version = "2.4.1", about = "Read issues and move them along from a chat." },
	{ id = "sentry", name = "Sentry", by = "Functional Software", hue = "rose", version = "1.9.0", update = "1.10.0", about = "Pull a stack trace and its release into the conversation." },
	{ id = "postgres", name = "Postgres", by = "Zeron", hue = "sky", version = "0.8.3", about = "Let agents read a schema and run read-only queries." },
	{ id = "playwright", name = "Playwright", by = "Microsoft", hue = "emerald", version = "3.2.0", about = "Drive a browser to check what an agent built." },
}
local CATALOG = {
	{ id = "figma", name = "Figma", by = "figma", hue = "orange", version = "1.2.0", installs = 4210, about = "Read frames and variables from a design file." },
	{ id = "slack", name = "Slack", by = "salesforce", hue = "purple", version = "4.0.2", installs = 3874, about = "Hear when an agent needs you, where you already are." },
	{ id = "datadog", name = "Datadog", by = "datadog", hue = "violet", version = "0.5.1", installs = 1290, about = "Bring a monitor's graph and its logs into a chat." },
}

local function matches(p: any): boolean
	local q = string.lower(query:get())
	return q == "" or string.find(string.lower(p.name .. " " .. p.about), q, 1, true) ~= nil
end

-- The plugins the tab holds that the search finds.
local found = ui.computed(function(): { any }
	local out = {}
	for _, p in (if tab:get() == "installed" then INSTALLED else CATALOG) do
		if matches(p) then
			table.insert(out, p)
		end
	end
	return out
end)

-- Browse: the featured over the catalog's card, its search, a bar of
-- filters, and its rows; a row opens the plugin's page.
local function browse(): Node
	local function feature(p: any): Node
		return ds.Settings.Feature {
			-- (A plugin with no screenshot: its tile on a quiet wash.)
			cover = ui.row {
				w_full = true,
				h_full = true,
				items = "center",
				justify = "center",
				bg = "$zeron.secondary.bg",
				ds.Tile { label = p.name, hue = p.hue, size = 48 },
			},
			label = p.name,
			byline = `by @{p.by}`,
			description = p.about,
			lead = ds.Tile { label = p.name, hue = p.hue, size = 40 },
			on_click = function()
				opened:set(p.id)
			end,
		}
	end
	return ui.column {
		w_full = true,
		gap = "$space.6",
		ui.show(function(): boolean
			return query:get() == ""
		end, ds.Settings.Section {
			title = "Featured",
			ui.row { w_full = true, gap = "$space.3", flex_wrap = true, feature(CATALOG[1]), feature(CATALOG[2]), feature(CATALOG[3]) },
		}),
		ds.Settings.CatalogCard {
			search = { value = query, placeholder = "Search plugins" },
			search_trail = ui.show(function(): boolean
				return keyword:get() ~= ""
			end, ds.Tag {
				function(): string
					return keyword:get()
				end :: any,
				size = "sm",
				selected = true,
				on_remove = function()
					keyword:set("")
				end,
			}),
			-- (A quiet tab's words are 12 inside it, so 4 here puts the
			-- first's on the rows' 16; the order ends on the rows' 16 too.)
			filters = ui.row {
				w_full = true,
				py = "$space.1",
				pl = "$space.1",
				pr = "$space.4",
				gap = "$space.3",
				items = "center",
				ui.scroll_x {
					grow = 1,
					min_w = 0,
					fade = { 12, 12 },
					fade_overflow = true,
					ds.Tabs {
						value = category,
						label = "Category",
						look = "pill",
						size = "sm",
						options = { { value = "", label = "All" }, { value = "integrations", label = "Integrations" }, { value = "developer-tools", label = "Developer tools" }, { value = "productivity", label = "Productivity" } },
					},
				},
				-- (How many it matches, before the order.)
				ui.text {
					function(): string
						return `{#found:get()} plugins`
					end,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
				},
				ui.div {
					shrink = 0,
					ds.Dropdown {
						value = order :: any,
						label = "Order",
						search = false,
						items = {
							{ value = "popular", label = "Most installed", icon = "download" },
							{ value = "updated", label = "Recently updated", icon = "refresh" },
							{ value = "new", label = "Newest", icon = "calendar" },
						},
					},
				},
			},
			body = ds.Settings.ListRows(found, function(p: any): Node
				return ds.Settings.Row {
					label = p.name,
					byline = `by @{p.by} · {p.installs} installs`,
					description = p.about,
					lead = ds.Tile { label = p.name, hue = p.hue, size = 32 },
					truncate = true,
					value = if p.id == "slack" then "Installed" else "",
					on_click = function()
						opened:set(p.id)
					end,
				}
			end, {
				key = function(p: any): string
					return p.id
				end,
				height = function(): number
					return ds.Settings.truncated_row_h(32, true)
				end,
			}),
		},
	}
end

-- A plugin of the store, opened: a page under Plugins.
local function listing(id: string): Node
	local p = CATALOG[1]
	for _, c in CATALOG do
		if c.id == id then
			p = c
		end
	end
	return ds.Settings.SubPage {
		where = ds.Settings.Back {
			label = "Plugins",
			on_click = function()
				opened:set(nil)
			end,
		},
		ds.Settings.Page {
			title = p.name,
			description = p.about,
			trail = ds.Tile { label = p.name, hue = p.hue, size = 48 },
			ds.Settings.Group {
				ui.column {
					w_full = true,
					px = "$space.4",
					py = "$space.3",
					ds.KeyValue { items = { { key = "By", value = `@{p.by}` }, { key = "Version", value = p.version }, { key = "Installs", value = `{p.installs} installs` } }, key_width = 96 },
				},
			},
			ui.row { w_full = true, justify = "end", ds.Button { "Install", tone = "primary" } },
			-- Its README as a document under the page's title (ds/markdown's
			-- `heading_step`, `drop_title`, `picture_radius`): its `# Name`
			-- not drawn again, its headings a level down in size, none larger
			-- than the page's name, each with its own level's room above (its
			-- `## Settings` 12 under the words before it, as in a chat reply).
			ds.Settings.Section {
				title = "About",
				ds.Settings.Group {
					ui.column {
						w_full = true,
						p = "$space.4",
						Markdown {
							text = `# {p.name}\n\n{p.about}\n\n# Getting started\n\nSign in once, then ask for it by name in any chat.\n\n## Settings\n\nNone: it reads what you already allowed.`,
							foreign = true,
							heading_step = 1,
							drop_title = function(words: string): boolean
								return string.lower(words) == string.lower(p.name)
							end,
							picture_radius = "$radius.lg",
						},
					},
				},
			},
		},
	}
end

local function row(p: any, installed: boolean): Node
	local control: Node
	if installed then
		control = ui.row {
			gap = "$space.2",
			items = "center",
			if p.update then ds.Button { "Update", tone = "secondary", size = "sm" } else nil,
			ds.Switch { value = enabled[p.id], label = p.name },
			ds.IconButton { icon = "trash-bin-minimalistic", label = `Remove {p.name}`, size = "sm", tone = "quiet", on_click = function() removing:set(true) end },
		}
	else
		control = ds.Button { "Install", tone = "secondary", size = "sm" }
	end
	return ds.Settings.Row {
		label = p.name,
		description = p.about,
		lead = ds.Tile { label = p.name, hue = p.hue, size = 32 },
		badge = if p.update
			then ds.Badge { `{p.version} → {p.update}`, tone = "accent", tabular = true }
			else ds.Badge { p.version, tabular = true },
		control = control,
	}
end

return {
	id = "ex_plugins",
	title = "Plugins",
	about = "Settings → Plugins: what's installed and what could be, searched; each switched on or off, updated or removed. And what shows while the catalog loads, when it can't be reached, and when nothing matches.",
	demo = function()
		-- A tab's plugins, each row its plugin's as the search narrows and
		-- widens; or, finding none, what it looked for.
		local function list(which: string): Node
			return ds.Settings.Each(found, function(p: any): Node
				return row(p, which == "installed")
			end, {
				key = function(p: any): string
					return p.id
				end,
			})
		end
		local function nothing(): Node
			return ds.EmptyState {
				icon = "search",
				title = function(): string
					return `Nothing matches “{query:get()}”`
				end,
				body = "Try another word, or look in the other tab.",
				size = "sm",
				action = { label = "Clear search", on_click = function() query:set("") end },
			}
		end
		local function body(key: string): Node
			if key == "loading" then
				return ds.Settings.Group {
					ds.SkeletonRow { trail = 40 },
					ds.SkeletonRow { title = 96, line = 280, trail = 40 },
					ds.SkeletonRow { title = 120, line = 200, trail = 40 },
					ds.SkeletonRow { title = 84, line = 260, trail = 40 },
				}
			elseif key == "failed" then
				return ds.Callout {
					tone = "danger",
					title = "Couldn't reach the plugin catalog",
					body = "Your installed plugins still work. Zeron will try again when you're back online.",
					action = { label = "Try again", on_click = function() shown:set("ready") end },
				}
			elseif key == "none" then
				return ds.EmptyState {
					icon = "widget",
					title = "No plugins yet",
					body = "Plugins give your agents tools: issue trackers, databases, browsers.",
					action = { label = "Browse plugins", on_click = function() shown:set("ready"); tab:set("browse") end },
				}
			end
			if string.match(key, ":nothing$") then
				return nothing()
			end
			if key == "list:browse" then
				return browse()
			end
			return list(string.match(key, "^list:(%a+)") or "installed")
		end
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "loading", label = "Loading" },
				{ value = "none", label = "Empty" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 900,
			h = 640,
			overlays = {
				ds.Dialog {
					open = removing,
					title = "Remove Sentry?",
					body = "Agents lose its tools in every project. Its settings stay, in case it comes back.",
					tone = "danger",
					confirm = { label = "Remove" },
				},
			},
			kit.settings {
				page = "plugins",
				ui.presence(function(): string
					return opened:get() or ""
				end, function(id: string): Node
				if id ~= "" then
					return listing(id)
				end
				return ds.Settings.Page {
					title = "Plugins",
					description = "Tools your agents can use, in every project in Halcyon Labs.",
					trail = ds.SearchField { value = query, placeholder = "Search plugins", size = "sm", w = 200 },
					ui.column {
						w_full = true,
						gap = "$space.4",
						ds.Tabs {
							value = tab,
							label = "Plugins",
							options = {
								{ value = "installed", label = "Installed", count = #INSTALLED },
								{ value = "browse", label = "Browse", count = #CATALOG },
							},
						},
						ui.presence(function(): string
							local s = shown:get()
							if s ~= "ready" then
								return s
							end
							-- A tab is a page of its own; within it, found or not.
							return if #found:get() > 0 then `list:{tab:get()}` else `list:{tab:get()}:nothing`
						end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
					},
				}
				end, { w_full = true }),
			},
		}
	end,
}
