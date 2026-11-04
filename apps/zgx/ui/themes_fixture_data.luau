-- Themes as src/themes.rs lists them, for fixtures (ui/themes_fixture.luau,
-- ui/templates_fixture.luau, the gallery's Themes page): a few of Zeron's
-- built in, their seeds as crates/theme builtins.rs has them; Ledger's own
-- (a template's theme.luau, dark only); and what src/themes.rs makes of
-- tests/fixtures/themes/Harbor Night.json.

local M = {}

M.BUILTIN = {
	{ id = "zeron-light", name = "Zeron Light", kind = "light", background = "#ffffff", raised = "#ededf0", text = "#303035", muted = "#62626a", faint = "#797981", accent = "#5b43e8", danger = "#dc2626", warning = "#a16207", success = "#15803d", syntax = { "#6b7280", "#5b43e8", "#15803d", "#a16207", "#7e22ce", "#2563eb", "#be185d", "#303035", "#52525b", "#be185d", "#0e7490", "#b91c1c" }, entry = "zeron" },
	{ id = "zeron-dark", name = "Zeron Dark", kind = "dark", background = "#060606", raised = "#343438", text = "#e8e8ea", muted = "#a9a9ae", faint = "#85858a", accent = "#8b7cf6", danger = "#f87171", warning = "#facc15", success = "#34d399", syntax = { "#92929a", "#8b7cf6", "#34d399", "#facc15", "#c084fc", "#60a5fa", "#f472b6", "#e8e8ea", "#a1a1aa", "#f472b6", "#22d3ee", "#f87171" }, entry = "zeron" },
	{ id = "gruvbox-light", name = "Gruvbox Light", kind = "light", background = "#fbf1c7", raised = "#d5c4a1", text = "#3c3836", muted = "#665c54", faint = "#7c6f64", accent = "#076678", danger = "#9d0006", warning = "#b57614", success = "#79740e", syntax = { "#928374", "#9d0006", "#79740e", "#8f3f71", "#b57614", "#076678", "#427b58", "#3c3836", "#665c54", "#9d0006", "#af3a03", "#cc241d" }, entry = "gruvbox" },
	{ id = "gruvbox-dark", name = "Gruvbox Dark", kind = "dark", background = "#282828", raised = "#504945", text = "#ebdbb2", muted = "#bdae93", faint = "#928374", accent = "#8ec07c", danger = "#fb4934", warning = "#fabd2f", success = "#b8bb26", syntax = { "#928374", "#fb4934", "#b8bb26", "#d3869b", "#fabd2f", "#83a598", "#8ec07c", "#ebdbb2", "#bdae93", "#fb4934", "#fe8019", "#fb4934" }, entry = "gruvbox" },
	{ id = "github-light", name = "GitHub Light", kind = "light", background = "#ffffff", raised = "#d0d7de", text = "#1f2328", muted = "#656d76", faint = "#818b98", accent = "#0969da", danger = "#cf222e", warning = "#9a6700", success = "#1a7f37", syntax = { "#6e7781", "#cf222e", "#0a3069", "#0550ae", "#953800", "#8250df", "#116329", "#1f2328", "#57606a", "#116329", "#953800", "#cf222e" }, entry = "github" },
	{ id = "github-dark", name = "GitHub Dark", kind = "dark", background = "#0d1117", raised = "#21262d", text = "#e6edf3", muted = "#8b949e", faint = "#6e7681", accent = "#2f81f7", danger = "#f85149", warning = "#d29922", success = "#3fb950", syntax = { "#8b949e", "#ff7b72", "#a5d6ff", "#79c0ff", "#ffa657", "#d2a8ff", "#7ee787", "#e6edf3", "#b1bac4", "#7ee787", "#ffa657", "#f85149" }, entry = "github" },
	{ id = "dracula", name = "Dracula", kind = "dark", background = "#282a36", raised = "#44475a", text = "#f8f8f2", muted = "#c5c8c6", faint = "#9a9cab", accent = "#bd93f9", danger = "#ff5555", warning = "#f1fa8c", success = "#50fa7b", syntax = { "#6272a4", "#ff79c6", "#f1fa8c", "#bd93f9", "#8be9fd", "#50fa7b", "#8be9fd", "#f8f8f2", "#f8f8f2", "#ff79c6", "#50fa7b", "#ff5555" }, entry = "dracula" },
	{ id = "ayu-mirage", name = "Ayu Mirage", kind = "dark", background = "#242936", raised = "#343d4d", text = "#cccac2", muted = "#8a94a6", faint = "#6e7c8f", accent = "#ffcc66", danger = "#ff6666", warning = "#ffcc66", success = "#87d96c", syntax = { "#6e7c8f", "#ffa659", "#d5ff80", "#dfbfff", "#5ccfe6", "#ffcd66", "#f28779", "#cccac2", "#8a94a6", "#5ccfe6", "#d9be98", "#ff6666" }, entry = "ayu-mirage" },
	{ id = "template-ledger-dark", name = "Ledger Ink", kind = "dark", background = "#2e3440", raised = "#434c5e", text = "#d8dee9", muted = "#aeb6c4", faint = "#7f899b", accent = "#88c0d0", danger = "#bf616a", warning = "#ebcb8b", success = "#a3be8c", syntax = { "#616e88", "#81a1c1", "#a3be8c", "#b48ead", "#8fbcbb", "#88c0d0", "#d08770", "#d8dee9", "#aeb6c4", "#81a1c1", "#8fbcbb", "#bf616a" }, entry = "template-ledger" },
}

M.ENTRIES = {
	{ id = "zeron", name = "Zeron", source = "builtin", variants = { "zeron-light", "zeron-dark" } },
	{ id = "gruvbox", name = "Gruvbox", source = "builtin", variants = { "gruvbox-light", "gruvbox-dark" } },
	{ id = "github", name = "GitHub", source = "builtin", variants = { "github-light", "github-dark" } },
	{ id = "dracula", name = "Dracula", source = "builtin", variants = { "dracula" } },
	{ id = "ayu-mirage", name = "Ayu Mirage", source = "builtin", variants = { "ayu-mirage" } },
	{ id = "template-ledger", name = "Ledger Ink", source = "template", template = "ledger", variants = { "template-ledger-dark" } },
}

-- Ledger's theme.luau, as it's written.
M.LEDGER_FILE = {
	name = "Ledger Ink",
	dark = { background = "#2e3440", raised = "#434c5e", text = "#d8dee9", muted = "#aeb6c4", faint = "#7f899b", accent = "#88c0d0", danger = "#bf616a", warning = "#ebcb8b", success = "#a3be8c", syntax = { "#616e88", "#81a1c1", "#a3be8c", "#b48ead", "#8fbcbb", "#88c0d0", "#d08770", "#d8dee9", "#aeb6c4", "#81a1c1", "#8fbcbb", "#bf616a" } },
}

M.HARBOR = {
	id = "custom-harbor-night-harbor-night",
	name = "Harbor Night",
	kind = "dark",
	entry = "custom-harbor-night",
	background = "#14181f",
	raised = "#1d232c",
	text = "#d8dee9",
	muted = "#a3acb9",
	faint = "#7c8594",
	accent = "#7aa2f7",
	danger = "#f7768e",
	warning = "#e0af68",
	success = "#9ece6a",
	syntax = { "#5c6773", "#c792ea", "#c3e88d", "#f78c6c", "#ffcb6b", "#82aaff", "#d8dee9", "#d8dee9", "#d8dee9", "#d8dee9", "#d8dee9", "#f7768e" },
}

return M
