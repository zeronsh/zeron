-- The gallery's Command palette page: ⌘K (while this page shows) or the
-- button opens it over hundreds of sessions, Zeron's actions, places and
-- settings. What it did shows under the button. A page is
-- { id, title, about, demo }: `demo` builds what the page shows.

local CommandPalette = require("@ds/command_palette")
local Button = require("@ds/button")
local Text = require("@ds/text")
local Icon = require("@ds/icon")
local Kbd = require("@ds/kbd")
local material = require("@ds/material")

type Item = CommandPalette.Item

local open = ui.state("palette_open", false)
local recent = ui.state("palette_recent", { "s-onboarding", "a-model", "s-rebase" } :: { string })
-- What the palette last did, said under the button.
local did = ui.state("palette_did", "")
-- The page, while it shows: ⌘K is its own only then.
local page = ui.ref()

-- The demo's own settings, flipped in place from the palette.
local compact = ui.state("palette_compact", false)
local branches = ui.state("palette_branches", true)
local prs = ui.state("palette_prs", true)
local notify = ui.state("palette_notify", true)
local sounds = ui.state("palette_sounds", false)
local model = ui.state("palette_model", "opus")
local effort = ui.state("palette_effort", "high")

local function say(what: string)
	return function()
		did:set(what)
	end
end

-- Projects, and sessions in them: a few written out (the sidebar demo's),
-- then hundreds made up, so the list has a real length to search.
local PROJECTS = {
	{ name = "comet-native", hue = "violet" },
	{ name = "zgx", hue = "sky" },
	{ name = "zui", hue = "teal" },
	{ name = "tidepool", hue = "orange" },
	{ name = "landing", hue = "pink" },
	{ name = "zeron-edge", hue = "emerald" },
}
local DEVICES = { "MacBook Pro", "devbox", "studio", "Cloud" }
local AGENTS = { "claude", "openai", "claude", "cursor", "grok", "claude", "openai" }
local VERBS = { "Fix", "Port", "Speed up", "Rename", "Test", "Tidy", "Profile", "Split", "Document", "Retry", "Cache", "Stream" }
local THINGS = {
	"the presence badge",
	"device room relay",
	"image uploads",
	"the sidebar's drag",
	"model catalog cache",
	"worktree sends",
	"Android emulator boot",
	"auto-updater manifest",
	"edge fade bands",
	"terminal resize",
	"plan mode approvals",
	"iOS fold layout",
	"WorkOS webhook",
	"markdown tables",
	"voice stage",
	"Tailscale MTU",
	"whale doc sidecar",
	"GPU memory stats",
	"session hover card",
	"file search index",
}
local KINDS = { "working", "input", "approval", "done", "failed" }

local function session(id: string, title: string, p: number, s: { [string]: any }): Item
	local project = PROJECTS[p]
	return {
		id = id,
		label = title,
		section = "Sessions",
		session = {
			status = s.status,
			time = s.time,
			agent = s.agent or "claude",
			project = project.name,
			hue = project.hue,
			branch = s.branch,
			device = s.device or "MacBook Pro",
			pr = s.pr,
			archived = s.archived,
		},
		pinned = s.pinned,
		run = say(`Opened “{title}”`),
		alt = { label = "Open beside", run = say(`Opened “{title}” beside this one`) },
	}
end

local now = os.time()
local SESSIONS: { Item } = {
	session("s-onboarding", "Onboarding in zgx", 1, { status = "working", time = now, branch = "zeron/zgx-onboarding", pinned = true, pr = { number = 1094, state = "draft" } }),
	session("s-release", "Release v0.2.130", 1, { status = "approval", time = now - 120, branch = "release/v0.2.130", pinned = true, pr = { number = 1092, state = "open" } }),
	session("s-rebase", "Which branch should this rebase onto?", 2, { status = "input", time = now - 240, agent = "openai", device = "devbox", branch = "main" }),
	session("s-presence", "Rename the presence room", 1, { status = "done", time = now - 720, branch = "presence-rename" }),
	session("s-flaky", "Fix the flaky local_first test", 1, { status = "failed", time = now - 3600, agent = "openai", device = "studio", branch = "fix/local-first" }),
	session("s-ws3", "Room socket goes deaf after auto-pong", 6, { status = "working", time = now - 30, agent = "openai", device = "devbox", branch = "ws3/pong" }),
	session("s-duo", "Fold rebuilds the session page", 1, { status = "input", time = now - 1200, branch = "ios/duo-fold" }),
	session("s-bands", "Faded box outside a popup panics", 2, { time = now - 480, branch = "fix/band-strips" }),
	session("s-pricing", "Pricing page copy", 5, { time = now - 18000, agent = "cursor", branch = "site/pricing" }),
}
-- Made up, but always the same: a little generator of its own.
do
	local seed = 7
	local function rand(n: number): number
		-- (Park and Miller's: its products stay exact in a double.)
		seed = (seed * 16807) % 2147483647
		return (seed // 13) % n + 1
	end
	for i = 1, 3200 do
		local title = `{VERBS[rand(#VERBS)]} {THINGS[rand(#THINGS)]}`
		local p = rand(#PROJECTS)
		local roll = rand(100)
		local slug = string.gsub(string.lower(title), "[^%w]+", "-")
		table.insert(
			SESSIONS,
			session(`s-{i}`, title, p, {
				status = if roll <= 6 then KINDS[rand(#KINDS)] else nil,
				time = now - i * 240 - rand(200),
				agent = AGENTS[rand(#AGENTS)],
				device = DEVICES[rand(#DEVICES)],
				branch = if rand(3) > 1 then `{string.sub(slug, 1, 24)}-{i}` else nil,
				archived = roll > 88,
				pr = if roll > 6 and roll <= 14 then { number = 900 + i % 300, state = ({ "open", "merged", "draft", "closed" })[rand(4)] } else nil,
			})
		)
	end
end

-- The pages the root's rows open.
local MODELS = {
	{ id = "opus", label = "Opus 4.5", mark = "claude" },
	{ id = "sonnet", label = "Sonnet 4.5", mark = "claude" },
	{ id = "haiku", label = "Haiku 4.5", mark = "claude" },
	{ id = "gpt", label = "GPT-5.1 Codex", mark = "openai" },
	{ id = "gpt-mini", label = "GPT-5.1 Codex Mini", mark = "openai" },
	{ id = "grok", label = "Grok 4", mark = "grok" },
	{ id = "composer", label = "Composer 1", mark = "cursor" },
}
local function name_of(list: { any }, id: string): string
	for _, m in list do
		if m.id == id then
			return m.label
		end
	end
	return id
end
local function models_page(): CommandPalette.Page
	local items: { Item } = {}
	for _, m in MODELS do
		table.insert(items, {
			id = `model-{m.id}`,
			label = m.label,
			section = if m.mark == "claude" then "Claude" elseif m.mark == "openai" then "Codex" else "Others",
			mark = m.mark :: any,
			checked = function(): boolean
				return model:get() == m.id
			end,
			verb = "Switch",
			run = function()
				model:set(m.id)
				did:set(`Switched to {m.label}`)
			end,
		})
	end
	return { id = "models", title = "Switch model", placeholder = "Search models…", items = items }
end
local EFFORTS = {
	{ id = "low", label = "Low" },
	{ id = "medium", label = "Medium" },
	{ id = "high", label = "High" },
	{ id = "xhigh", label = "Extra high" },
	{ id = "max", label = "Max" },
}
local function effort_page(): CommandPalette.Page
	local items: { Item } = {}
	for _, e in EFFORTS do
		local item: Item = {
			id = `effort-{e.id}`,
			label = e.label,
			section = "Effort",
			icon = "fast-tier",
			checked = function(): boolean
				return effort:get() == e.id
			end,
			verb = "Set",
			run = function()
				effort:set(e.id)
				did:set(`Reasoning effort: {e.label}`)
			end,
		}
		table.insert(items, item)
	end
	return { id = "effort", title = "Reasoning effort", placeholder = "Search levels…", items = items }
end
local function projects_page(): CommandPalette.Page
	local items: { Item } = {}
	for _, p in PROJECTS do
		table.insert(items, {
			id = `project-{p.name}`,
			label = p.name,
			section = "Projects",
			hue = p.hue,
			verb = "Move",
			run = say(`Moved the session to {p.name}`),
		})
	end
	return { id = "move", title = "Move to project", placeholder = "Search projects…", items = items }
end

-- Branches come from the device the session runs on: a page that loads.
local function branches_page(): CommandPalette.Page
	return {
		id = "branches",
		title = "Switch branch",
		placeholder = "Search branches…",
		loading = { title = "Fetching branches from MacBook Pro" },
		load = function(done)
			ui.after(1100, function()
				local items: { Item } = {}
				for i, b in { "main", "zeron/zgx-onboarding", "release/v0.2.130", "fix/local-first", "presence-rename", "ios/duo-fold", "ws3/pong", "site/pricing" } do
					local item: Item = {
						id = `branch-{i}`,
						label = b,
						section = if i == 1 then "Default" else "Local",
						icon = "git-branch",
						verb = "Check out",
						checked = function(): boolean
							return i == 2
						end,
						run = say(`Checked out {b}`),
					}
					table.insert(items, item)
				end
				done(items)
			end)
		end,
	}
end
-- Pull requests waiting on you: none, today.
local function reviews_page(): CommandPalette.Page
	return {
		id = "reviews",
		title = "Review requests",
		placeholder = "Search review requests…",
		items = {},
		empty = { title = "Nothing to review", hint = "Pull requests that ask for your review show up here." },
	}
end

local COMMANDS: { Item } = {
	{ id = "a-new", label = "New chat", section = "Actions", icon = "pen-new-square", shortcut = "cmd+n", keywords = "session start", run = say("Started a new chat"), alt = { label = "New chat in Cloud", run = say("Started a new chat in Cloud") } },
	{ id = "a-project", label = "New project", section = "Actions", icon = "folder", shortcut = "cmd+shift+n", keywords = "add folder repository", run = say("Opened New project") },
	{
		id = "a-model",
		label = "Switch model",
		section = "Actions",
		icon = "bot",
		keywords = "claude codex opus sonnet gpt harness",
		hint = function(): string
			return name_of(MODELS, model:get())
		end,
		page = models_page,
	},
	{
		id = "a-effort",
		label = "Reasoning effort",
		section = "Actions",
		icon = "fast-tier",
		keywords = "thinking level",
		hint = function(): string
			return name_of(EFFORTS, effort:get())
		end,
		page = effort_page,
	},
	{ id = "a-move", label = "Move session to project", section = "Actions", icon = "map-arrow-right", page = projects_page },
	{ id = "a-branch", label = "Switch branch", section = "Actions", icon = "git-branch", keywords = "checkout git", hint = "zeron/zgx-onboarding", page = branches_page },
	{ id = "a-reviews", label = "Review requests", section = "Actions", icon = "pull-request-draft", keywords = "github pr", page = reviews_page },
	{ id = "a-pr", label = "Open pull request", section = "Actions", icon = "pull-request", keywords = "github review", run = say("Opened #1094 on GitHub"), alt = { label = "Copy link", run = say("Copied the pull request's link") } },
	{ id = "a-copy", label = "Copy session link", section = "Actions", icon = "copy", shortcut = "cmd+shift+c", run = say("Copied the session's link") },
	{ id = "a-archive", label = "Archive session", section = "Actions", icon = "archive-minimalistic", shortcut = "cmd+shift+a", keywords = "close hide", run = say("Archived the session") },
	{ id = "g-settings", label = "Settings", section = "Go to", icon = "settings-minimalistic", shortcut = "cmd+,", keywords = "preferences", verb = "Open", run = say("Opened Settings") },
	{ id = "g-devices", label = "Devices", section = "Go to", icon = "monitor", keywords = "machines hosts", verb = "Open", run = say("Opened Devices") },
	{ id = "g-activity", label = "Activity", section = "Go to", icon = "clock-circle", keywords = "history usage", verb = "Open", run = say("Opened Activity") },
	{ id = "g-notifications", label = "Notifications", section = "Go to", icon = "bell", verb = "Open", run = say("Opened Notifications") },
	{ id = "g-keys", label = "Keyboard shortcuts", section = "Go to", icon = "keyboard", shortcut = "cmd+/", keywords = "keymap bindings", verb = "Open", run = say("Opened Keyboard shortcuts") },
	{ id = "t-frosted", label = "Frosted glass", section = "Settings", icon = "moon", keywords = "material translucent opaque window appearance", toggle = material.frosted },
	{ id = "t-compact", label = "Compact sidebar", section = "Settings", icon = "list", keywords = "density", toggle = compact },
	{ id = "t-branches", label = "Show branches", section = "Settings", icon = "git-branch", keywords = "sidebar", toggle = branches },
	{ id = "t-prs", label = "Show pull requests", section = "Settings", icon = "pull-request", keywords = "sidebar badge", toggle = prs },
	{ id = "t-notify", label = "Notify when an agent needs you", section = "Settings", icon = "bell", keywords = "alerts input approval", toggle = notify },
	{ id = "t-sounds", label = "Play sounds", section = "Settings", icon = "volume-loud", keywords = "audio", toggle = sounds },
}

local ITEMS: { Item } = {}
for _, item in COMMANDS do
	table.insert(ITEMS, item)
end
for _, item in SESSIONS do
	table.insert(ITEMS, item)
end
local ROOT: CommandPalette.Page = { id = "root", title = "Zeron", items = ITEMS }

-- The app's command, for a host that lists commands in its menus. Its key
-- is the page's own (below), so ⌘K stays free on every other page.
ui.command {
	id = "command-palette",
	title = "Command Palette…",
	menu = "View",
	run = function()
		if page.mounted then
			open:set(true)
		end
	end,
}

-- Things to try, so the search shows what it can do.
local TRY = {
	{ "nc", "letters of words' starts find New chat" },
	{ "metal fix", "every word somewhere: the device, then the title" },
	{ "model", "→ or Tab opens Switch model; Backspace comes back" },
	{ "branch", "a page that loads: its rows' shapes, then its branches" },
	{ "review", "a page with nothing in it yet" },
	{ "frost", "Enter flips Frosted glass in place" },
}

local function tries(): { Node }
	local out: { Node } = {}
	for _, t in TRY do
		table.insert(
			out,
			ui.row {
				gap = "$space.3",
				items = "center",
				ui.row {
					h = 22,
					px = "$space.2",
					items = "center",
					rounded = "$radius.sm",
					bg = "$zeron.secondary.bg",
					ui.text { t[1], literal = true, font = "$font.mono", text_size = "$font.size.sm", color = "$text.primary" },
				},
				Text { t[2], style = "caption", tone = "secondary" },
			}
		)
	end
	return out
end

return {
	id = "command-palette",
	title = "Command palette",
	about = "⌘K: go anywhere, do anything. One fuzzy search over sessions, actions, places and settings.",
	demo = function(): Node
		return ui.column {
			ref = page,
			gap = "$space.5",
			focusable = true,
			focus = { outline = 0 },
			-- ⌘K is this page's: heard while focus is in it (it takes focus
			-- as it shows, and back as the palette closes).
			on_key = function(e): boolean
				if (e.meta or e.cmd) and (e.key == "k" or e.key == "K") then
					open:set(not open:peek())
					return true
				end
				return false
			end,
			on_mount = function(el)
				ui.after(0, function()
					el:focus()
				end)
				-- Closed, focus comes back to the page, so ⌘K opens it again.
				-- (An element doesn't say whether it holds focus.)
				ui.effect(function()
					if not open:get() then
						ui.after(0, function()
							if page.mounted then
								el:focus()
							end
						end)
					end
				end)
			end,
			ui.row {
				gap = "$space.3",
				items = "center",
				Button {
					tone = "secondary",
					size = "md",
					label = "Open the command palette",
					on_click = function()
						open:set(true)
					end,
					Icon { name = "palette-search", size = 14 },
					ui.text { "Search or run a command" },
					Kbd { keys = "cmd+k", tone = "quiet" },
				},
				Text {
					function(): string
						local d = did:get()
						return if d == "" then `{#SESSIONS} sessions, {#COMMANDS} commands` else d
					end,
					style = "caption",
					tone = "tertiary",
				},
			},
			ui.column {
				gap = "$space.2",
				Text { "TRY", style = "label", tone = "tertiary" },
				table.unpack(tries()),
			},
			CommandPalette { open = open, root = ROOT, recent = recent },
		}
	end,
}
