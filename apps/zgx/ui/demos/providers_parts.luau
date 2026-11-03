-- Settings → Providers, in the pieces its pages share: the agents on a
-- device and the accounts they sign in with (one card each), the agents
-- still to install, every agent that speaks the Agent Client Protocol (the
-- registry) and the pages that add one, each agent's own settings dialog,
-- and the resets Codex banks for an account. The agents, their order and
-- what each can do are Zeron's (crates/proto/src/providers.rs,
-- settings/accounts.rs, settings/harnesses.rs); the registry is the real
-- one (cdn.agentclientprotocol.com, 43 agents on Oct 30 2026).

local ds = require("@ds")

local M = {}

-- Whose agents the page shows: one of your devices, or Cloud.
M.DEVICES = {
	{ value = "mac", label = "Mara's MacBook Pro", icon = "laptop" },
	{ value = "box", label = "build-box-02", icon = "remote-server" },
	{ value = "cloud", label = "Cloud", icon = "cloud" },
}
M.device = ui.state("ex_pv_device", "mac")
function M.device_name(): string
	for _, d in M.DEVICES do
		if d.value == M.device:get() then
			return d.label
		end
	end
	return "this device"
end

-- Which of the page's views shows: the agents, the registry, one agent of
-- it, or a form for one of your own.
M.view = ui.state("ex_pv_view", "agents")
-- Adding an agent from the registry: not yet, under way, done, or failed.
M.adding = ui.state("ex_pv_adding", "idle")
M.query = ui.state("ex_pv_query", "")

-- Each installed agent's switch, and the account it runs as.
M.enabled = {
	claude = ui.state("ex_pv_on_claude", true),
	codex = ui.state("ex_pv_on_codex", true),
	cursor = ui.state("ex_pv_on_cursor", true),
	grok = ui.state("ex_pv_on_grok", false),
}
M.active = {
	claude = ui.state("ex_pv_active_claude", "mara@halcyon.dev"),
	codex = ui.state("ex_pv_active_codex", "mara@halcyon.dev"),
}
-- A sign-in under way (Codex) and one that failed (Grok), each shown in
-- the row where the account will be.
M.codex_login = ui.state("ex_pv_codex_login", "waiting")
M.grok_login = ui.state("ex_pv_grok_login", "failed")
M.removing = ui.state("ex_pv_removing", false)
-- Codex's banked resets for the account at its limit: still banked, one
-- being used, used, kept (no window needed it), failed (Codex answered for
-- another account), or none banked. (Being chosen is `reset_open`.)
M.codex_reset = ui.state("ex_pv_codex_reset", "banked")
M.reset_open = ui.state("ex_pv_reset_open", false)
M.reset_choice = ui.state("ex_pv_reset_choice", "c1")

-- The agents on this Mac, in Zeron's order, with the version each reports.
-- An agent: its name and mark, the version on this Mac (and a newer one
-- out), or that Zeron can't install it (`manual`).
export type Agent = { id: string, name: string, mark: string, version: string?, latest: string?, manual: boolean? }

local INSTALLED: { Agent } = {
	{ id = "claude", name = "Claude Code", mark = "claude", version = "2.1.14", latest = "2.2.0" },
	{ id = "codex", name = "Codex", mark = "openai", version = "0.163.0" },
	{ id = "cursor", name = "Cursor", mark = "cursor", version = "2026.10.14-3f2c9a1" },
	{ id = "grok", name = "Grok", mark = "grok", version = "1.4.0" },
}
M.INSTALLED = INSTALLED
-- The rest of Zeron's own providers, not on this Mac. Antigravity has no
-- installer Zeron can run.
local MORE: { Agent } = {
	{ id = "antigravity", name = "Antigravity", mark = "antigravity", manual = true },
	{ id = "devin", name = "Devin", mark = "devin" },
	{ id = "opencode", name = "OpenCode", mark = "opencode" },
	{ id = "pi", name = "Pi", mark = "pi" },
	{ id = "hermes", name = "Hermes", mark = "hermes" },
	{ id = "muse", name = "Muse Code", mark = "muse" },
}
M.MORE = MORE
-- Who each agent is signed in as: a plan, and how much of its session and
-- week are used (the two windows the providers report). Codex banks
-- resets for an account; the one at its limit has two.
M.ACCOUNTS = {
	claude = {
		{ email = "mara@halcyon.dev", plan = "Max 20x", session = 0.62, week = 0.31 },
		{ email = "mara@example.net", plan = "Pro", session = 1, week = 0.88, resets_at = "3:45 PM" },
	},
	codex = {
		{
			email = "mara@halcyon.dev",
			plan = "Plus",
			session = 1,
			week = 0.64,
			resets_at = "6:10 PM",
			resets = {
				{ id = "c1", title = "Full reset", scope = "Week and session", expires = "Nov 4" },
				{ id = "c2", title = "Full reset", scope = "Week and session", expires = "Nov 12" },
			},
		},
	},
	cursor = {},
	grok = {},
}

-- Every agent in the registry (its id, name, who makes it, its version,
-- how it installs, its license, what it says of itself), as published. The
-- ones Zeron drives itself are built in.
export type Entry = { id: string, name: string, by: string, version: string, kind: string, license: string, about: string, builtin: boolean?, icon: boolean?, source: string? }
local REGISTRY: { Entry } = {
	{ id = "agoragentic-acp", name = "Agoragentic", by = "ACRE", version = "1.3.0", kind = "npx", license = "MIT", about = "An agent marketplace: browse, invoke and pay for agent services." },
	{ id = "amp-acp", name = "Amp", by = "Sourcegraph", version = "0.10.0", kind = "binary", license = "Apache-2.0", about = "The frontier coding agent, through its ACP wrapper.", icon = true, source = "github.com/sourcegraph/amp" },
	{ id = "antigravity-acp", name = "Google Antigravity", by = "Google", version = "1.4.5", kind = "binary", license = "Proprietary", about = "Google's AI coding agent.", builtin = true },
	{ id = "auggie", name = "Auggie CLI", by = "Augment Code", version = "0.36.0", kind = "npx", license = "Proprietary", about = "Augment's software agent, backed by its context engine.", icon = true },
	{ id = "autohand", name = "Autohand Code", by = "Autohand AI", version = "0.2.1", kind = "npx", license = "Apache-2.0", about = "AI coding agent powered by Autohand AI." },
	{ id = "bob", name = "Bob", by = "IBM", version = "2.0.9", kind = "npx", license = "Proprietary", about = "IBM's AI-powered development partner." },
	{ id = "claude-acp", name = "Claude Agent", by = "Anthropic", version = "0.99.1", kind = "npx", license = "Proprietary", about = "Anthropic's Claude, through its ACP wrapper.", builtin = true },
	{ id = "cline", name = "Cline", by = "Cline Bot Inc.", version = "3.0.74", kind = "npx", license = "Apache-2.0", about = "Autonomous coding agent: edits files, runs commands, uses the browser.", icon = true, source = "github.com/cline/cline" },
	{ id = "codebuddy-code", name = "Codebuddy Code", by = "Tencent Cloud", version = "2.178.1", kind = "npx", license = "Proprietary", about = "Tencent Cloud's official intelligent coding tool." },
	{ id = "codex-acp", name = "Codex", by = "OpenAI", version = "2.5.0", kind = "npx", license = "Apache-2.0", about = "OpenAI's coding assistant, through its ACP adapter.", builtin = true },
	{ id = "cortex-code", name = "Cortex Code", by = "Snowflake", version = "1.0.73", kind = "binary", license = "Proprietary", about = "Snowflake's Cortex Code coding agent." },
	{ id = "corust-agent", name = "Corust Agent", by = "Corust AI", version = "0.6.0", kind = "binary", license = "GPL-3.0", about = "Co-building with a seasoned Rust partner." },
	{ id = "crow-cli", name = "crow-cli", by = "Thomas Wood", version = "0.1.24", kind = "binary", license = "Apache-2.0", about = "A minimal, native ACP coding agent." },
	{ id = "cursor", name = "Cursor", by = "Cursor", version = "2026.10.22", kind = "binary", license = "Proprietary", about = "Cursor's coding agent.", builtin = true },
	{ id = "deepagents", name = "DeepAgents", by = "LangChain", version = "0.1.7", kind = "npx", license = "MIT", about = "Batteries-included coding and general agent, powered by LangChain.", icon = true, source = "github.com/langchain-ai/deepagents" },
	{ id = "devin", name = "Devin", by = "Cognition", version = "3000.11.234", kind = "binary", license = "Proprietary", about = "Devin CLI coding agent by Cognition.", builtin = true },
	{ id = "dimcode", name = "DimCode", by = "ArcShips", version = "0.5.22", kind = "npx", license = "Proprietary", about = "A coding agent that puts leading models at your command." },
	{ id = "dirac", name = "Dirac", by = "Dirac Delta Labs", version = "0.5.24", kind = "npx", license = "Apache-2.0", about = "Cuts API costs with hash-anchored parallel edits." },
	{ id = "factory-droid", name = "Factory Droid", by = "Factory AI", version = "1.0.0", kind = "npx", license = "Proprietary", about = "Factory's AI coding agent.", icon = true },
	{ id = "fast-agent", name = "fast-agent", by = "fast-agent.ai", version = "0.10.1", kind = "uvx", license = "Apache-2.0", about = "Code and build agents with multi-provider support." },
	{ id = "gemini", name = "Gemini CLI", by = "Google", version = "0.66.0", kind = "npx", license = "Apache-2.0", about = "Google's official CLI for Gemini.", icon = true, source = "github.com/google-gemini/gemini-cli" },
	{ id = "github-copilot-cli", name = "Copilot", by = "GitHub", version = "1.0.105", kind = "npx", license = "Proprietary", about = "GitHub's AI pair programmer.", icon = true, source = "github.com/github/copilot-cli" },
	{ id = "glm-acp-agent", name = "GLM Agent", by = "Stefan de Vogelaere", version = "1.17.0", kind = "npx", license = "Apache-2.0", about = "Zhipu AI's GLM Coding Plan models, as an agent." },
	{ id = "goose", name = "goose", by = "Block", version = "1.57.0", kind = "binary", license = "Apache-2.0", about = "A local, extensible, open source agent that automates engineering tasks.", icon = true, source = "github.com/block/goose" },
	{ id = "grok-build", name = "Grok Build", by = "xAI", version = "1.0.73", kind = "npx", license = "Proprietary", about = "xAI's coding agent and CLI.", builtin = true },
	{ id = "harn", name = "Harn", by = "Burin Labs", version = "0.10.181", kind = "binary", license = "Apache-2.0", about = "Runs .harn agent pipelines as a native coding agent." },
	{ id = "junie", name = "Junie", by = "JetBrains", version = "3893.6.0", kind = "binary", license = "Proprietary", about = "JetBrains' AI coding agent.", icon = true },
	{ id = "kilo", name = "Kilo", by = "Kilo Code", version = "7.8.18", kind = "binary", license = "MIT", about = "The open source coding agent.", icon = true, source = "github.com/Kilo-Org/kilocode" },
	{ id = "kimchi", name = "Kimchi", by = "Kimchi", version = "1.25.0", kind = "binary", license = "Apache-2.0", about = "Coding agent powered by multi-model orchestration." },
	{ id = "kimi", name = "Kimi CLI", by = "Moonshot AI", version = "1.52.0", kind = "binary", license = "MIT", about = "Moonshot AI's coding assistant.", icon = true, source = "github.com/MoonshotAI/kimi-cli" },
	{ id = "minimax-code", name = "MiniMax Code", by = "MiniMax", version = "0.2.7", kind = "npx", license = "MIT", about = "MiniMax's AI coding agent." },
	{ id = "minion-code", name = "Minion Code", by = "femto", version = "0.1.44", kind = "uvx", license = "AGPL-3.0", about = "A code assistant on the Minion framework, with rich tools." },
	{ id = "mistral-vibe", name = "Mistral Vibe", by = "Mistral AI", version = "3.0.3", kind = "binary", license = "Apache-2.0", about = "Mistral's open-source coding assistant.", icon = true, source = "github.com/mistralai/mistral-vibe" },
	{ id = "nova", name = "Nova", by = "Compass AI", version = "1.1.62", kind = "npx", license = "Proprietary", about = "A fully-fledged software engineer at your command." },
	{ id = "opencode", name = "OpenCode", by = "Anomaly", version = "2.2.3", kind = "binary", license = "MIT", about = "The open source coding agent.", builtin = true },
	{ id = "opencode-v2", name = "OpenCode V2", by = "Anomaly", version = "2.2.3", kind = "binary", license = "MIT", about = "The open source coding agent, version 2." },
	{ id = "pi-acp", name = "pi ACP", by = "Sergii Kozak", version = "0.0.34", kind = "npx", license = "MIT", about = "The pi coding agent, through its ACP adapter.", builtin = true },
	{ id = "poolside", name = "Poolside", by = "Poolside", version = "1.0.16", kind = "binary", license = "Proprietary", about = "Poolside's coding agent." },
	{ id = "qoder", name = "Qoder CLI", by = "Qoder AI", version = "0.2.14", kind = "npx", license = "Proprietary", about = "AI coding assistant with agentic capabilities." },
	{ id = "qwen-code", name = "Qwen Code", by = "Alibaba Qwen Team", version = "0.26.4", kind = "npx", license = "Apache-2.0", about = "Alibaba's Qwen coding assistant.", icon = true, source = "github.com/QwenLM/qwen-code" },
	{ id = "sigit", name = "siGit Code", by = "smbCloud", version = "1.7.7", kind = "binary", license = "Apache-2.0", about = "Local-first coding agent, with optional on-device inference." },
	{ id = "stakpak", name = "Stakpak", by = "Stakpak", version = "0.3.88", kind = "binary", license = "Apache-2.0", about = "Open-source DevOps agent in Rust." },
	{ id = "vtcode", name = "VT Code", by = "vinhnx", version = "0.96.14", kind = "binary", license = "MIT", about = "An open-source coding agent with LLM-native code understanding." },
}
M.REGISTRY = REGISTRY

function M.entry(id: string): Entry?
	for _, e in M.REGISTRY do
		if e.id == id then
			return e
		end
	end
	return nil
end

-- What adding an agent takes on the device, as an outcome: what it runs
-- with and whether the device has it (Mara's Mac has Node.js, not uv).
function M.needs(e: Entry): string
	local where = M.device_name()
	if e.kind == "npx" then
		return `Runs with Node.js, which {where} has.`
	elseif e.kind == "uvx" then
		return `Needs uv on {where}, which isn't there yet.`
	end
	return `Downloads a build for {where}.`
end

-- An agent's icon on the marks' white tile (the registry draws its icons
-- in one ink), Zeron's own mark for the agents it drives itself, or its
-- initial on a tile of its own when it has none.
local MARKS: { [string]: string } = {
	["claude-acp"] = "claude",
	["codex-acp"] = "openai",
	cursor = "cursor",
	devin = "devin",
	["grok-build"] = "grok",
	opencode = "opencode",
	["pi-acp"] = "pi",
	["antigravity-acp"] = "antigravity",
}
function M.registry_tile(e: Entry, size: number): Node
	if MARKS[e.id] then
		return ds.Mark { name = MARKS[e.id] :: any, size = size }
	end
	-- (In the agent's colours, as the app draws it: ds.AgentMark.)
	return ds.AgentMark {
		label = e.name,
		seed = e.id,
		src = if e.icon then `assets/icons/registry/{e.id}.svg` else nil,
		size = size,
	}
end

-- The cards and their pieces are ds.ProviderCard (setup's Your agents
-- shows them too); this page holds the agents, their accounts and their
-- sign-ins.
local Card = ds.ProviderCard
M.disc = Card.disc

-- What a card's page does with it: whether the agent's settings open from
-- it (Settings only), what Add account starts, and what a sign-in's row
-- does when it's cancelled, dismissed or tried again.
export type CardOptions = {
	settings: boolean?,
	on_add: (() -> ())?,
	settle: ((next: string) -> ())?,
}

-- Each agent's own settings, in a dialog from its card. The rows are the
-- ones Zeron keeps per agent (settings/completion.rs, delegation.rs,
-- harnesses.rs): how skills complete in the composer, whether the agent
-- delegates to subagents of its own, and how its updates are taken. They
-- apply as they are changed, so the dialog only needs a way out.
local POLICIES = {
	{ value = "notify", label = "Notify" },
	{ value = "auto", label = "Auto when idle" },
	{ value = "off", label = "Off" },
}
-- (Short, so each row keeps its control beside it in the dialog's width.)
local POLICY_NOTE = {
	notify = "Only when you choose Update.",
	auto = "On its own, once no chat runs on it.",
	off = "New versions aren't looked for.",
}
M.settings_open = {}
for _, p in M.INSTALLED do
	M.settings_open[p.id] = ui.state(`ex_pv_settings_{p.id}`, false)
end

function M.settings_dialog(provider: any): Node
	local dollar = ui.state(`ex_pv_dollar_{provider.id}`, true)
	local separate = ui.state(`ex_pv_separate_{provider.id}`, false)
	local subagents = ui.state(`ex_pv_subagents_{provider.id}`, provider.id == "claude")
	local policy = ui.state(`ex_pv_policy_{provider.id}`, "notify")
	return ds.Dialog {
		open = M.settings_open[provider.id],
		title = `{provider.name} settings`,
		body = function()
			return `On {M.device_name()}. Changes apply as you make them.`
		end :: any,
		size = "md",
		cancel = false,
		confirm = { label = "Done" },
		ds.Settings.Group {
			ds.Settings.Row {
				label = "Use $ for skills",
				description = "Type $ in the composer to pick a skill.",
				control = ds.Switch { value = dollar, label = "Use $ for skills" },
			},
			ds.Settings.Row {
				label = "Separate / commands",
				description = "Keep skills out of the / menu.",
				control = ds.Switch { value = separate, label = "Separate / commands" },
			},
			ds.Settings.Row {
				label = "Native subagents",
				description = "Off, it delegates through side chats.",
				control = ds.Switch { value = subagents, label = "Native subagents" },
			},
			ds.Settings.Row {
				label = "Updates",
				description = function()
					return POLICY_NOTE[policy:get()] or ""
				end,
				control = ds.Select { value = policy, name = "Update policy", size = "sm", w = 136, options = POLICIES },
			},
		},
	}
end

-- Codex's account as its banked resets stand (the card follows it in
-- place): as the engine's data has it after each outcome.
function M.codex_accounts(): { any }
	local a: any = table.clone(M.ACCOUNTS.codex[1])
	local s = M.codex_reset:get()
	if s == "used" then
		-- The soonest spent; the windows start over until usage says more.
		a.session, a.week = 0, 0
		a.resets = { a.resets[2] }
		a.reset = { state = "reset" }
	elseif s == "using" then
		a.reset = { state = "using" }
	elseif s == "kept" then
		-- The session had started over on its own: nothing to reset.
		a.session, a.resets_at = 0.08, nil
		a.reset = { state = "nothing" }
	elseif s == "failed" then
		a.reset = { state = "failed", why = "mismatch" }
	elseif s == "none" then
		a.resets = {}
	end
	return { a }
end

-- Codex's banked resets, chosen and used in one dialog (ds.ProviderCard's,
-- the app's own).
function M.resets_dialog(): Node
	return ds.ProviderCard.ResetsDialog {
		open = M.reset_open,
		account = function()
			return M.codex_accounts()[1]
		end,
		choice = M.reset_choice,
		on_use = function()
			M.codex_reset:set("used")
		end,
	}
end

-- An installed agent's card, as its sign-in stands (`foot`), on this
-- page's data: who is signed in, the account in use, Codex's resets.
-- `foot` a value, or a function the card's foot follows in place.
function M.card(provider: any, foot: (string | () -> string)?, opts: CardOptions?): Node
	local o: CardOptions = opts or {}
	return Card.Card {
		provider = provider,
		accounts = if provider.id == "codex" then M.codex_accounts else (M.ACCOUNTS[provider.id] or {}),
		foot = foot,
		enabled = M.enabled[provider.id],
		active = function()
			local a = M.active[provider.id]
			return if a then a:get() else nil
		end,
		settings = o.settings,
		on_settings = function()
			M.settings_open[provider.id]:set(true)
		end,
		on_add = o.on_add,
		settle = o.settle or function(_next: string)
			if provider.id == "codex" then
				M.codex_login:set("idle")
			else
				M.grok_login:set("idle")
			end
		end,
		on_switch = function(a)
			M.active[provider.id]:set(a.email)
		end,
		on_reset = function(_a)
			M.reset_open:set(true)
		end,
		on_dismiss_reset = function(_a)
			M.codex_reset:set("banked")
		end,
		on_remove = function()
			M.removing:set(true)
		end,
	}
end

-- The installed agents' cards, and the one for Codex and Grok in the state
-- their sign-in is in: the foot follows it in place (the cards are built
-- once).
function M.cards(): Node
	local cards: { Node } = {}
	for _, p in M.INSTALLED do
		local login = if p.id == "codex" then M.codex_login elseif p.id == "grok" then M.grok_login else nil
		table.insert(cards, M.card(p, if login then function()
			return login:get()
		end else nil))
	end
	return ui.column { w_full = true, gap = "$space.4", table.unpack(cards) }
end

-- The agents still to install: Zeron's own, a click each; then every other
-- agent there is, a page away.
M.install_row = Card.InstallRow
M.loading = Card.Loading
M.unavailable = Card.Unavailable

function M.more(): Node
	local rows: { Node } = {}
	for _, p in M.MORE do
		table.insert(rows, M.install_row(p))
	end
	table.insert(rows, ds.Settings.Row {
		label = "Add another agent",
		description = `{#M.REGISTRY} agents in the registry, or one of your own.`,
		lead = M.disc(ds.Icon { name = "plus", size = 16 }),
		on_click = function()
			M.view:set("add")
		end,
	})
	return ds.Settings.Section {
		title = "More agents",
		description = "Install one and it appears above, ready to sign in.",
		ds.Settings.Group(rows :: any),
	}
end

-- Where the add pages stand, over their title: the way back to the list.
-- The way back from a page under Providers: the page it came from, as a
-- quiet link led by a chevron, over the title. (A breadcrumb trail ending
-- in the page's own name said the title twice.)
local function back(label: string, to: string): Node
	return ui.row {
		self = "start",
		gap = "$space.1",
		items = "center",
		cursor = "pointer",
		role = "button",
		label = `Back to {label}`,
		color = "$text.tertiary",
		hover = { color = "$text.primary" },
		on_click = function()
			M.view:set(to)
		end,
		ds.Icon { name = "alt-arrow-left", size = 14 },
		ui.text { label, text_size = "$font.size.sm", line_height = "$font.line.sm" },
	}
end

-- The registry: every agent, searched, each a row that opens it; the ones
-- Zeron drives itself say so and stay put. Last, one of your own.
function M.registry_page(): Node
	local function rows(): Node
		local q = string.lower(M.query:get())
		local out: { Node } = {}
		for _, e in M.REGISTRY do
			if q == "" or string.find(string.lower(e.name .. " " .. e.by .. " " .. e.about), q, 1, true) then
				if e.builtin then
					table.insert(out, ds.Settings.Row {
						label = e.name,
						description = e.about,
						lead = M.registry_tile(e, 32),
						control = ui.text { "Built in", text_size = "$font.size.sm", color = "$text.tertiary" },
					})
				else
					table.insert(out, ds.Settings.Row {
						label = e.name,
						description = e.about,
						lead = M.registry_tile(e, 32),
						badge = ds.Badge { e.by },
						on_click = function()
							M.adding:set("idle")
							M.view:set(`agent:{e.id}`)
						end,
					})
				end
			end
		end
		if #out == 0 then
			return ds.EmptyState {
				icon = "search",
				title = `Nothing called "{M.query:peek()}"`,
				body = "Try the maker's name, or add it as your own agent.",
				size = "sm",
				action = {
					label = "Clear search",
					on_click = function()
						M.query:set("")
					end,
				},
			}
		end
		table.insert(out, ds.Settings.Row {
			label = "Run your own agent",
			description = "One you built, or one that isn't in the registry yet.",
			lead = M.disc(ds.Icon { name = "terminal", size = 16 }),
			on_click = function()
				M.view:set("own")
			end,
		})
		return ds.Settings.Group(out :: any)
	end
	return M.sub_page(back("Providers", "agents"), ds.Settings.Page {
		title = "Add an agent",
		description = `Every agent that speaks the Agent Client Protocol: {#M.REGISTRY} today.`,
		trail = ds.SearchField { value = M.query, placeholder = "Search agents", size = "sm", w = 220, autofocus = true },
		ui.presence(function(): string
			return M.query:get()
		end, function(_: string): Node
			return rows()
		end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 100 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	})
end

-- A page under Providers: where it stands, over its title, at the page's
-- own measure.
function M.sub_page(where: Node, page: Node): Node
	return ui.column {
		w_full = true,
		-- (A theme number, read as it is built: max_w takes points.)
		max_w = ui.theme.get("zeron.settings.measure") :: number?,
		gap = "$space.4",
		where,
		page,
	}
end

-- One agent of the registry, before it is added: what it is, where it is
-- from, what adding it takes, and the one button; then adding, added, or
-- the failure, in the same place.
function M.agent_page(e: Entry): Node
	local where = M.device_name()
	local facts: { any } = {
		{ key = "Version", value = e.version },
		{ key = "License", value = e.license },
	}
	if e.source then
		table.insert(facts, { key = "Source", value = ds.Link { e.source, external = true, size = "sm", tone = "quiet" } })
	end
	table.insert(facts, { key = "Runs", value = M.needs(e) })
	table.insert(facts, { key = "Models", value = `{e.name}'s own, once it's added.` })
	table.insert(facts, { key = "Sign-in", value = "Its own, the first time you use it, or a key in its settings." })
	local function outcome(state: string): Node
		if state == "added" then
			return ds.Callout {
				tone = "success",
				title = `{e.name} is ready`,
				body = `It's in your providers on {where}, switched on.`,
				action = {
					label = "Open Providers",
					on_click = function()
						M.view:set("agents")
					end,
				},
			}
		elseif state == "failed" then
			return ds.Callout {
				tone = "danger",
				title = `Couldn't add {e.name}`,
				body = `The download didn't finish. Nothing was changed on {where}.`,
				action = {
					label = "Try again",
					on_click = function()
						M.adding:set("adding")
					end,
				},
			}
		end
		return ui.row {
			w_full = true,
			justify = "end",
			gap = "$space.2",
			items = "center",
			ui.show(function()
				return state == "adding"
			end, ds.ProgressRing { size = 16, tone = "neutral", label = "Adding" }),
			ds.Button {
				if state == "adding" then `Adding {e.name}…` else `Add to {where}`,
				disabled = state == "adding",
				on_click = function()
					M.adding:set("adding")
				end,
			},
		}
	end
	return M.sub_page(back("Add an agent", "add"), ds.Settings.Page {
		title = e.name,
		description = e.about,
		trail = M.registry_tile(e, 48),
		ds.Settings.Section {
			ds.Settings.Group {
				ui.column { w_full = true, px = "$space.4", py = "$space.3", ds.KeyValue { items = facts, key_width = 80 } },
			},
		},
		ui.presence(function(): string
			return M.adding:get()
		end, outcome, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	})
end

-- One of your own: a name, how to run it, and what it needs from the
-- environment; all of it stays on the device.
local own_name = ui.state("ex_pv_own_name", "")
local own_command = ui.state("ex_pv_own_command", "")
local own_env = ui.state("ex_pv_own_env", "")

function M.own_page(): Node
	local where = M.device_name()
	return M.sub_page(back("Add an agent", "add"), ds.Settings.Page {
		title = "Run your own agent",
		description = `One you built, or one that isn't in the registry yet. It runs on {where}.`,
		ds.Settings.Group {
			ds.Settings.Row {
				label = "Name",
				description = "How it shows in your providers and the composer.",
				control = ds.TextField { value = own_name, name = "Name", placeholder = "Halcyon agent", w = 240, autofocus = true },
			},
			ds.Settings.Row {
				label = "Command",
				description = "The program and its arguments, as you'd run them in a terminal.",
				stacked = true,
				control = ds.TextArea { value = own_command, name = "Command", placeholder = "~/code/halcyon-agent/bin/agent --acp", mono = true, rows = 1, max_rows = 3, w_full = true },
			},
			ds.Settings.Row {
				label = "Environment",
				description = `One a line. Keys stay on {where}.`,
				stacked = true,
				control = ds.TextArea { value = own_env, name = "Environment", placeholder = "OPENAI_API_KEY=sk-…", mono = true, rows = 2, max_rows = 6, w_full = true },
			},
		},
		ui.row {
			w_full = true,
			justify = "end",
			ds.Button {
				`Add to {where}`,
				disabled = function()
					return own_name:get() == "" or own_command:get() == ""
				end,
			},
		},
	})
end

-- The page in whichever view it is in.
function M.page_for(view: string): Node
	if view == "add" then
		return M.registry_page()
	elseif view == "own" then
		return M.own_page()
	end
	local id = string.match(view, "^agent:(.+)$")
	local e = if id then M.entry(id) else nil
	if e then
		return M.agent_page(e)
	end
	return ui.div {}
end

-- The dialogs the pages share: every agent's settings, the resets, the
-- account removal.
function M.dialogs(): { Node }
	local out: { Node } = {}
	for _, p in M.INSTALLED do
		table.insert(out, M.settings_dialog(p))
	end
	table.insert(out, M.resets_dialog())
	table.insert(out, ds.Dialog {
		open = M.removing,
		title = "Remove mara@example.net?",
		body = "Claude Code signs out of it on this Mac. Chats that used it stay as they are.",
		tone = "danger",
		confirm = { label = "Remove" },
	})
	return out
end

return M
