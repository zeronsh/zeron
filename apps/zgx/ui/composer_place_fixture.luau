-- A new chat's composer over its wallpaper, for
-- tests/composer_place.test.luau and captures: the footer's place chip
-- (project › branch), its backing over the art, and the context ring's
-- card. Each piece is a state the test (or `zgx inspect set`) changes:
--
--   test_place_art       the wallpaper shows (the footer stands over it)
--   test_place_effect    "none" | "dither" | "ascii" | "halftone" | "scanlines"
--   test_place_dark      the theme
--   test_place_project   "zeron", "atlas", "none"
--   test_place_branch    the branch chosen
--   test_place_checkout  "worktree" | "local"
--   test_place_usage     { tokens, window } or nil
--   test_place_w         the composer's width
--   test_place_card      the ring's card, drawn in place over the composer
--   test_place_menu_specimen  the draft's text menu, drawn in place
--   test_place_bare      the art hidden, its backings kept (to measure ink)
--   test_place_picture   "plate-1-night" | "plate-3-sunrise"
--   test_place_radii     "default" | "hot" (the Hot Pink template's radii; reload)

ui.theme.strict("warn")
local ds = require("@ds")
local Composer = require("@ds/composer")
ds.setup()
-- (`test_place_radii` = "hot": the Hot Pink template's radii, the pill's 36
-- and the trays' 24, read as the fixture loads: set it, then reload.)
-- (A test sets the shared `test_place_hot_radii` before the fixture loads.)
if ui.state("test_place_radii", "default"):peek() == "hot" or ui.shared("test_place_hot_radii", false):peek() == true then
	ui.theme.tokens { zeron = { composer = { radius = 36, tray = { radius = 24 } } } }
end

local art = ui.state("test_place_art", true)
local effect = ui.state("test_place_effect", "scanlines")
local dark = ui.state("test_place_dark", true)
local width = ui.state("test_place_w", 640)
local card = ui.state("test_place_card", false)
local menu_specimen = ui.state("test_place_menu_specimen", false)
local bare = ui.state("test_place_bare", false)
-- (The picture: "plate-1-night" dark, "plate-3-sunrise" bright.)
local picture = ui.state("test_place_picture", "plate-1-night")
local usage = ui.state("test_place_usage", { tokens = 408389, window = 1000000 } :: Composer.Usage?)

-- The theme, as the app sets it (the window's, global), so what reads the
-- theme's values follows it too.
ui.effect(function()
	ui.theme.follow_system(false)
	ui.theme.current:set(if dark:get() then "zeron-dark" else "zeron-light")
end)

-- (For tests: what zgx's own menu tokens resolve to beside ours, as the
-- theme changes.)
local menu_tokens = ui.state("test_place_menu_tokens", {} :: { [string]: any })
ui.effect(function()
	local _ = ui.theme.current:get()
	menu_tokens:set {
		menu_bg = tostring(ui.theme.get("menu.bg")),
		menu_hover = tostring(ui.theme.get("menu.hover")),
		tint = tostring(ui.theme.get("zeron.menu.tint")),
		hover = tostring(ui.theme.get("hover")),
	}
end)

local HARNESSES: { ds.Harness } = { { id = "claude", label = "Claude", mark = "claude" } }
local MODELS: { ds.Model } = { { value = "opus", label = "Opus 5.5", harness = "claude" } }
local PROJECTS: { Composer.Project } = {
	{ value = "zeron", label = "zeron", hue = "violet" },
	{ value = "atlas", label = "Atlas Platform Monorepo", hue = "teal" },
}
local BRANCHES = {
	{ value = "main", label = "main" },
	{ value = "zeron/create-readme-update-for-onboarding", label = "zeron/create-readme-update-for-onboarding" },
}

local box = ui.ref()
return ui.column {
	ref = box,
	w_full = true,
	h_full = true,
	relative = true,
	overflow = "hidden",
	theme = function(): string
		return if dark:get() then "zeron-dark" else "zeron-light"
	end,
	-- (Opaque: a capture of the window's glass comes out white.)
	bg = function(): any
		return ui.raw(if dark:get() then "#111113" else "#f4f4f5", "a capture's backdrop: the window's glass snapshots white")
	end,
	items = "center",
	justify = "center",
	ds.Wallpaper {
		effect = effect,
		-- (`test_place_bare`: the art hidden, its backings kept: a plain
		-- ground to measure the chips' ink on.)
		shown = function(): boolean
			return art:get() and not bare:get()
		end,
		faded = false,
		width = function(): number
			return box.width or 960
		end,
		key = function(): string
			return picture:get()
		end,
		content = function(): Node
			-- (Named for tests: which picture stands there.)
			return ui.div {
				id = `wall-{picture:peek()}`,
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				ds.wallpaper.Picture { src = `assets/art/{picture:peek()}.jpg`, w = 2100, h = 700, x = 0.62, y = 0.7, zoom = 1.4 },
			}
		end,
	},
	-- (The window's menu host, as the shell mounts it: what opens the text
	-- menus, ds/menu_at's.)
	require("@ds/menu_at").Host {},
	-- (What a capture takes: the composer and some of the art round it.)
	ui.column {
		id = "place-shot",
		p = "$space.6",
	ui.column {
		id = "place-stage",
		w = function(): number
			return width:get()
		end :: any,
		-- The draft's text menu (ds/editor_menu), drawn in place as the
		-- menus' specimen (`still`): a menu is a window of its own, which a
		-- snapshot of the page doesn't hold. For captures.
		ui.show(function(): boolean
			return menu_specimen:get()
		end, ui.row {
			justify = "start",
			pb = "$space.2",
			require("@ds/context_menu") {
				still = require("@ds/editor_menu").entries({
					selected = "dasdasd",
					copyable = true,
					editable = true,
					clipboard = true,
					platform = true,
					looks_up = true,
				}, { takes_paste = true }),
				items = function()
					return nil
				end,
			},
		}),
		-- The ring's card, drawn in place (a popover is a window of its own,
		-- which a snapshot of the page doesn't hold): for captures.
		ui.show(function(): boolean
			return card:get()
		end, ui.row {
			justify = "end",
			pb = "$space.2",
			ds.context_usage.Card {
				usage = function(): Composer.Usage?
					return usage:get()
				end,
			},
		}),
		ds.Composer {
			value = ui.state("test_place_draft", ""),
			expanded = true,
			menus_below = true,
			access = ui.state("test_place_access", "full" :: string?),
			plan = ui.state("test_place_plan", false),
			model = ui.state("test_place_model", "opus" :: string?),
			effort = ui.state("test_place_effort", "high" :: string?),
			harnesses = HARNESSES,
			models = MODELS,
			project = ui.state("test_place_project", "zeron" :: string?),
			projects = PROJECTS,
			checkout = ui.state("test_place_checkout", "worktree" :: string?),
			branch = ui.state("test_place_branch", "zeron/create-readme-update-for-onboarding" :: string?),
			branches = BRANCHES,
			-- (The fraction too: a composer before `usage` reads it.)
			context = function(): number
				local u = usage:get()
				return if u and u.tokens and u.window then u.tokens / u.window else 0
			end,
			usage = function(): Composer.Usage?
				return usage:get()
			end,
			over_art = function(): boolean
				return art:get()
			end,
		},
	},
	},
}
