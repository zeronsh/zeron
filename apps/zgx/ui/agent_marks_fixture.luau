-- Agents' marks, light and dark side by side, as each place draws them
-- (ds.AgentMark): Providers' cards (32), the registry's rows (32), the model
-- picker's tabs and rows (16 and 14, bare) and the sidebar's rows (14,
-- bare). Zeron's own agents in their bundled brand marks beside agents from
-- the ACP registry in the registry's own icons (assets/icons/registry), in
-- the theme's ink; one whose icon didn't come (the registry's glyph); and
-- one of your own (its initial). For captures and tests
-- (`fixture=agent_marks_fixture.luau`).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

type Agent = { name: string, seed: string, mark: string?, src: string?, by: string? }

local BUILT_IN: { Agent } = {
	{ name = "Claude Code", seed = "claude-code", mark = "claude", by = "Anthropic" },
	{ name = "Codex", seed = "codex", mark = "openai", by = "OpenAI" },
	{ name = "Cursor", seed = "cursor", mark = "cursor", by = "Cursor" },
	{ name = "Antigravity", seed = "antigravity", mark = "antigravity", by = "Google" },
}
local function registry(id: string, name: string, by: string): Agent
	return { name = name, seed = id, src = `assets/icons/registry/{id}.svg`, by = by }
end
local REGISTRY: { Agent } = {
	registry("factory-droid", "Factory Droid", "Factory AI"),
	registry("gemini", "Gemini CLI", "Google"),
	registry("kimi", "Kimi CLI", "Moonshot AI"),
	registry("qwen-code", "Qwen Code", "Alibaba Qwen"),
	registry("goose", "goose", "Block"),
	registry("cline", "Cline", "Cline Bot"),
	registry("github-copilot-cli", "Copilot", "GitHub"),
	registry("junie", "Junie", "JetBrains"),
	registry("kilo", "Kilo", "Kilo Code"),
	registry("mistral-vibe", "Mistral Vibe", "Mistral AI"),
	registry("auggie", "Auggie CLI", "Augment Code"),
	registry("deepagents", "DeepAgents", "LangChain"),
	registry("amp-acp", "Amp", "Sourcegraph"),
	-- (Its icon didn't come: the registry's glyph.)
	{ name = "Corust Agent", seed = "corust-agent", by = "Corust" },
}
local OWN: Agent = { name = "Halcyon agent", seed = "own.halcyon-agent", by = "Yours" }

local function mark(a: Agent, size: number, bare: boolean?): Node
	return ds.AgentMark { label = a.name, seed = a.seed, mark = a.mark, src = a.src, size = size, bare = bare }
end

local function heading(text: string): Node
	return ui.text { text, text_size = "$font.size.xs", weight = "$font.weight.semibold", color = "$text.tertiary" }
end

local function all(): { Agent }
	local out = table.clone(BUILT_IN)
	for _, a in REGISTRY do
		table.insert(out, a)
	end
	table.insert(out, OWN)
	return out
end

-- One theme's board.
local function board(theme: string, title: string): Node
	local cards: { Node } = {}
	for _, a in all() do
		table.insert(cards, ui.row {
			gap = "$space.3",
			items = "center",
			mark(a, 32),
			ui.column {
				gap = "$space.1",
				ui.text { a.name, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
				ui.text { a.by or "", text_size = "$font.size.xs", color = "$text.tertiary" },
			},
		})
	end
	local tabs: { Node } = {}
	for _, a in all() do
		table.insert(tabs, ui.div { w = 28, h = 28, items = "center", justify = "center", mark(a, 16, true) })
	end
	local rows: { Node } = {}
	for i, a in all() do
		if i % 2 == 1 then
			table.insert(rows, ui.row {
				gap = "$space.2",
				items = "center",
				ui.div { w = 18, h = 18, items = "center", justify = "center", mark(a, 14, true) },
				ui.text { a.name, text_size = "$font.size.sm", color = "$text.primary" },
			})
		end
	end
	local sidebar: { Node } = {}
	for i, a in all() do
		if i % 2 == 0 then
			table.insert(sidebar, ui.row {
				gap = "$space.2",
				items = "center",
				ui.div { opacity = 0.9, mark(a, 14, true) },
				ui.text { `Chat on {a.name}`, text_size = "$font.size.sm", color = "$text.secondary" },
			})
		end
	end
	return ui.column {
		theme = theme,
		w = 560,
		p = "$space.6",
		gap = "$space.5",
		bg = "$zeron.window",
		ui.text { title, text_size = "$font.size.lg", weight = "$font.weight.semibold", color = "$text.primary" },
		heading("PROVIDERS · ADD AN AGENT (32, on the tile)"),
		ui.grid { w_full = true, cols = 2, gap = "$space.3", table.unpack(cards) },
		heading("MODEL PICKER (tabs 16, rows 14, bare)"),
		ui.row { gap = "$space.1", flex_wrap = true, table.unpack(tabs) },
		ui.column { gap = "$space.2", table.unpack(rows) },
		heading("SIDEBAR (14, bare)"),
		ui.column { gap = "$space.2", table.unpack(sidebar) },
	}
end

return ui.row {
	id = "agent-marks",
	w_full = true,
	h_full = true,
	board("zeron-light", "Light"),
	board("zeron-dark", "Dark"),
}
