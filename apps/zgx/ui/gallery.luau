-- The design system's workbench: a page per component, grouped in a sidebar
-- that shows and hides, one theme at a time. The header's switches choose
-- light or dark and frosted (the window's glass) or opaque. Save any ds/
-- module or theme file and the page restyles in place.
--
-- Run: cargo run --release -- gallery

ui.theme.strict("warn")
local ds = require("@ds")
local Dropdown = require("@ds/dropdown")
local SidebarTypes = require("@ds/sidebar")
local ComposerTypes = require("@ds/composer")
-- Pages built in files of their own (each its own work).
local context_menu_page = require("@demos/context_menu")
local command_palette_page = require("@demos/command_palette")
local transcript_page = require("@demos/transcript")
local syntax_page = require("@demos/syntax")
local files_page = require("@demos/files")
local code_editor_page = require("@demos/code_editor")
-- Groups of pages kept in files of their own (each a chat's work).
local primitives_groups = require("@demos/primitives")
local patterns_groups = require("@demos/patterns")
ds.setup()

-- What the workbench shows, kept across launches.
local dark = ui.persist("dark", true)
-- Frosted or opaque: the app's one setting (ds.material), which components
-- follow too.
local frosted = ds.material.frosted
local sidebar = ui.persist("sidebar", true)
local page = ui.persist("page", "buttons")
-- Where the workbench is (made once the pages are known, below; the App
-- shell's page reads it too: one router, as an app has one).
local router: Router
-- The page a route shows: a page's own route, or the App shell's sessions.
local function page_of(name: string): string
	return if name == "sidebar_session" then "sidebar" else name
end
local function on_page(id: string): boolean
	return page_of(router.route:get().name) == id
end
-- The windows' own chrome (the traffic lights) in the theme shown.
ui.effect(function()
	ui.invoke("appearance", dark:get())
end)
-- The demos' own state.
local picked = ui.state("picked", 2)
local on = ui.state("on", true)
local density = ui.state("density", "regular")
local project = ui.state("project", "comet-native" :: string?)
-- The composer's: its draft, a running turn, the model and the checkout.
local draft = ui.state("draft", "")
local turn = ui.state("turn", "idle")
local running = ui.computed(function(): boolean
	return turn:get() == "running"
end)
local model = ui.state("model", "opus" :: string?)
local access = ui.state("access", "full" :: string?)
local plan = ui.state("plan", false)
local checkout = ui.state("checkout", "worktree" :: string?)
local device = ui.state("device", "mac" :: string?)
local DEVICES: { ds.ComposerDevice } = {
	{ value = "mac", label = "MacBook Pro", you = true },
	{ value = "devbox", label = "devbox" },
	{ value = "studio", label = "studio", offline = true },
	{ value = "cloud", label = "Cloud", cloud = true },
}
local branch = ui.state("branch", "zeron/zgx-onboarding-design-system" :: string?)
-- The agents, and their models with the reasoning each offers (low to
-- high) and the level it starts on.
local HARNESSES: { ds.Harness } = {
	{ id = "claude", label = "Claude", mark = "claude" },
	{ id = "codex", label = "Codex", mark = "openai" },
	{ id = "grok", label = "Grok", mark = "grok" },
	{ id = "cursor", label = "Cursor", mark = "cursor" },
	{ id = "opencode", label = "OpenCode", mark = "opencode" },
	{ id = "devin", label = "Devin", mark = "devin" },
	{ id = "antigravity", label = "Antigravity", mark = "antigravity" },
	{ id = "pi", label = "Pi", mark = "pi" },
	{ id = "hermes", label = "Hermes", mark = "hermes" },
	{ id = "muse", label = "Muse", mark = "muse" },
}
local CLAUDE_EFFORT = {
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
	{ value = "xhigh", label = "Extra high" },
	{ value = "max", label = "Max" },
}
local GPT_EFFORT = {
	{ value = "minimal", label = "Minimal" },
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
	{ value = "xhigh", label = "Extra high" },
}
local MODELS: { ds.Model } = {
	{ value = "opus", label = "Opus 5.5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "high", fast = true },
	{ value = "sonnet", label = "Sonnet 5.5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "medium" },
	{ value = "fable", label = "Fable 5.1", harness = "claude", efforts = CLAUDE_EFFORT, effort = "high" },
	{ value = "haiku", label = "Haiku 5.5", harness = "claude" },
	{ value = "opus-5", label = "Opus 5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "high" },
	{ value = "sonnet-5", label = "Sonnet 5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "medium" },
	{ value = "haiku-5", label = "Haiku 5", harness = "claude" },
	{ value = "opus-4-5", label = "Opus 4.5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "high" },
	{ value = "sonnet-4-5", label = "Sonnet 4.5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "medium" },
	{ value = "haiku-4-5", label = "Haiku 4.5", harness = "claude" },
	{ value = "gpt", label = "GPT-5.5", harness = "codex", efforts = GPT_EFFORT, effort = "medium", fast = true },
	{ value = "gpt-codex", label = "GPT-5.5 Codex", harness = "codex", efforts = GPT_EFFORT, effort = "high" },
	{ value = "gpt-mini", label = "GPT-5.5 Mini", harness = "codex", efforts = GPT_EFFORT, effort = "low" },
	{
		value = "grok",
		label = "Grok 5",
		harness = "grok",
		efforts = { { value = "low", label = "Low" }, { value = "high", label = "High" } },
		effort = "high",
	},
	{ value = "grok-fast", label = "Grok 5 Fast", harness = "grok" },
	{ value = "composer", label = "Composer 2", harness = "cursor" },
	{ value = "auto", label = "Auto", harness = "cursor" },
	{ value = "big-pickle", label = "Big Pickle", harness = "opencode" },
	{ value = "devin-2", label = "Devin 2", harness = "devin" },
	-- Configured in place: its lead and sidekick are options (a card beside
	-- its row; its rows under it once chosen).
	{
		value = "fusion",
		label = "Fusion",
		harness = "devin",
		efforts = { { value = "low", label = "Low" }, { value = "medium", label = "Medium" }, { value = "high", label = "High" }, { value = "xhigh", label = "Extra high" } },
		effort = "high",
		fast = true,
		options = {
			{ id = "lead", label = "Lead", default = "fable", choices = { { id = "fable", label = "Claude Fable 5.1" }, { id = "sol", label = "GPT-6 Sol" } } },
			{ id = "sidekick", label = "Sidekick", default = "swe-medium", choices = { { id = "swe-medium", label = "SWE-2 Medium" }, { id = "swe-high", label = "SWE-2 High" } } },
		},
	},
	{ value = "gemini", label = "Gemini 3 Pro", harness = "antigravity", efforts = GPT_EFFORT, effort = "medium" },
	{ value = "pi-default", label = "Pi", harness = "pi" },
	{ value = "hermes-4", label = "Hermes 4", harness = "hermes" },
	{ value = "muse-1", label = "Muse 1", harness = "muse" },
}
local effort = ui.state("effort", "high" :: string?)
local fast = ui.state("fast", false)
-- The picks of the model's options (Fusion's lead and sidekick).
local options = ui.state("model_options", {} :: { [string]: string })
-- The slider's own demo: a level of its own.
local demo_effort = ui.state("demo_effort", "xhigh" :: string?)
local BRANCHES: { Dropdown.Item } = {
	{ value = "main", label = "main" },
	{ value = "zeron/zgx-onboarding-design-system", label = "zeron/zgx-onboarding-design-system" },
	{ value = "zeron/composer", label = "zeron/composer" },
}

-- The window's own geometry (not spacing): the title bar's strip, the
-- sidebar's width, where the toggle sits beside the window's buttons and how
-- far the header's title keeps from them while the sidebar is hidden.
local TITLEBAR, SIDEBAR_W = 40, 220
-- The sidebar's width as its edge is dragged (kept across launches): no
-- narrower than its rows read, and always leaving the page this much.
local SIDEBAR_MIN, PAGE_MIN = 160, 280
local sidebar_w = ui.persist("sidebar_w", SIDEBAR_W)
-- The width it shows at: its width, or 0 hidden; eased as it shows or
-- hides, followed at once as its edge is dragged.
local nav_w = ui.state("nav_w", if sidebar:peek() then sidebar_w:peek() else 0)
local function show_sidebar(open: boolean)
	sidebar:set(open)
	anime.animate(nav_w, { value = if open then sidebar_w:peek() else 0, duration = 200, ease = "out(3)" })
end
local root = ui.ref()
local TOGGLE_X, TOGGLE_Y = 86, 6
local CLEAR_OF_BUTTONS = 128
-- How far the page fades out at its foot, and where it first lines up under
-- the header: long enough to dissolve rather than cut off. And the header's
-- height: the page stops at it, so nothing runs behind its title.
local FADE = 72
local HEADER = 48

-- `foot`: pinned to the bottom of the page, over it (a chat's composer).
-- (`fill`: the page fills the window rather than scrolling in it, its demo
-- growing into the room left: for a demo with its own scroll views, a
-- transcript's or an editor's, so a wheel gesture over it never moves the
-- page instead.)
type Page = { id: string, title: string, about: string, demo: () -> Node, foot: (() -> Node)?, fill: boolean? }
type Group = { title: string, pages: { Page } }

-- A row of examples.
local function row(...: Child): Node
	return ui.row { gap = "$space.3", items = "center", flex_wrap = true, ... }
end

-- A titled run of examples on a page.
local function block(title: string, ...: Child): Node
	return ui.column {
		gap = "$space.3",
		ds.SectionTitle { title },
		...,
	}
end

local GROUPS: { Group } = {
	{
		title = "Foundations",
		pages = {
			{
				id = "type",
				title = "Type",
				about = "Every piece of text is one of these styles, in one of five tones.",
				demo = function()
					return ui.column {
						gap = "$space.6",
						block(
							"STYLES",
							ds.Text { "Welcome to Zeron", style = "display" },
							ds.Text { "Choose a look", style = "title" },
							ds.Text { "Heading", style = "heading" },
							ds.Text { "Body copy sits at thirteen points, in Geist.", style = "body" },
							ds.Text { "A caption.", style = "caption" },
							ds.Text { "LABEL", style = "label" },
							ds.Text { "cargo run -- gallery", style = "mono" }
						),
						block(
							"TONES",
							ds.Text { "Primary", tone = "primary" },
							ds.Text { "Secondary", tone = "secondary" },
							ds.Text { "Tertiary", tone = "tertiary" },
							ds.Text { "Accent", tone = "accent" },
							ds.Text { "Danger", tone = "danger" }
						),
					}
				end,
			},
			{
				id = "palette",
				title = "Palette",
				about = "Every hue by step, as Tailwind's: the same lightness at a step in any hue. Pull colours from here.",
				demo = function()
					local STEPS = { "50", "100", "200", "300", "400", "500", "600", "700", "800", "900", "950" }
					local HUES = {
						"gray", "red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal",
						"cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink", "rose",
					}
					local SWATCH, NAME = 44, 64
					local head: { Node } = { ui.div { w = NAME, shrink = 0 } }
					for _, step in STEPS do
						table.insert(head, ui.text { step, w = SWATCH, shrink = 0, font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary", align = "center" })
					end
					local rows: { Node } = { ui.row { gap = "$zeron.space.hair", table.unpack(head) } }
					for _, hue in HUES do
						local cells: { Node } = { ds.Text { hue, style = "caption", tone = "secondary", w = NAME, shrink = 0 } }
						for _, step in STEPS do
							table.insert(cells, ui.div {
								w = SWATCH,
								h = 28,
								shrink = 0,
								rounded = "$radius.sm",
								bg = `$palette.{hue}.{step}`,
								tooltip = `palette.{hue}.{step}`,
							})
						end
						table.insert(rows, ui.row { gap = "$zeron.space.hair", items = "center", table.unpack(cells) })
					end
					return ui.column { gap = "$zeron.space.hair", table.unpack(rows) }
				end,
			},
			-- Every palette a window can wear, and the import
			-- (demos/theme_library.luau), loaded on its own so a slip there
			-- shows in its place.
			(function(): Page
				local ok, result = pcall(function()
					return require("@demos/theme_library")
				end)
				if ok then
					return result
				end
				return {
					id = "theme-library",
					title = "Theme library",
					about = "This page didn't load.",
					demo = function()
						return ui.text { tostring(result), literal = true, wrap = true, font = "$font.mono", text_size = "$font.size.sm", color = "$danger" }
					end,
				}
			end)(),
		},
	},
	{
		title = "Actions",
		pages = {
			{
				id = "buttons",
				title = "Buttons",
				about = "One primary per screen; secondary beside it; ghost for the way back.",
				demo = function()
					return ui.column {
						gap = "$space.6",
						block(
							"TONES",
							row(
								ds.Button { "Continue" },
								ds.Button { "Secondary", tone = "secondary" },
								ds.Button { "Back", tone = "ghost" },
								ds.Button { "Accent", tone = "accent" }
							)
						),
						block(
							"SIZES",
							row(
								ds.Button { "Small", size = "sm" },
								ds.Button { "Medium" },
								ds.Button { "Large", size = "lg" }
							)
						),
						block(
							"SHAPES",
							row(
								ds.Button { "Continue", shape = "pill" },
								ds.Button { "Secondary", tone = "secondary", shape = "pill" },
								ds.Button { "Back", tone = "ghost", shape = "pill" },
								ds.Button { "Accent", tone = "accent", shape = "pill" }
							)
						),
						block(
							"WITH AN ICON",
							row(
								ds.Button { ds.Icon { name = "plus" }, "New chat" },
								ds.Button { tone = "secondary", ds.Icon { name = "copy" }, "Copy" },
								ds.Button { tone = "ghost", ds.Icon { name = "refresh" }, "Retry" },
								ds.Button { tone = "accent", "Open", ds.Icon { name = "arrow-up-right" } },
								ds.Button { tone = "secondary", shape = "pill", "Continue", ds.Icon { name = "arrow-right" } }
							)
						),
						block(
							"ICON ONLY",
							row(
								ds.Button { icon = true, label = "New", ds.Icon { name = "plus" } },
								ds.Button { icon = true, tone = "secondary", label = "Settings", ds.Icon { name = "settings-minimalistic" } },
								ds.Button { icon = true, tone = "ghost", label = "More", ds.Icon { name = "more-horizontal" } },
								ds.Button { icon = true, tone = "accent", label = "Done", ds.Icon { name = "check" } },
								ds.Button { icon = true, tone = "ghost", label = "Delete", ds.Icon { name = "trash-bin-minimalistic" } }
							),
							row(
								ds.Button { icon = true, size = "sm", tone = "secondary", label = "Edit", ds.Icon { name = "pen", size = 14 } },
								ds.Button { icon = true, tone = "secondary", label = "Edit", ds.Icon { name = "pen" } },
								ds.Button { icon = true, size = "lg", tone = "secondary", label = "Edit", ds.Icon { name = "pen", size = 18 } },
								ds.Button { icon = true, shape = "pill", tone = "secondary", label = "Close", ds.Icon { name = "close" } },
								ds.Button { icon = true, shape = "pill", size = "lg", label = "Send", ds.Icon { name = "arrow-right", size = 18 } }
							)
						),
						block("STATES", row(ds.Button { "Disabled", disabled = true }, ds.Button { icon = true, disabled = true, label = "Add", ds.Icon { name = "plus" } })),
					}
				end,
			},
		},
	},
	{
		title = "Inputs",
		pages = {
			{
				id = "switch",
				title = "Switch",
				about = "A groove and a knob that springs across it; on, the groove fills with the accent.",
				demo = function()
					return block(
						"STATES",
						row(
							ds.Switch { value = on, label = "Example" },
							ds.Text {
								function()
									return if on:get() then "On" else "Off"
								end,
								tone = "secondary",
							}
						)
					)
				end,
			},
			-- The composer's states, a page of its own (demos/composer.luau),
			-- loaded on its own so a slip there shows in its place and the
			-- rest of the workbench carries on.
			(function(): Page
				local ok, result = pcall(function()
					return require("@demos/composer")
				end)
				if ok then
					return result
				end
				return {
					id = "composer",
					title = "Composer",
					about = "This page didn't load.",
					demo = function()
						return ui.text { tostring(result), literal = true, wrap = true, font = "$font.mono", text_size = "$font.size.sm", color = "$danger" }
					end,
				}
			end)(),
		},
	},
	{
		title = "Choices",
		pages = {
			{
				id = "segmented",
				title = "Segmented",
				about = "One of a few options, a raised thumb springing to the chosen one.",
				demo = function()
					return block(
						"OPTIONS",
						row(
							ds.Segmented {
								value = density,
								options = {
									{ value = "compact", label = "Compact" },
									{ value = "regular", label = "Regular" },
									{ value = "roomy", label = "Roomy" },
								},
								segment = 72,
							}
						)
					)
				end,
			},
		},
	},
	{
		title = "Menus",
		pages = {
			{
				id = "dropdown",
				title = "Dropdown",
				about = "A compact trigger that opens a panel: a search, the items it filters, actions below.",
				demo = function()
					local PROJECTS: { Dropdown.Item } = {
						{ value = "tidepool", label = "tidepool" },
						{ value = "comet-native", label = "comet-native" },
						{ value = "zgx", label = "zgx" },
						{ value = "zui", label = "zui" },
						{ value = "landing", label = "landing" },
						{ value = "atlas", label = "atlas" },
						{ value = "beacon", label = "beacon" },
						{ value = "cinder", label = "cinder" },
						{ value = "delta-ops", label = "delta-ops" },
						{ value = "ember", label = "ember" },
						{ value = "fjord", label = "fjord" },
						{ value = "glacier", label = "glacier" },
						{ value = "harbor", label = "harbor" },
						{ value = "indigo", label = "indigo" },
						{ value = "juniper", label = "juniper" },
						{ value = "kestrel", label = "kestrel" },
						{ value = "lumen", label = "lumen" },
						{ value = "meridian", label = "meridian" },
						{ value = "nimbus", label = "nimbus" },
						{ value = "orbit", label = "orbit" },
					}
					return block(
						"PROJECT",
						row(
							ds.Dropdown {
								value = project,
								label = "Project",
								icon = "folder",
								placeholder = "No project",
								search = "Search projects…",
								items = PROJECTS,
								actions = {
									{ label = "New project", icon = "plus" },
									{
										label = "Don't work in a project",
										icon = "close",
										on_click = function()
											project:set(nil)
										end,
									},
								},
							}
						)
					)
				end,
			},
			{
				id = "model-picker",
				title = "Model picker",
				about = "An agent's models by tab, searched, then the chosen model's reasoning: a slider whose fill is a field of characters, busier the harder it thinks.",
				demo = function()
					return ui.column {
						gap = "$space.6",
						block(
							"PICKER",
							row(ds.ModelPicker {
								model = model,
								effort = effort,
								fast = fast,
								options = options,
								harnesses = HARNESSES,
								models = MODELS,
								tone = "filled",
							})
						),
						block(
							"SLIDER",
							ui.div {
								w = 280,
								ds.EffortSlider { value = demo_effort, levels = CLAUDE_EFFORT, title = "REASONING" },
							}
						),
					}
				end,
			},
		},
	},
	{
		title = "Shell",
		pages = {
			{
				-- (Its id kept, so the page you were on stays chosen.)
				id = "sidebar",
				title = "App shell",
				fill = true,
				about = "The window as Zeron's: its title bar, its sidebar (grouped, sorted, your sections, dragged), a new chat over its wallpaper, a session's page, the composer travelling between them.",
				demo = function()
					local compact = ui.persist("demo_sidebar_compact", true)
					local collapsed = ui.state("demo_sidebar_collapsed", false)
					local chosen = ui.state("demo_sidebar_selected", "s-onboarding" :: string?)
					local pinned: { SidebarTypes.Session } = {
						{ id = "s-release", created = "1d", title = "Release v0.2.130", status = "approval", time = "2m", agent = "claude", device = "MacBook Pro", branch = "release/v0.2.130", pr = { number = 1092, state = "open", title = "Bump version to v0.2.130" } },
					}
					local projects: { SidebarTypes.Project } = {
						{
							id = "zeron",
							name = "Zeron",
							src = "assets/favicons/zeron.png",
							hue = "violet",
							sessions = {
								{ id = "s-onboarding", created = "2d", title = "Onboarding in zgx", status = "working", time = "now", agent = "claude", device = "MacBook Pro", branch = "zeron/zgx-onboarding", pr = { number = 1094, state = "draft", title = "zgx onboarding and design system" } },
								{ id = "s-rebase", created = "5h", title = "Which branch should this rebase onto?", status = "input", time = "4m", agent = "openai", device = "devbox", branch = "main" },
								{ id = "s-presence", created = "1d", title = "Rename the presence room", status = "done", time = "12m", agent = "claude", device = "MacBook Pro", branch = "presence-rename", pr = { number = 1088, state = "merged", title = "Rename the presence room" } },
								{ id = "s-flaky", created = "3d", title = "Fix the flaky local_first test", status = "failed", time = "1h", agent = "openai", device = "studio", branch = "fix/local-first" },
								{ id = "s-battery", created = "4h", title = "Battery drain on idle spinners", time = "3h", agent = "claude", device = "MacBook Pro", branch = "perf/spinners" },
								{ id = "s-ios", created = "1w", title = "Port the reasoning slider to iOS", time = "1d", agent = "cursor", device = "MacBook Pro", branch = "ios/slider" },
							},
						},
						{
							id = "zgx",
							name = "zgx",
							src = "assets/favicons/zgx.png",
							hue = "blue",
							sessions = {
								{ id = "s-bands", created = "6h", title = "Faded box outside a popup panics", status = "done", time = "8m", agent = "claude", device = "MacBook Pro", branch = "fix/band-strips", pr = { number = 950, state = "merged", title = "A faded box outside a popup's target skips its bands" } },
								{ id = "s-ring", created = "3d", title = "Themable focus ring width", time = "2h", agent = "claude", device = "MacBook Pro", branch = "focus-ring-width", pr = { number = 951, state = "closed", title = "A themable focus ring width" } },
							},
						},
						{
							id = "edge",
							name = "Edge",
							hue = "emerald",
							sessions = {
								{ id = "s-ws3", created = "1h", title = "Room socket goes deaf after auto-pong", status = "working", time = "now", agent = "openai", device = "devbox", branch = "ws3/pong" },
								{ id = "s-staging", created = "2w", title = "Staging deploy wedged on vault", time = "2d", agent = "grok", device = "studio", branch = "staging" },
							},
						},
						{
							id = "ios",
							name = "iOS",
							src = "assets/favicons/ios.png",
							hue = "sky",
							sessions = {
								{ id = "s-duo", created = "2d", title = "Fold rebuilds the session page", status = "input", time = "20m", agent = "claude", device = "MacBook Pro", branch = "ios/duo-fold" },
								{ id = "s-tf", created = "1w", title = "TestFlight build 98 looks old", time = "3d", agent = "openai", device = "MacBook Pro", branch = "main" },
							},
						},
						{
							id = "marigold",
							name = "Marigold",
							hue = "amber",
							src = "assets/favicons/marigold.png",
							sessions = {
								{ id = "s-garden", created = "30m", title = "Seed packets print upside down", status = "working", time = "now", agent = "claude", device = "studio", branch = "print/packets" },
								{ id = "s-frost", created = "5d", title = "Frost dates for the southern hemisphere", time = "6h", agent = "openai", device = "studio", branch = "frost/south" },
							},
						},
						{
							id = "flint",
							name = "Flint",
							hue = "gray",
							src = "assets/favicons/flint.png",
							sessions = {
								{ id = "s-ledger", created = "2h", title = "Ledger totals round twice", status = "done", time = "40m", agent = "claude", device = "MacBook Pro", branch = "ledger/rounding" },
							},
						},
						{
							id = "android",
							name = "Android",
							hue = "lime",
							sessions = {
								{ id = "s-emu", created = "6d", title = "Emulator needs more disk", status = "failed", time = "1h", agent = "claude", device = "MacBook Pro", branch = "android/app" },
							},
						},
						{
							id = "website",
							name = "Website",
							hue = "rose",
							sessions = {
								{ id = "s-pricing", created = "3w", title = "Pricing page copy", status = "done", time = "5h", agent = "cursor", device = "MacBook Pro", branch = "site/pricing" },
								{ id = "s-og", created = "2w", title = "Open graph images", time = "1w", agent = "claude", device = "MacBook Pro", branch = "site/og" },
							},
						},
					}
					-- Many: forty projects more of twenty-five sessions each (a
					-- thousand), to see the sidebar hold up.
					local many = ui.persist("demo_sidebar_many", false)
					local HUES = { "red", "orange", "amber", "lime", "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink", "rose" }
					local WORDS = { "Fix", "Port", "Refactor", "Speed up", "Test", "Design", "Rename", "Wire up", "Clean up", "Document" }
					local THINGS = { "the sync engine", "presence badges", "the composer dock", "image uploads", "the rail", "folds", "the transcript", "model catalogs", "the updater", "worktrees" }
					local STATES: { ds.StatusKind? } = { "working", "input", "approval", "done", "failed", nil, nil, nil, nil, nil }
					local AGENTS: { ds.MarkName } = { "claude", "openai", "cursor", "grok" }
					local DEVICES_OF = { "MacBook Pro", "devbox", "studio" }
					local AGES = { "now", "3m", "20m", "1h", "4h", "1d", "3d", "1w" }
					local extra: { SidebarTypes.Project } = {}
					for pi = 1, 40 do
						local sessions: { SidebarTypes.Session } = {}
						for si = 1, 25 do
							local n = pi * 31 + si * 7
							table.insert(sessions, {
								id = `x-{pi}-{si}`,
								title = `{WORDS[n % #WORDS + 1]} {THINGS[(n // 3) % #THINGS + 1]}`,
								status = STATES[n % #STATES + 1],
								time = AGES[(n // 5) % #AGES + 1],
								created = AGES[(n // 2) % #AGES + 1],
								agent = AGENTS[n % #AGENTS + 1],
								device = DEVICES_OF[n % #DEVICES_OF + 1],
								branch = `work/{pi}-{si}`,
							})
						end
						table.insert(extra, { id = `x-{pi}`, name = `Project {pi}`, hue = HUES[pi % #HUES + 1], sessions = sessions })
					end
					local function projects_for(lots: boolean): { SidebarTypes.Project }
						if not lots then
							return projects
						end
						local all = table.clone(projects)
						for _, p in extra do
							table.insert(all, p)
						end
						return all
					end
					-- The session open, for the pane beside the sidebar.
					local by_id: { [string]: SidebarTypes.Session } = {}
					for _, p in extra do
						for _, s in p.sessions do
							by_id[s.id] = s
						end
					end
					for _, s in pinned do
						by_id[s.id] = s
					end
					for _, p in projects do
						for _, s in p.sessions do
							by_id[s.id] = s
						end
					end
					-- A window: its title bar (the window's buttons, and the
					-- sidebar's toggle beside them, where it stays at any width),
					-- the sidebar, the open session.
					-- The title bar's height (Zeron's).
					local TITLE_H = 38
					-- What the page shows behind it: a picture, as ds.Wallpaper draws
					-- one (a card scene's sky standing in for your own), its effect.
					local wallpaper = ui.persist("demo_sidebar_wallpaper", "violet-dusk")
					if wallpaper:peek() ~= "none" and wallpaper:peek() ~= "violet-dusk" and wallpaper:peek() ~= "aurora" and wallpaper:peek() ~= "blossom" then
						wallpaper:set("violet-dusk")
					end
					-- The title bar's files toggle (lit while they'd show).
					local demo_files = ui.state("demo_sidebar_files", false)
					-- The sidebar's width (dragged at its edge), and what collapsing
					-- leaves: a rail of icons, or nothing (as Zeron's today).
					local side_w = ui.persist("demo_sidebar_width", 256)
					local side_mode = ui.persist("demo_sidebar_mode", "rail")
					-- Its view options (its "…"), shared by it and its rail.
					local organize = ui.persist("demo_sidebar_organize", "project" :: SidebarTypes.Organize)
					local sort = ui.persist("demo_sidebar_sort", "updated" :: SidebarTypes.Sort)
					-- Where sessions are put by hand: pinned, and sections of your own.
					local places = ui.persist("demo_sidebar_places", { pinned = { "s-release" }, sections = {} } :: SidebarTypes.Places)
					local details: SidebarTypes.Details = {
						branch = ui.persist("demo_sidebar_show_branch", true),
						pr = ui.persist("demo_sidebar_show_pr", true),
						harness = ui.persist("demo_sidebar_show_harness", true),
						project_icon = ui.persist("demo_sidebar_show_project_icon", true),
						location = ui.persist("demo_sidebar_show_location", true),
					}
					local MARA: SidebarTypes.User = { id = "mara", name = "Mara Okafor" }
					-- The sidebar, or (`island`) its rail.
					local function sidebar(island: boolean, lots: boolean?): Node
						return ds.Sidebar {
							compact = compact,
							organize = organize,
							sort = sort,
							places = places,
							details = details,
							here = "MacBook Pro",
							collapsed = collapsed,
							island = island,
							width = side_w,
							selected = chosen,
							pinned = pinned,
							projects = projects_for(lots == true),
							user = MARA,
							top = TITLE_H,
							on_new = function()
								chosen:set(nil)
							end,
						}
					end
					local effect = ui.persist("demo_sidebar_effect", "dither")
					if effect:peek() == "blur" then
						effect:set("none")
					end
					-- Faded as Zeron fades its wallpaper: from the top, full, to
					-- nothing at the bottom, across its whole height.
					local faded = ui.persist("demo_sidebar_faded", true)
					-- How strongly an effect draws over the picture, and (ASCII) how
					-- many cells hold a character.
					local intensity = ui.persist("demo_sidebar_intensity", "medium")
					local density = ui.persist("demo_sidebar_density", "medium")
					-- Where the shell is: a new chat (the page's own route), or a
					-- session (`sidebar_session`), in the workbench's one router.
					-- Back and forward step through those opened: the title bar's
					-- buttons, the mouse's and a trackpad's over the page, ⌘[ and ⌘].
					-- Choosing in the sidebar moves the route (arriving with one
					-- chosen, in place: no step back to a blank page); going back or
					-- forward moves the choice.
					ui.effect(function()
						local id = chosen:get()
						local now = router.route:peek()
						if page_of(now.name) ~= "sidebar" then
							return
						end
						if id == nil then
							if now.name ~= "sidebar" then
								router.navigate("sidebar")
							end
						elseif now.name == "sidebar" then
							router.replace("sidebar_session", { id = id })
						elseif now.params.id ~= id then
							router.navigate("sidebar_session", { id = id })
						end
					end)
					ui.effect(function()
						local now = router.route:get()
						if now.name == "sidebar_session" then
							if chosen:peek() ~= now.params.id then
								chosen:set(now.params.id)
							end
						elseif now.name == "sidebar" and chosen:peek() ~= nil then
							chosen:set(nil)
						end
					end)
					-- The page: its wallpaper (to the window's top, under the title
					-- bar), the session open over it on a frosted card; an effect
					-- draws over the picture.
					local px = ui.state("demo_sidebar_px", 0.5)
					local py = ui.state("demo_sidebar_py", 0.5)
					local page_ref = ui.ref()
					-- How the composer travels: Zeron's critically damped dock
					-- spring (ω ≈ 27, between its 0.42 s and 0.47 s).
					local DOCK: SpringOptions = { stiffness = 730, damping = 54 }
					local dock_ref = ui.ref()
					-- The composer, as the gallery's composer page has it.
					-- The project a new chat works in, from the sidebar's projects.
					local project = ui.state("demo_sidebar_project", "zeron" :: string?)
					local project_choices: { ComposerTypes.Project } = {}
					for _, p in projects do
						table.insert(project_choices, { value = p.id, label = p.name, hue = p.hue, src = p.src })
					end
					local function composer(placeholder: string, below: (boolean | () -> boolean)?): Node
						return ds.Composer {
							value = draft,
							placeholder = placeholder,
							-- A new chat's opens out, as the app's (and Zeron's) does.
							expanded = below,
							menus_below = below,
							project = project,
							projects = project_choices,
							on_new_project = function() end,
							running = running,
							access = access,
							plan = plan,
							model = model,
							effort = effort,
							fast = fast,
							harnesses = HARNESSES,
							models = MODELS,
							device = device,
							devices = DEVICES,
							checkout = checkout,
							branch = branch,
							branches = BRANCHES,
							context = 0.66,
							on_stop = function()
								turn:set("idle")
							end,
							on_send = function()
								turn:set("running")
							end,
						}
					end
					-- (The art is a new chat's canvas, as Zeron's: a session's page
					-- has none.)
					local function new_chat(): boolean
						return chosen:get() == nil
					end
					-- A picture and its effect (ds.Wallpaper, as the main window's new
					-- chat has it), laid from the page's right edge at the window's
					-- width, so it holds still as the sidebar comes and goes.
					local art = ds.Wallpaper {
						key = function(): string
							local w = wallpaper:get()
							return if w == "none" then "" else w
						end,
						content = function(scene: string): Node
							return ds.wallpaper.Picture { src = `assets/cards/{scene}/sky.webp`, w = 576, h = 768 }
						end,
						-- (The effects work on the picture's grid.)
						picture = function(): any
							return { w = 576, h = 768 }
						end,
						effect = effect,
						shown = new_chat,
						intensity = intensity,
						density = density,
						faded = faded,
						width = 820,
					}
					local pane = ui.column {
						ref = page_ref,
						grow = 1,
						min_w = 0,
						h_full = true,
						relative = true,
						overflow = "hidden",
						on_pointer_move = function(e)
							local w = page_ref.width or 1
							local h = page_ref.height or 1
							px:set(math.clamp((e.local_x or 0) / w, 0, 1))
							py:set(math.clamp((e.local_y or 0) / h, 0, 1))
						end,
						on_hover = function(inside: boolean)
							if not inside then
								px:set(0.5)
								py:set(0.5)
							end
						end,
						art,
						-- A session: what it is (the composer docked at its foot),
						-- following the session chosen; a new chat, the art alone
						-- (the composer floats over it).
						ui.presence(function(): string?
							return if by_id[chosen:get() or ""] then "session" else nil
						end, function(): Node
							local last = by_id[chosen:peek() or ""]
							local function session(): SidebarTypes.Session
								local s = by_id[chosen:get() or ""]
								if s then
									last = s
								end
								-- (This shows only while a session is chosen, so there is one.)
								return last :: SidebarTypes.Session
							end
							return ui.column {
								absolute = true,
								left = 0,
								right = 0,
								top = TITLE_H,
								bottom = 0,
								px = "$space.6",
								pt = "$space.6",
								pb = "$space.4",
								gap = "$space.2",
								ds.Text {
									function(): string
										return session().title
									end,
									style = "heading",
								},
								-- (Its status a glyph of its kind; working, for 47s.)
								ui.presence(function(): ds.StatusKind?
									return session().status
								end, function(kind: ds.StatusKind): Node
									return ds.Status {
										kind = kind,
										since = if kind == "working" then os.time() - 47 else nil,
									}
								end),
								ds.Text {
									function(): string
										local s = session()
										return `{s.device} · {s.branch}`
									end,
									style = "caption",
									tone = "tertiary",
								},
							}
						end, { absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
						-- The composer: one, travelling between the pages as Zeron's
						-- does (crates/ui/src/composer_dock.rs): docked at a session's
						-- foot, lifted to a little over the middle of a new chat, its
						-- width easing between the two, on one critically damped
						-- spring (ω = 12 / its time: 0.42 s docking, 0.47 s rising).
						-- What's typed travels with it.
						ui.row {
							absolute = true,
							left = 0,
							right = 0,
							bottom = 16,
							px = "$space.6",
							justify = "center",
							y = ui.spring(ui.computed(function(): number
								if not new_chat() then
									return 0
								end
								local page = page_ref.height or 0
								local tall = dock_ref.height or 52
								-- A new chat's: centred in the page under the title bar,
								-- lifted a little over the middle.
								local hero = TITLE_H + (page - TITLE_H - 96) / 2
								local docked = page - 16 - tall / 2
								return hero - docked
							end), DOCK),
							ui.div {
								ref = dock_ref,
								w_full = true,
								max_w = ui.spring(ui.computed(function(): number
									return if new_chat() then 560 else 640
								end), DOCK),
								composer("Ask anything, @ to mention…", new_chat),
							},
						},
					}
					return ui.column {
						gap = "$space.4",
						w_full = true,
						h_full = true,
						row(
							ds.Segmented {
								value = compact,
								options = { { value = true, label = "Compact" }, { value = false, label = "Regular" } },
							},
							ds.Segmented {
								value = collapsed,
								options = { { value = false, label = "Open" }, { value = true, label = "Collapsed" } },
							},
							ds.Segmented {
								value = side_mode,
								options = { { value = "rail", label = "Rail" }, { value = "hidden", label = "Hidden" } },
							},
							ds.Segmented {
								value = many,
								options = { { value = false, label = "Few" }, { value = true, label = "Many" } },
							}
						),
						row(
							ds.Segmented {
								value = wallpaper,
								options = {
									{ value = "none", label = "None" },
									{ value = "violet-dusk", label = "Dusk" },
									{ value = "aurora", label = "Aurora" },
									{ value = "blossom", label = "Blossom" },
								},
							},
							ds.Segmented {
								value = effect,
								options = {
									{ value = "none", label = "None" },
									{ value = "dither", label = "Dither" },
									{ value = "ascii", label = "ASCII" },
									{ value = "halftone", label = "Halftone" },
									{ value = "scanlines", label = "Scanlines" },
								},
							},
							ds.Segmented {
								value = faded,
								options = { { value = true, label = "Faded" }, { value = false, label = "Full" } },
							}
						),
						row(
							ds.Segmented {
								value = intensity,
								options = {
									{ value = "subtle", label = "Subtle" },
									{ value = "medium", label = "Medium" },
									{ value = "strong", label = "Strong" },
								},
							},
							ds.Segmented {
								value = density,
								options = {
									{ value = "sparse", label = "Sparse" },
									{ value = "medium", label = "Medium" },
									{ value = "dense", label = "Dense" },
								},
							}
						),
						ui.column {
							w_full = true,
							grow = 1,
							min_h = 0,
							rounded = "$radius.xl",
							overflow = "hidden",
							border = 1,
							border_color = "$border.subtle",
							-- No fill: the gallery's own window shows through (frosted
							-- glass or opaque, as it is set), as the sidebar sits in
							-- the app.
							relative = true,
							-- The sidebar runs to the window's top (its hairline with
							-- it); the page beside it, its wallpaper to the top too; the
							-- title bar laid over both.
							ui.row {
								grow = 1,
								min_h = 0,
								-- (Built again as the data changes.)
								ui.presence(function(): boolean
									return many:get()
								end, function(lots: boolean): Node
									return sidebar(false, lots)
								end, { h_full = true }),
								pane,
							},
							-- The rail, while it collapses to one: floating over the page at
							-- the window's bottom left.
							ui.div {
								absolute = true,
								-- The island's inset from the window's edges, as the
								-- title bar's island has it.
								left = ds.island.metrics().inset,
								bottom = ds.island.metrics().inset,
								ui.show(function(): boolean
									return side_mode:get() == "rail"
								end, ui.presence(function(): boolean
									return many:get()
								end, function(lots: boolean): Node
									return sidebar(true, lots)
								end)),
							},
							ui.div {
								absolute = true,
								left = 0,
								right = 0,
								top = 0,
								-- (Over the sidebar's top: the pointer goes through to it,
								-- but for the bar's own controls.)
								pointer_events = "none",
								ds.TitleBar {
									collapsed = collapsed,
									lights = true,
									-- As Zeron's: only over a new chat's art.
									island = function(): boolean
										return collapsed:get() and new_chat() and wallpaper:get() ~= "none"
									end,
									can_back = function(): boolean
										return router.can_back:get()
									end,
									can_forward = function(): boolean
										return router.can_forward:get()
									end,
									on_back = router.back,
									on_forward = router.forward,
									-- The page's own controls at the bar's other end: a
									-- session's files' toggle, as the main window's.
									actions = ui.show(function(): boolean
										return not new_chat()
									end, ds.IconButton {
										icon = "folder-with-files",
										label = "Files",
										tooltip_side = "bottom",
										w = ds.island.metrics().control,
										h = ds.island.metrics().control,
										lit = demo_files,
										on_click = function()
											demo_files:set(not demo_files:peek())
										end,
									}),
								},
							},
						},
					}
				end,
			},
		},
	},
	{
		title = "Commands",
		pages = { context_menu_page :: Page, command_palette_page :: Page },
	},
	{
		title = "Conversation",
		pages = {
			transcript_page :: Page,
			-- A session's side chats and subagents (demos/session_lanes.luau),
			-- loaded on its own so a slip there shows in its place.
			(function(): Page
				local ok, result = pcall(function()
					return require("@demos/session_lanes")
				end)
				if ok then
					return result
				end
				return {
					id = "session-lanes",
					title = "Session lanes",
					about = "This page didn't load.",
					demo = function()
						return ui.text { tostring(result), literal = true, wrap = true, font = "$font.mono", text_size = "$font.size.sm", color = "$danger" }
					end,
				}
			end)(),
			-- LaTeX math (demos/math.luau), loaded on its own so a slip there
			-- shows in its place.
			(function(): Page
				local ok, result = pcall(function()
					return require("@demos/math")
				end)
				if ok then
					return result
				end
				return {
					id = "math",
					title = "Math",
					about = "This page didn't load.",
					demo = function()
						return ui.text { tostring(result), literal = true, wrap = true, font = "$font.mono", text_size = "$font.size.sm", color = "$danger" }
					end,
				}
			end)(),
		},
	},
	{
		title = "Code",
		pages = { syntax_page :: Page, files_page :: Page, code_editor_page :: Page },
	},
	{
		title = "Containers",
		pages = {
			{
				id = "browser",
				title = "Browser",
				about = "The panel's browser tab: the address bar, a page, and annotation mode (the cursor, ⌘⇧I), whose picks are chips in the composer and numbered marks on the page.",
				fill = true,
				demo = function()
					-- (Loaded on its own: a fault in the tab leaves the gallery up.)
					local ok, built = pcall(function(): Node
						local Browser = require("@app/screens/panel/browser")
						local state = require("@app/screens/panel/state")
						local Cargo = require("@ds/composer_cargo")
						local Annotation = require("@ds/annotation")
						local key = "project:gallery-browser"
						local tabs = state.of(key).tabs
						local tab = if #tabs > 0 then tabs[1].id else state.open(key, "browser", { url = "https://example.com" })
						-- Three picks, as the composer would carry them.
						local function pick(index: number, element: string, text: string, note: string?): { [string]: any }
							local chip = Annotation.chip({
								index = index,
								url = "https://example.com/",
								title = "Example Domain",
								element = element,
								selector = element,
								path = { "body" },
								role = "",
								name = "",
								text = text,
								html = `<{element}>{text}</{element}>`,
								rect = { 395, 166, 416, 77 },
								viewport = { 1205, 598 },
								styles = {},
							}, `gallery-pick-{index}`)
							chip.note = note
							return chip
						end
						local items = ui.state("gallery_browser_picks", {
							pick(1, "h1", "Example Domain"),
							pick(2, "p", "This domain is for use in documentation examples without needing permission."),
							pick(3, "p.more", "", "tighten this copy"),
						} :: { any })
						return ui.column {
							w_full = true,
							grow = 1,
							min_h = 0,
							gap = "$space.3",
							ui.column {
								w_full = true,
								grow = 1,
								min_h = 0,
								rounded = "$radius.xl",
								border = 1,
								border_color = "$border",
								overflow = "hidden",
								Browser.Browser { key = key, tab = tab, w_full = true, grow = 1, min_h = 0 },
							},
							block("CHIPS, AS THE COMPOSER CARRIES THEM", Cargo.Strip {
								items = items,
								on_remove = function(id: string)
									local left = {}
									for _, it in items:peek() do
										if it.id ~= id then
											table.insert(left, it)
										end
									end
									items:set(left)
								end,
							}),
						}
					end)
					if ok then
						return built
					end
					return ds.Text { `The browser page didn't load: {built}`, style = "caption", tone = "danger" }
				end,
			},
			{
				id = "surfaces",
				title = "Surfaces",
				about = "What content sits in when it doesn't sit straight on the window.",
				demo = function()
					local function sample(kind: ds.SurfaceKind, label: string): Node
						return ds.Surface {
							kind = kind,
							w = 140,
							h = 88,
							p = "$space.3",
							ds.Text { label, style = "caption" },
						}
					end
					return block("KINDS", row(sample("raised", "Raised"), sample("sunken", "Sunken"), sample("overlay", "Overlay")))
				end,
			},
			{
				id = "selection",
				title = "Selection",
				about = "A wash on hover; selected, its own fill and ring.",
				demo = function()
					local cards: { Node } = {}
					for i = 1, 3 do
						table.insert(
							cards,
							ds.Surface {
								selected = function()
									return picked:get() == i
								end,
								on_click = function()
									picked:set(i)
								end,
								w = 140,
								p = "$space.3",
								gap = "$space.1",
								ds.Text { `Option {i}`, style = "heading" },
								ds.Text {
									function()
										return if picked:get() == i then "Selected" else "Hover me"
									end,
									style = "caption",
									tone = "secondary",
								},
							}
						)
					end
					return block("OPTIONS", row(table.unpack(cards)))
				end,
			},
		},
	},
	{
		title = "Brand",
		pages = {
			{
				id = "card",
				title = "Member card",
				about = "A member's card: their number, a sky in characters, their badges. Move the pointer over one.",
				demo = function()
					-- A scene from assets/cards (manifest.json): its sky, then its
					-- layers toward the viewer.
					local function scene(slug: string, back: string, ...: string): { ds.CardLayer }
						if slug == "salt-flats" then
							return ds.CardScenes.layers(14)
						end
						local layers: { ds.CardLayer } = { { src = `assets/cards/{slug}/{back}.webp`, depth = 0 } }
						for i, name in { ... } do
							table.insert(layers, { src = `assets/cards/{slug}/{name}.webp`, depth = i })
						end
						return layers
					end
					-- A card from a seed: type any id (a member's) and see the card
					-- it makes, the same at every launch and wherever it's drawn
					-- (ds/card_seed): its scene among the stored count, its foil,
					-- and where its character field starts.
					local function seeded(): Node
						local id = ui.persist("card_seed_demo", "user_mara")
						-- The count stored with this member when their card was made:
						-- twenty, so a scene added since (Black Hole was the 21st)
						-- leaves their card where it is. A member made today gets
						-- today's count, ds.CardScenes.COUNT.
						local STORED = 20
						return block(
							"FROM A SEED",
							ui.row {
								gap = "$space.6",
								items = "start",
								flex_wrap = true,
								ui.column {
									w = 320,
									gap = "$space.3",
									ds.TextField { value = id, label = "Seed", placeholder = "A member's id", w_full = true },
									ds.Text {
										function(): string
											local c = ds.CardSeed.pick(id:get(), STORED)
											return `Scene {c.scene + 1} of the {STORED} stored with them ({ds.CardScenes.COUNT} today), {ds.CardScenes.slug(c.scene)} · {c.holo} · sky {c.sky}`
										end,
										style = "caption",
										tone = "secondary",
										wrap = true,
									},
								},
								ui.presence(id, function(seed: string): Node
									return ds.MemberCard { number = 1378, name = "Mara", since = "Oct 30, 2026", seed = seed, scenes = STORED }
								end, { enter = { opacity = { 0, 1 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
							}
						)
					end
					return ui.column {
						gap = "$space.6",
						seeded(),
						row(
						ds.MemberCard {
							number = 4182,
							name = "Graham",
							tagline = "Methodical big shot",
							since = "Oct 4, 2023",
							layers = scene("violet-dusk", "sky", "far", "near"),
							art_number = 1,
							art_title = "VIOLET DUSK",
							finish = "VELVET ASCENT",
							holo = "aurora",
							badges = {
								{ label = "Cohort 01", glyph = "#", style = "foil" },
								{ label = "First 500", glyph = "@", style = "ink" },
								{ label = "Beta", style = "outline" },
							},
						},
						ds.MemberCard {
							number = 117,
							name = "Ada",
							tagline = "Ships at dawn",
							since = "Jan 12, 2024",
							layers = scene("aurora", "sky", "far", "near"),
							art_number = 2,
							art_title = "AURORA",
							finish = "POLAR WHISPER",
							holo = "secret",
							badges = { { label = "Early access", style = "ink" } },
						},
						ds.MemberCard {
							number = 51,
							name = "Lin",
							tagline = "Night owl",
							since = "Mar 3, 2024",
							layers = scene("dunes", "sky", "detail", "far", "near"),
							art_number = 5,
							art_title = "MOON DUNES",
							finish = "SILKEN DRIFT",
							holo = "starlight",
						},
						ds.MemberCard {
							number = 2,
							name = "Mara",
							tagline = "Founder",
							since = "Jun 1, 2023",
							layers = scene("blossom", "sky", "far", "near", "detail"),
							art_number = 3,
							art_title = "BLOSSOM",
							finish = "PETAL REVERIE",
							holo = "glyph",
							badges = {
								{ label = "Founder", glyph = "@", style = "foil" },
								{ label = "Year 3", style = "ink" },
								{ label = "Core team", style = "outline" },
							},
						},
						ds.MemberCard {
							number = 990,
							name = "Mia",
							tagline = "Quietly prolific",
							since = "Aug 19, 2024",
							layers = scene("lighthouse", "sky", "far", "near"),
							art_number = 4,
							art_title = "LIGHTHOUSE",
							finish = "PEARL BEACON",
							holo = "reverse",
							badges = {
								{ label = "Year 1", style = "outline" },
								{ label = "Invited", style = "ink" },
							},
						},
						ds.MemberCard {
							number = 233,
							name = "Theo",
							tagline = "Reads every diff",
							since = "Feb 2, 2024",
							layers = scene("canyon", "back", "mid", "near", "detail"),
							art_number = 6,
							art_title = "CANYON",
							finish = "AMBER ECHO",
							holo = "aurora",
							badges = { { label = "Cohort 02", glyph = "#", style = "foil" }, { label = "Beta", style = "outline" } },
						},
						ds.MemberCard {
							number = 1408,
							name = "Noor",
							tagline = "Ships quietly",
							since = "Apr 9, 2024",
							layers = scene("ravine", "back", "mid", "near", "detail"),
							art_number = 7,
							art_title = "RAVINE",
							finish = "JADE WHISPER",
							holo = "glyph",
							badges = { { label = "Year 1", style = "ink" } },
						},
						ds.MemberCard {
							number = 77,
							name = "Kai",
							tagline = "Always in the terminal",
							since = "Nov 20, 2023",
							layers = scene("forest", "back", "mid", "near", "detail"),
							art_number = 8,
							art_title = "FOREST",
							finish = "GILDED HUSH",
							holo = "starlight",
							badges = { { label = "First 500", glyph = "@", style = "ink" }, { label = "Year 2", style = "outline" } },
						},
						ds.MemberCard {
							number = 3021,
							name = "Iris",
							tagline = "Calm under deadlines",
							since = "Jul 7, 2024",
							layers = scene("bamboo", "back", "mid", "near", "detail"),
							art_number = 9,
							art_title = "BAMBOO",
							finish = "EMERALD STILLNESS",
							holo = "secret",
							badges = { { label = "Invited", style = "outline" } },
						},
						ds.MemberCard {
							number = 512,
							name = "Juno",
							tagline = "Deep work",
							since = "Dec 1, 2023",
							layers = scene("kelp", "back", "mid", "near", "detail"),
							art_number = 10,
							art_title = "KELP",
							finish = "TIDAL REVERIE",
							holo = "reverse",
							badges = { { label = "Early access", style = "foil" }, { label = "Year 2", style = "ink" } },
						},
						ds.MemberCard {
							number = 811,
							name = "Sage",
							since = "Oct 30, 2026",
							layers = scene("lotus", "back", "far", "near"),
							art_number = 11,
							art_title = "LOTUS",
							finish = "OPAL STILLNESS",
							holo = "glyph",
						},
						ds.MemberCard {
							number = 884,
							name = "Luca",
							since = "Oct 30, 2026",
							layers = scene("icebergs", "back", "far", "near"),
							art_number = 12,
							art_title = "ICEBERGS",
							finish = "GLACIAL PRISM",
							holo = "secret",
						},
						ds.MemberCard {
							number = 957,
							name = "Ari",
							since = "Oct 30, 2026",
							layers = scene("wheat", "back", "far", "near"),
							art_number = 13,
							art_title = "WHEAT",
							finish = "HONEY CADENCE",
							holo = "reverse",
						},
						ds.MemberCard {
							number = 1030,
							name = "Ember",
							since = "Oct 30, 2026",
							layers = scene("volcano", "back", "far", "near"),
							art_number = 14,
							art_title = "CALDERA",
							finish = "EMBER VEIL",
							holo = "aurora",
						},
						ds.MemberCard {
							number = 1103,
							name = "Rose",
							since = "Oct 30, 2026",
							layers = scene("salt-flats", "back", "far", "near"),
							art_number = 15,
							art_title = "SALT BLOOM",
							finish = "ROSE MIRAGE",
							holo = "starlight",
						},
						ds.MemberCard {
							number = 1176,
							name = "Ren",
							since = "Oct 30, 2026",
							layers = scene("terraces", "back", "far", "near"),
							art_number = 16,
							art_title = "RICE TERRACES",
							finish = "VERDANT RHYTHM",
							holo = "glyph",
						},
						ds.MemberCard {
							number = 1249,
							name = "Vera",
							since = "Oct 30, 2026",
							layers = scene("crystals", "back", "far", "near"),
							art_number = 17,
							art_title = "CRYSTAL HOLLOW",
							finish = "AMETHYST CHIME",
							holo = "secret",
						},
						ds.MemberCard {
							number = 1322,
							name = "Cove",
							since = "Oct 30, 2026",
							layers = scene("surf", "back", "far", "near"),
							art_number = 18,
							art_title = "SEA GLASS",
							finish = "FOAM ARABESQUE",
							holo = "reverse",
						},
						ds.MemberCard {
							number = 1395,
							name = "Sol",
							since = "Oct 30, 2026",
							layers = scene("mycelium", "back", "far", "near"),
							art_number = 19,
							art_title = "MYCELIUM",
							finish = "LUMEN HUSH",
							holo = "aurora",
						},
						ds.MemberCard {
							number = 1468,
							name = "Nova",
							since = "Oct 30, 2026",
							layers = scene("saturnine", "back", "far", "near", "detail"),
							art_number = 20,
							art_title = "RINGED SILENCE",
							finish = "CELESTIAL LILT",
							holo = "starlight",
						},
						ds.MemberCard {
							number = 1541,
							name = "Nox",
							since = "Oct 30, 2026",
							layers = scene("black-hole", "back", "far", "near"),
							art_number = 21,
							art_title = "BLACK HOLE",
							finish = "EVENTIDE HALO",
							holo = "glyph",
						},
						ds.MemberCard {
							number = 1614,
							name = "Aster",
							since = "Oct 30, 2026",
							layers = scene("ruins", "back", "far", "near"),
							art_number = 22,
							art_title = "LOST COURT",
							finish = "MOSS REMEMBRANCE",
							holo = "secret",
						},
						ds.MemberCard {
							number = 1687,
							name = "Jun",
							since = "Oct 30, 2026",
							layers = scene("tram", "back", "far", "near"),
							art_number = 23,
							art_title = "SUMMER TRAM",
							finish = "SUMMER CURRENT",
							holo = "reverse",
						},
						ds.MemberCard {
							number = 1760,
							name = "Mira",
							since = "Oct 30, 2026",
							layers = scene("rooftops", "back", "far", "near"),
							art_number = 24,
							art_title = "ROOFTOPS",
							finish = "TERRACOTTA HUSH",
							holo = "aurora",
						},
						ds.MemberCard {
							number = 1833,
							name = "Rowan",
							since = "Oct 30, 2026",
							layers = scene("aqueduct", "back", "far", "near"),
							art_number = 25,
							art_title = "STONE SPAN",
							finish = "PEARL PASSAGE",
							holo = "starlight",
						},
						ds.MemberCard {
							number = 1906,
							name = "Lumi",
							since = "Oct 30, 2026",
							layers = scene("station", "back", "far", "near"),
							art_number = 26,
							art_title = "LAST STATION",
							finish = "LANTERN INTERLUDE",
							holo = "glyph",
						}
						),
					}
				end,
			},
			{
				id = "marks",
				title = "Marks",
				about = "The agents Zeron runs, each on its tile.",
				demo = function()
					return block(
						"MARKS",
						row(
							ds.Mark { name = "claude" },
							ds.Mark { name = "openai" },
							ds.Mark { name = "cursor" },
							ds.Mark { name = "grok" },
							ds.Mark { name = "opencode" },
							ds.Mark { name = "antigravity" }
						),
						row(ds.Mark { name = "claude", size = 28 }, ds.Mark { name = "claude", size = 40 }, ds.Mark { name = "claude", size = 56 })
					)
				end,
			},
		},
	},
	{
		title = "Progress",
		pages = {
			{
				id = "steps",
				title = "Steps",
				about = "Where a flow is: a dot per step, the current one drawn out.",
				demo = function()
					return block(
						"STEPS",
						ds.Steps { count = 4, current = picked },
						row(
							ds.Button {
								"Back",
								tone = "ghost",
								size = "sm",
								on_click = function()
									picked:set(math.max(1, picked:peek() - 1))
								end,
							},
							ds.Button {
								"Next",
								tone = "secondary",
								size = "sm",
								on_click = function()
									picked:set(math.min(4, picked:peek() + 1))
								end,
							}
						)
					)
				end,
			},
			{
				id = "statuses",
				title = "Statuses",
				about = "What an agent is doing: a glyph and a name in a chat's header, a dot in a list.",
				demo = function()
					-- Working for 47 seconds when the page opened.
					local started = os.time() - 47
					local header: { Node } = {}
					local HEADER_KINDS: { ds.StatusKind } = { "working", "waiting", "input", "approval", "failed", "limited", "done", "plan", "connecting" }
					for _, kind in HEADER_KINDS do
						table.insert(header, ds.Status { kind = kind, since = if kind == "working" then started else nil })
					end
					-- A sidebar's sessions, each with its dot.
					local sessions: { { kind: ds.StatusKind, title: string } } = {
						{ kind = "approval", title = "Ship the release notes" },
						{ kind = "input", title = "Which branch should this rebase onto?" },
						{ kind = "working", title = "Port the effort slider to Android" },
						{ kind = "connecting", title = "Fix the flaky local_first test" },
						{ kind = "waiting", title = "Wait for CI on the edge worker" },
						{ kind = "plan", title = "Plan the composer rewrite" },
						{ kind = "done", title = "Rename the presence room" },
					}
					local rows: { Node } = {}
					for _, s in sessions do
						table.insert(
							rows,
							ui.row {
								w = 300,
								h = 28,
								px = "$space.2",
								gap = "$space.2",
								items = "center",
								rounded = "$radius.sm",
								hover = { bg = "$hover" },
								ds.StatusDot { kind = s.kind },
								ds.FadeText { s.title, grow = 1, text_size = "$font.size.md", color = "$text.primary" },
							}
						)
					end
					local tags: { Node } = {}
					local glyphs: { Node } = {}
					local TAG_KINDS: { ds.StatusKind } = { "approval", "input", "working", "connecting", "waiting", "plan", "done", "failed", "limited" }
					for _, kind in TAG_KINDS do
						table.insert(tags, ds.StatusTag { kind = kind })
					end
					local GLYPH_KINDS: { ds.StatusKind } = { "working", "input", "approval", "failed", "done" }
					for _, kind in GLYPH_KINDS do
						table.insert(glyphs, ds.StatusIcon { kind = kind, size = 20 })
					end
					return ui.column {
						gap = "$space.6",
						block("IN A CHAT'S HEADER", ui.column { gap = "$space.3", table.unpack(header) }),
						block("IN A LIST", ui.column { gap = "$zeron.list.gap", table.unpack(rows) }),
						block("WITH THEIR NAMES", row(table.unpack(tags))),
						block("GLYPHS", row(table.unpack(glyphs))),
					}
				end,
			},
		},
	},
}

for _, extra in { patterns_groups, primitives_groups } do
	for _, g in extra :: { Group } do
		table.insert(GROUPS, g)
	end
end
local PAGES: { [string]: Page } = {}
-- Where the workbench is: a route per page. ⌘[ and ⌘] step back and
-- forward through the pages visited, as do the mouse's back and forward
-- buttons and a trackpad's swipe over the page.
local ROUTES: { [string]: string } = {}
for _, group in GROUPS do
	for _, p in group.pages do
		PAGES[p.id] = p
		ROUTES[p.id] = `/{p.id}`
	end
end
-- (The App shell's sessions: its page, a session chosen.)
ROUTES.sidebar_session = "/sidebar/session/:id"
router = ui.router {
	routes = ROUTES,
	initial = ROUTES[page:peek()] or "/buttons",
	scheme = "zeron-gallery",
}
-- `page` follows the route, so the page is kept across launches; set from
-- outside (as devtools set it), it moves the route.
ui.effect(function()
	local id = page_of(router.route:get().name)
	if page:peek() ~= id then
		page:set(id)
	end
end)
ui.effect(function()
	local id = page:get()
	if PAGES[id] and page_of(router.route:peek().name) ~= id then
		router.navigate(id)
	end
end)

-- The sidebar: the groups and their pages.
local function nav(): Node
	local items: { Node } = {}
	for i, group in GROUPS do
		table.insert(
			items,
			ds.SectionTitle {
				group.title,
				px = "$space.2",
				mt = if i == 1 then nil else "$space.3",
				mb = "$space.1",
			}
		)
		for _, p in group.pages do
			table.insert(
				items,
				ds.Surface {
					selected = function()
						return on_page(p.id)
					end,
					on_click = function()
						router.navigate(p.id)
					end,
					depth = "flat",
					rounded = "$radius.md",
					px = "$space.2",
					py = "$space.1",
					ds.Text { p.title },
				}
			)
		end
	end
	return ui.column {
		w = function()
			return nav_w:get()
		end,
		h_full = true,
		shrink = 0,
		overflow = "hidden",
		border_sides = { 0, 1, 0, 0 },
		border_color = "$border.subtle",
		-- Its empty top drags the window too; only its rows take the pointer.
		pointer_events = "none",
		ui.column {
			w = function()
				return sidebar_w:get()
			end,
			h_full = true,
			-- Under the title bar: the window's buttons sit here (and it drags
			-- the window).
			ui.div { h = "$space.7", shrink = 0 },
			-- The pages scroll when they run past the window, each end fading
			-- only while there is more that way.
			ui.scroll {
				w_full = true,
				grow = 1,
				fade = { 16, 16 },
				fade_overflow = true,
				-- (The inset inside: a scroll clips inside its own padding.)
				ui.column { w_full = true, px = "$space.3", pb = "$space.3", gap = "$zeron.list.gap", table.unpack(items) },
			},
		},
	}
end

-- The sidebar's edge: dragged, it resizes the sidebar (and so the page);
-- a hairline lights along it under the pointer and while it is held. The
-- press must land on the edge itself for it to keep the pointer (zgx
-- captures for the node pressed, not its parent), so the hairline takes no
-- pointer of its own.
local GRAB = 10
local function edge(): Node
	local from_x, from_w = 0, 0
	local dragging = ui.state("edge_dragging", false)
	local over = ui.state("edge_over", false)
	return ui.show(
		function()
			return sidebar:get()
		end,
		ui.div {
			absolute = true,
			top = 0,
			bottom = 0,
			w = GRAB,
			left = function()
				return nav_w:get() - GRAB / 2
			end,
			cursor = "col_resize",
			items = "center",
			label = "Resize sidebar",
			on_hover = function(inside: boolean)
				over:set(inside)
			end,
			on_press = function(e)
				from_x, from_w = e.x, sidebar_w:peek()
				dragging:set(true)
			end,
			on_pointer_move = function(e)
				if not dragging:peek() then
					return
				end
				local most = math.max(SIDEBAR_MIN, (root.width or 1000) - PAGE_MIN)
				local w = math.clamp(from_w + e.x - from_x, SIDEBAR_MIN, most)
				sidebar_w:set(w)
				nav_w:set(w)
			end,
			on_release = function()
				dragging:set(false)
			end,
			ui.div {
				w = 1,
				h_full = true,
				pointer_events = "none",
				bg = function()
					return if over:get() or dragging:get() then "$border.strong" else "#00000000"
				end,
			},
		}
	)
end

-- The sidebar's toggle, in the title bar beside the window's buttons
-- whether the sidebar shows or not.
local function toggle(): Node
	return ui.div {
		absolute = true,
		left = TOGGLE_X,
		top = TOGGLE_Y,
		ds.IconButton {
			label = "Toggle sidebar",
			size = "lg",
			-- Below: above it is the window's edge.
			tooltip_side = "bottom",
			on_click = function()
				show_sidebar(not sidebar:peek())
			end,
			ds.SidebarIcon { open = sidebar },
		},
	}
end

-- The header: the page's name, and how the workbench shows it.
local function header(): Node
	return ui.row {
		h = TITLEBAR,
		-- Clear of the window's buttons and the toggle while the sidebar is
		-- hidden; while it shows, space.6, the page's own inset, so the title
		-- and the page share one left edge. Easing with the sidebar's width,
		-- so the title glides with it.
		pl = ui.tween(sidebar, { from = CLEAR_OF_BUTTONS, to = ui.theme.get("space.6") :: number, duration = 200 }),
		pr = "$space.3",
		gap = "$space.2",
		items = "center",
		-- The header is the title bar: its empty space drags the window (the
		-- page scrolls under it, so a strip beneath could not); its controls
		-- take their own clicks.
		window_drag = true,
		ds.Text {
			function()
				return PAGES[page_of(router.route:get().name)].title
			end,
			-- As Zeron's title bar: body size, medium.
			style = "body",
			weight = "$font.weight.medium",
			grow = 1,
		},
		ds.Segmented {
			value = dark,
			label = "Theme",
			segment = 52,
			options = { { value = false, label = "Light" }, { value = true, label = "Dark" } },
		},
		ds.Segmented {
			value = frosted,
			label = "Window",
			segment = 64,
			options = { { value = true, label = "Frosted" }, { value = false, label = "Opaque" } },
		},
	}
end

-- The pages' feet, each shown with its page: across the page's width, at
-- its bottom.
local function feet(): { Node }
	local out: { Node } = {}
	for id, p in PAGES do
		local foot = p.foot
		if foot then
			table.insert(
				out,
				ui.show(
					function()
						return on_page(id)
					end,
					ui.div {
						absolute = true,
						-- (Insets take points, not spacing tokens: the page's
						-- own side padding, and a little off the window's edge.)
						left = 32,
						right = 32,
						bottom = 8,
						foot(),
					}
				)
			)
		end
	end
	return out
end

-- Each page as its route shows it: what it is for, its examples, rising in
-- as it comes.
local views: { [string]: () -> Node } = {}
for id, p in PAGES do
	views[id] = function(): Node
		local fill = p.fill == true
		return ui.column {
			w_full = true,
			h_full = if fill then true else nil,
			px = "$space.6",
			-- Clear of the fade at rest: content starts where it ends.
			pt = ui.raw(if fill then FADE else FADE - HEADER, "where the scroll fade ends"),
			pb = if fill then "$space.6" else "$space.8",
			gap = "$space.6",
			opacity = 0,
			on_mount = function(el)
				anime.animate(el, { opacity = { 0, 1 }, y = { 6, 0 }, duration = 220, ease = "outQuad" })
			end,
			ds.Text { p.about, tone = "secondary", wrap = true },
			if fill then ui.column { w_full = true, grow = 1, min_h = 0, p.demo() } else p.demo(),
		}
	end
end

-- The page shown: its name, what it is for, its examples.
local function content(): Node
	-- The mouse's back and forward buttons and a trackpad's swipe over the
	-- page step the history, as `router.view` would.
	local function navigate(e)
		if e.direction == "back" then
			router.back()
		else
			router.forward()
		end
	end
	-- Keyed by page, not route: choosing a session in the App shell moves
	-- the route without building its page again.
	return ui.presence(function(): string
		return page_of(router.route:get().name)
	end, function(id: string): Node
		if PAGES[id].fill then
			return ui.column { w_full = true, h_full = true, on_navigate = navigate, views[id]() }
		end
		-- The page stops at the header, fading out on the way to its edge as it
		-- scrolls there (at rest nothing is under it), and at the window's
		-- foot.
		return ui.column {
			w_full = true,
			h_full = true,
			pt = ui.raw(HEADER, "the header's height"),
			ui.scroll { w_full = true, grow = 1, min_h = 0, fade = { FADE - HEADER, FADE }, on_navigate = navigate, views[id]() },
		}
	end, { w_full = true, h_full = true })
end

-- The projects of the dropdown at the window's edge.
local EDGE_PROJECTS: { Dropdown.Item } = {
	{ value = "tidepool", label = "tidepool" },
	{ value = "comet-native", label = "comet-native" },
	{ value = "zgx", label = "zgx" },
	{ value = "zui", label = "zui" },
	{ value = "landing", label = "landing" },
}

return ui.overlay {
	id = "gallery",
	ref = root,
	w_full = true,
	h_full = true,
	theme = function()
		return if dark:get() then "zeron-dark" else "zeron-light"
	end,
	bg = function()
		return if frosted:get() then "$background" else "$zeron.window"
	end,
	-- The title bar's strip: it moves the window.
	ui.div { absolute = true, left = 0, right = 0, top = 0, h = TITLEBAR, window_drag = true },
	ui.row {
		w_full = true,
		h_full = true,
		nav(),
		-- The page, with the header over its top.
		ui.overlay {
			grow = 1,
			min_w = 0,
			h_full = true,
			content(),
			feet(),
			ui.div { absolute = true, left = 0, right = 0, top = 0, header() },
		},
	},
	edge(),
	toggle(),
	-- On the dropdown page: one at the window's bottom-right corner, opening
	-- down, so its panel goes past the window (zgx draws popups past it on
	-- macOS; with too little room below, as over the Dock, it flips up).
	ui.show(
		function()
			return on_page("dropdown")
		end,
		ui.div {
			absolute = true,
			-- (Insets take points, not spacing tokens.)
			right = 16,
			bottom = 12,
			ds.Dropdown {
				value = project,
				label = "Project at the window's edge",
				icon = "folder",
				placeholder = "No project",
				search = "Search projects…",
				placement = { side = "bottom", align = "end", offset = 6 },
				items = EDGE_PROJECTS,
				actions = { { label = "New project", icon = "plus" } },
			},
		}
	),
}
