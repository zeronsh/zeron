-- The palettes a window can wear, Zeron's built-in themes (crates/theme
-- builtins.rs, its seeds copied as they are): each a few colours a theme
-- is grown from. Zeron Dark and Zeron Light are the design system's own
-- themes (themes/*.json) and change nothing; every other one overrides the
-- colour tokens through `tokens(p)`, the one mapping from a palette to
-- zgx's tokens, in the window's glass as Zeron's are: the window and its
-- frosted tint from its background, menus and the composer from its chrome
-- and card, every hairline and wash from its text's own ink, its accent and
-- tones, its code's and Markdown's colours, diffs, git, statuses, and a
-- visual's chart colours from its terminal's ANSI ones. (The terminal's
-- own colours have no token yet: zgx's terminal paints a fixed palette.)
--
-- Each palette stays legible: an ink that reads too faint on the palette's
-- surfaces is moved toward black or white until it reads (body text and
-- code at 4.5:1, faint text, comments and fills at 3:1), and `tokens`
-- says what it moved.
--
--   palettes.LIST            every built-in one, in Zeron's order ({ id, name, kind, … })
--   palettes.imported()      your imported VS Code themes' (src/themes.rs), followed
--   palettes.find(id)        one, built in or imported, by its id (nil if none)
--   palettes.tokens(p)       the token overrides that make a window wear it,
--                            and the inks moved to stay legible ({ Fix })
--   palettes.wear(family)    a template's palettes (its theme.luau: `{ name,
--                            dark?, light? }`, or nil for Zeron's own) as the
--                            themes each scheme wears: their names
--   palettes.scheme          those names now ({ dark, light })
--   palettes.contrast(a, b)  two colours' contrast ratio (WCAG 2)

local M = {}

export type Palette = {
	id: string,
	name: string,
	kind: string,
	background: string,
	raised: string,
	text: string,
	muted: string,
	faint: string,
	accent: string,
	danger: string,
	warning: string,
	success: string,
	-- comment, keyword, string, number, type, function, property,
	-- variable, punctuation, tag, attribute, invalid (Zeron's order).
	syntax: { string },
	-- An imported one's library entry (src/themes.rs).
	entry: string?,
	-- The shipping app's chrome and card surfaces (the window's where
	-- absent), its terminal's background and its sixteen ANSI colours
	-- (black, red, green, yellow, blue, magenta, cyan, white, then bright).
	shell: string?,
	card: string?,
	terminal: string?,
	ansi: { string }?,
}

-- A template's palettes (its theme.luau): one per scheme it has.
export type Family = { name: string?, dark: Palette?, light: Palette? }

-- An ink `tokens` moved to keep it legible: its role, the source's colour,
-- what it wears, and the contrast it was held to.
export type Fix = { role: string, from: string, to: string, min: number }

M.LIST = {
	{ id = "zeron-dark", name = "Zeron Dark", kind = "dark", background = "#060606", raised = "#343438", text = "#e8e8ea", muted = "#a9a9ae", faint = "#85858a", accent = "#8b7cf6", danger = "#f87171", warning = "#facc15", success = "#34d399", syntax = { "#92929a", "#8b7cf6", "#34d399", "#facc15", "#c084fc", "#60a5fa", "#f472b6", "#e8e8ea", "#a1a1aa", "#f472b6", "#22d3ee", "#f87171" } },
	{ id = "zeron-light", name = "Zeron Light", kind = "light", background = "#ffffff", raised = "#ededf0", text = "#303035", muted = "#62626a", faint = "#797981", accent = "#5b43e8", danger = "#dc2626", warning = "#a16207", success = "#15803d", syntax = { "#6b7280", "#5b43e8", "#15803d", "#a16207", "#7e22ce", "#2563eb", "#be185d", "#303035", "#52525b", "#be185d", "#0e7490", "#b91c1c" } },
	{ id = "vscode-dark-plus", name = "Dark+", kind = "dark", background = "#1e1e1e", raised = "#2a2d2e", text = "#d4d4d4", muted = "#a8a8a8", faint = "#858585", accent = "#007acc", danger = "#f48771", warning = "#cca700", success = "#89d185", syntax = { "#6a9955", "#c586c0", "#ce9178", "#b5cea8", "#4ec9b0", "#dcdcaa", "#9cdcfe", "#d4d4d4", "#d4d4d4", "#569cd6", "#9cdcfe", "#f44747" } },
	{ id = "vscode-light-plus", name = "Light+", kind = "light", background = "#ffffff", raised = "#e8e8e8", text = "#1f1f1f", muted = "#616161", faint = "#767676", accent = "#0078d4", danger = "#a1260d", warning = "#895503", success = "#008000", syntax = { "#008000", "#0000ff", "#a31515", "#098658", "#267f99", "#795e26", "#001080", "#1f1f1f", "#1f1f1f", "#800000", "#ff0000", "#cd3131" } },
	{ id = "catppuccin-mocha", name = "Catppuccin Mocha", kind = "dark", background = "#1e1e2e", raised = "#313244", text = "#cdd6f4", muted = "#a6adc8", faint = "#9399b2", accent = "#cba6f7", danger = "#f38ba8", warning = "#f9e2af", success = "#a6e3a1", syntax = { "#6c7086", "#cba6f7", "#a6e3a1", "#fab387", "#f9e2af", "#89b4fa", "#94e2d5", "#cdd6f4", "#bac2de", "#f38ba8", "#f5c2e7", "#f38ba8" } },
	{ id = "catppuccin-latte", name = "Catppuccin Latte", kind = "light", background = "#eff1f5", raised = "#dce0e8", text = "#4c4f69", muted = "#5c5f77", faint = "#6c6f85", accent = "#8839ef", danger = "#d20f39", warning = "#df8e1d", success = "#40a02b", syntax = { "#8c8fa1", "#8839ef", "#40a02b", "#fe640b", "#df8e1d", "#1e66f5", "#179299", "#4c4f69", "#6c6f85", "#d20f39", "#ea76cb", "#d20f39" } },
	{ id = "tokyo-night", name = "Tokyo Night", kind = "dark", background = "#1a1b26", raised = "#292e42", text = "#c0caf5", muted = "#9aa5ce", faint = "#7f87ad", accent = "#7aa2f7", danger = "#f7768e", warning = "#e0af68", success = "#9ece6a", syntax = { "#565f89", "#bb9af7", "#9ece6a", "#ff9e64", "#2ac3de", "#7aa2f7", "#73daca", "#c0caf5", "#9abdf5", "#f7768e", "#e0af68", "#f7768e" } },
	{ id = "tokyo-night-light", name = "Tokyo Night Light", kind = "light", background = "#d5d6db", raised = "#bfc0c6", text = "#343b59", muted = "#485e7d", faint = "#5a6378", accent = "#34548a", danger = "#8c4351", warning = "#8f5e15", success = "#33635c", syntax = { "#6c6e75", "#5a4a78", "#33635c", "#965027", "#0f4b6e", "#34548a", "#166775", "#343b59", "#485e7d", "#8c4351", "#8f5e15", "#8c4351" } },
	{ id = "dracula", name = "Dracula", kind = "dark", background = "#282a36", raised = "#44475a", text = "#f8f8f2", muted = "#c5c8c6", faint = "#9a9cab", accent = "#bd93f9", danger = "#ff5555", warning = "#f1fa8c", success = "#50fa7b", syntax = { "#6272a4", "#ff79c6", "#f1fa8c", "#bd93f9", "#8be9fd", "#50fa7b", "#8be9fd", "#f8f8f2", "#f8f8f2", "#ff79c6", "#50fa7b", "#ff5555" } },
	{ id = "github-light", name = "GitHub Light", kind = "light", background = "#ffffff", raised = "#d0d7de", text = "#1f2328", muted = "#656d76", faint = "#818b98", accent = "#0969da", danger = "#cf222e", warning = "#9a6700", success = "#1a7f37", syntax = { "#6e7781", "#cf222e", "#0a3069", "#0550ae", "#953800", "#8250df", "#116329", "#1f2328", "#57606a", "#116329", "#953800", "#cf222e" } },
	{ id = "github-dark", name = "GitHub Dark", kind = "dark", background = "#0d1117", raised = "#21262d", text = "#e6edf3", muted = "#8b949e", faint = "#6e7681", accent = "#2f81f7", danger = "#f85149", warning = "#d29922", success = "#3fb950", syntax = { "#8b949e", "#ff7b72", "#a5d6ff", "#79c0ff", "#ffa657", "#d2a8ff", "#7ee787", "#e6edf3", "#b1bac4", "#7ee787", "#ffa657", "#f85149" } },
	{ id = "ayu-light", name = "Ayu Light", kind = "light", background = "#fcfcfc", raised = "#e6e9ed", text = "#5c6166", muted = "#828e9f", faint = "#969da8", accent = "#f29718", danger = "#e65050", warning = "#eba400", success = "#6cbf43", syntax = { "#adaeb1", "#fa8532", "#86b300", "#a37acc", "#55b4d4", "#f29718", "#f07171", "#5c6166", "#828e9f", "#55b4d4", "#e59645", "#e65050" } },
	{ id = "ayu-dark", name = "Ayu Dark", kind = "dark", background = "#10141c", raised = "#1b1f29", text = "#bfbdb6", muted = "#8a9199", faint = "#5a6673", accent = "#e6b450", danger = "#d95757", warning = "#ffb454", success = "#70bf56", syntax = { "#5a6673", "#ff8f40", "#aad94c", "#d2a6ff", "#39bae6", "#ffb454", "#f07178", "#bfbdb6", "#8a9199", "#39bae6", "#e6c08a", "#d95757" } },
	{ id = "ayu-mirage", name = "Ayu Mirage", kind = "dark", background = "#242936", raised = "#343d4d", text = "#cccac2", muted = "#8a94a6", faint = "#6e7c8f", accent = "#ffcc66", danger = "#ff6666", warning = "#ffcc66", success = "#87d96c", syntax = { "#6e7c8f", "#ffa659", "#d5ff80", "#dfbfff", "#5ccfe6", "#ffcd66", "#f28779", "#cccac2", "#8a94a6", "#5ccfe6", "#d9be98", "#ff6666" } },
	{ id = "gruvbox-light", name = "Gruvbox Light", kind = "light", background = "#fbf1c7", raised = "#d5c4a1", text = "#3c3836", muted = "#665c54", faint = "#7c6f64", accent = "#076678", danger = "#9d0006", warning = "#b57614", success = "#79740e", syntax = { "#928374", "#9d0006", "#79740e", "#8f3f71", "#b57614", "#076678", "#427b58", "#3c3836", "#665c54", "#9d0006", "#af3a03", "#cc241d" } },
	{ id = "gruvbox-dark", name = "Gruvbox Dark", kind = "dark", background = "#282828", raised = "#504945", text = "#ebdbb2", muted = "#bdae93", faint = "#928374", accent = "#8ec07c", danger = "#fb4934", warning = "#fabd2f", success = "#b8bb26", syntax = { "#928374", "#fb4934", "#b8bb26", "#d3869b", "#fabd2f", "#83a598", "#8ec07c", "#ebdbb2", "#bdae93", "#fb4934", "#fe8019", "#fb4934" } },
	{ id = "rose-pine-dawn", name = "Rosé Pine Dawn", kind = "light", background = "#faf4ed", raised = "#dfdad9", text = "#575279", muted = "#797593", faint = "#9893a5", accent = "#907aa9", danger = "#b4637a", warning = "#ea9d34", success = "#56949f", syntax = { "#9893a5", "#286983", "#ea9d34", "#d7827e", "#56949f", "#b4637a", "#56949f", "#d7827e", "#797593", "#56949f", "#907aa9", "#b4637a" } },
	{ id = "rose-pine-moon", name = "Rosé Pine Moon", kind = "dark", background = "#232136", raised = "#393552", text = "#e0def4", muted = "#908caa", faint = "#6e6a86", accent = "#c4a7e7", danger = "#eb6f92", warning = "#f6c177", success = "#9ccfd8", syntax = { "#6e6a86", "#3e8fb0", "#f6c177", "#ea9a97", "#9ccfd8", "#eb6f92", "#9ccfd8", "#ea9a97", "#908caa", "#9ccfd8", "#c4a7e7", "#eb6f92" } },
	{ id = "nord", name = "Nord", kind = "dark", background = "#2e3440", raised = "#434c5e", text = "#d8dee9", muted = "#aeb6c4", faint = "#7f899b", accent = "#88c0d0", danger = "#bf616a", warning = "#ebcb8b", success = "#a3be8c", syntax = { "#616e88", "#81a1c1", "#a3be8c", "#b48ead", "#8fbcbb", "#88c0d0", "#d08770", "#d8dee9", "#aeb6c4", "#81a1c1", "#8fbcbb", "#bf616a" } },
	{ id = "one-dark-pro", name = "One Dark Pro", kind = "dark", background = "#282c34", raised = "#2c313a", text = "#abb2bf", muted = "#9da5b4", faint = "#7f848e", accent = "#528bff", danger = "#e05561", warning = "#d19a66", success = "#8cc265", syntax = { "#7f848e", "#c678dd", "#e06c75", "#c678dd", "#e5c07b", "#e5c07b", "#d19a66", "#abb2bf", "#abb2bf", "#c678dd", "#56b6c2", "#e05561" } },
	{ id = "atom-one-dark", name = "Atom One Dark", kind = "dark", background = "#282c34", raised = "#2c313a", text = "#abb2bf", muted = "#9da5b4", faint = "#5c6370", accent = "#528bff", danger = "#e06c75", warning = "#d19a66", success = "#98c379", syntax = { "#5c6370", "#c678dd", "#98c379", "#d19a66", "#e5c07b", "#61afef", "#56b6c2", "#e06c75", "#abb2bf", "#e06c75", "#61afef", "#e06c75" } },
	{ id = "night-owl", name = "Night Owl", kind = "dark", background = "#011627", raised = "#0b253a", text = "#d6deeb", muted = "#8ca6ba", faint = "#637777", accent = "#82aaff", danger = "#ef5350", warning = "#ffeb95", success = "#22da6e", syntax = { "#637777", "#82aaff", "#c5e478", "#f78c6c", "#ffcb8b", "#c5e478", "#bec5d4", "#d7dbe0", "#82aaff", "#7fdbca", "#f78c6c", "#ef5350" } },
	{ id = "night-owl-light", name = "Night Owl Light", kind = "light", background = "#fbfbfb", raised = "#d3e8f8", text = "#403f53", muted = "#66657a", faint = "#7a8297", accent = "#4876d6", danger = "#de3d3b", warning = "#a67d00", success = "#08916a", syntax = { "#6f7896", "#4876d6", "#0c8f75", "#aa0982", "#111111", "#4876d6", "#111111", "#403f53", "#4876d6", "#111111", "#aa0982", "#de3d3b" } },
	{ id = "winter-dark-blue", name = "Winter is Coming Dark Blue", kind = "dark", background = "#011627", raised = "#0b253a", text = "#d6deeb", muted = "#8ca6ba", faint = "#5f7e97", accent = "#219fd5", danger = "#ef5350", warning = "#ffca28", success = "#22da6e", syntax = { "#8095a8", "#c792ea", "#78bd65", "#8dec95", "#d29ffc", "#87aff4", "#7fdbca", "#cbcdd2", "#4fb4d8", "#6dbdfa", "#f7ecb5", "#ef5350" } },
	{ id = "winter-light", name = "Winter is Coming Light", kind = "light", background = "#ffffff", raised = "#e5e5e5", text = "#1857a4", muted = "#62626a", faint = "#66768a", accent = "#2f86d2", danger = "#de3d3b", warning = "#9b6a00", success = "#08916a", syntax = { "#357b42", "#7942a8", "#87429a", "#174781", "#0444ac", "#b1108e", "#207b76", "#224555", "#4f7aa0", "#0444ac", "#b46a0f", "#c73532" } },
	{ id = "palenight", name = "Palenight", kind = "dark", background = "#292d3e", raised = "#34384c", text = "#eeffff", muted = "#a6accd", faint = "#697098", accent = "#82aaff", danger = "#ff5572", warning = "#ffcb6b", success = "#c3e88d", syntax = { "#697098", "#c792ea", "#c3e88d", "#f78c6c", "#eeffff", "#ffcb6b", "#89ddff", "#bfc7d5", "#82aaff", "#89ddff", "#7986e7", "#ff5572" } },
	{ id = "synthwave-84", name = "SynthWave '84", kind = "dark", background = "#262335", raised = "#37294d", text = "#ffffff", muted = "#b8b5c6", faint = "#848bbd", accent = "#ff7edb", danger = "#fe4450", warning = "#fede5d", success = "#72f1b8", syntax = { "#848bbd", "#72f1b8", "#f97e72", "#2ee2fa", "#fe4450", "#72f1b8", "#2ee2fa", "#f8f8f2", "#72f1b8", "#72f1b8", "#dd5500", "#fe4450" } },
	{ id = "shades-of-purple", name = "Shades of Purple", kind = "dark", background = "#2d2b55", raised = "#393668", text = "#ffffff", muted = "#b8afea", faint = "#8d84c7", accent = "#fad000", danger = "#ec3a37", warning = "#fad000", success = "#3ad900", syntax = { "#b362ff", "#ff628c", "#fb94ff", "#fad000", "#fb94ff", "#fad000", "#fad000", "#9effff", "#fad000", "#fad000", "#ff9d00", "#ec3a37" } },
	{ id = "cobalt2", name = "Cobalt2", kind = "dark", background = "#193549", raised = "#20455e", text = "#ffffff", muted = "#b7c9d3", faint = "#7da1b5", accent = "#ffc600", danger = "#ff628c", warning = "#ffc600", success = "#3ad900", syntax = { "#0088ff", "#ff9d00", "#ffee80", "#ffc600", "#ff68b8", "#ffc600", "#9effff", "#ffffff", "#ffee80", "#ffc600", "#ffb454", "#f44542" } },
	{ id = "andromeda", name = "Andromeda", kind = "dark", background = "#23262e", raised = "#2d313b", text = "#d5ced9", muted = "#a6a0aa", faint = "#746f77", accent = "#00e8c6", danger = "#ee5d43", warning = "#ff9f2e", success = "#96e072", syntax = { "#a0a1a7", "#ee5d43", "#96e072", "#f39c12", "#ffe66d", "#ffe66d", "#d5ced9", "#f39c12", "#96e072", "#f92672", "#ee5d43", "#ee5d43" } },
} :: { Palette }

-- The design system's own: nothing to override.
local OWN = { ["zeron-dark"] = true, ["zeron-light"] = true }

-- Your theme library, as the host publishes it (src/themes.rs): `{ state,
-- list = { Palette }, entries = { { id, name, status, variants } } }`.
-- (Made when first read, so this module needs no window to load.)
local library: State<any>? = nil
function M.library(): State<any>
	if library == nil then
		library = ui.shared("themes.library", { state = "loading", list = {}, entries = {} } :: any)
	end
	return library :: State<any>
end

-- Your imported palettes (read tracked: they follow the library).
function M.imported(): { Palette }
	return (M.library():get().list or {}) :: { Palette }
end

function M.find(id: string?): Palette?
	for _, p in M.LIST do
		if p.id == id then
			return p
		end
	end
	for _, p in M.imported() do
		if p.id == id then
			return p
		end
	end
	return nil
end

-- Colours, as the shipping app's Color does them (crates/theme lib.rs):
-- "#rrggbb" or "#rrggbbaa", mixed channel by channel, contrast as WCAG 2
-- has it (a translucent ink laid over what it stands on first).
local WHITE, BLACK = "#ffffff", "#000000"

local function parse(c: string): (number, number, number, number)
	local h = string.gsub(c, "^#", "")
	if #h == 3 or #h == 4 then
		h = string.gsub(h, "%x", "%0%0")
	end
	local function at(i: number): number?
		return tonumber(string.sub(h, i, i + 1), 16)
	end
	return at(1) or 0, at(3) or 0, at(5) or 0, if #h >= 8 then at(7) or 255 else 255
end

local function hex(r: number, g: number, b: number, a: number?): string
	local function byte(x: number): string
		return string.format("%02x", math.clamp(math.floor(x + 0.5), 0, 255))
	end
	local alpha = if a and a < 255 then byte(a) else ""
	return `#{byte(r)}{byte(g)}{byte(b)}{alpha}`
end

-- `a` moved `t` of the way to `b` (alpha too).
function M.mix(a: string, b: string, t: number): string
	local t_ = math.clamp(t, 0, 1)
	local ar, ag, ab, aa = parse(a)
	local br, bg, bb, ba = parse(b)
	return hex(ar + (br - ar) * t_, ag + (bg - ag) * t_, ab + (bb - ab) * t_, aa + (ba - aa) * t_)
end

-- `c` at opacity `a` (0 to 1).
function M.alpha(c: string, a: number): string
	local r, g, b = parse(c)
	return hex(r, g, b, math.clamp(a, 0, 1) * 255)
end

local function luminance(c: string): number
	local r, g, b = parse(c)
	local function linear(ch: number): number
		local v = ch / 255
		return if v <= 0.04045 then v / 12.92 else ((v + 0.055) / 1.055) ^ 2.4
	end
	return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
end

function M.contrast(ink: string, under: string): number
	local r, g, b, a = parse(ink)
	if a < 255 then
		local ur, ug, ub = parse(under)
		local t = a / 255
		ink = hex(r * t + ur * (1 - t), g * t + ug * (1 - t), b * t + ub * (1 - t))
	end
	local x, y = luminance(ink), luminance(under)
	return (math.max(x, y) + 0.05) / (math.min(x, y) + 0.05)
end

local function least(ink: string, unders: { string }): number
	local worst = math.huge
	for _, u in unders do
		worst = math.min(worst, M.contrast(ink, u))
	end
	return worst
end

-- `c`, moved toward black or white (whichever the first surface takes
-- better) until it reads at `min` on every one of `unders`.
function M.ensure(c: string, unders: { string }, min: number): string
	if least(c, unders) >= min then
		return c
	end
	local target = if M.contrast(BLACK, unders[1]) >= M.contrast(WHITE, unders[1]) then BLACK else WHITE
	for step = 1, 20 do
		local candidate = M.mix(c, target, step / 20)
		if least(candidate, unders) >= min then
			return candidate
		end
	end
	return target
end

-- Black or white, whichever reads better on `c` (a fill's label).
function M.on(c: string): string
	return if M.contrast(WHITE, c) >= M.contrast(BLACK, c) then WHITE else BLACK
end

-- What each of the seeds' code colours sets (the syntax tokens), as the
-- shipping app maps them (crates/theme builtins.rs `syntax`): labels take
-- the functions' colour, macros and operators the keywords'. Markdown's
-- roles ride along (headings and diff hunks the keywords', links the
-- functions', a bare address the comments'). The last, invalid code, has
-- no token of zgx's.
local SYNTAX = {
	{ "comment", "url" },
	{ "keyword", "heading", "hunk", "macro", "operator" },
	{ "string" },
	{ "number", "constant", "strong" },
	{ "type" },
	{ "function", "link", "label" },
	{ "property" },
	{ "variable", "emphasis" },
	{ "punctuation" },
	{ "tag" },
	{ "attribute", "escape", "regex" },
	{},
}
-- Code's quieter roles: held to 3:1, as faint text is.
local QUIET = { [1] = true, [9] = true }

-- An agent's status, a tone a state (`zeron.status.*` and its dots).
local STATUS = {
	connecting = "info",
	waiting = "faint",
	input = "accent",
	approval = "warning",
	plan = "accent",
	done = "success",
	failed = "danger",
	limited = "warning",
}

-- A visual's chart colours (ds/visual_ink) from the terminal's ANSI ones:
-- the bright half on dark, the normal on light.
local SERIES = { teal = 7, amber = 4, rose = 2, violet = 6, lime = 3, sky = 5 }

-- The overrides a window wears for `p` (nil for the design system's own),
-- and the inks moved to stay legible.
function M.tokens(p: Palette): ({ [string]: any }?, { Fix })
	local fixes: { Fix } = {}
	if OWN[p.id] then
		return nil, fixes
	end
	local dark = p.kind ~= "light" and p.kind ~= "high-contrast-light"
	local function pick(on_dark: any, on_light: any): any
		return if dark then on_dark else on_light
	end
	local function ensure(role: string, c: string, unders: { string }, min: number): string
		local out = M.ensure(c, unders, min)
		if out ~= c then
			table.insert(fixes, { role = role, from = c, to = out, min = min })
		end
		return out
	end
	local a = M.alpha

	-- The surfaces: the window, its chrome, a card, and a menu's opaque
	-- fill (the shipping app's overlay: the card a little toward raised).
	local bg = p.background
	local shell = p.shell or bg
	local card = p.card or bg
	local raised = p.raised
	local menu = M.mix(card, raised, pick(0.34, 0.02))
	local surfaces = { bg, shell, card, menu }
	local code = { bg, card }

	-- Inks: text and its muted step at 4.5, the faint one at 3; fills (the
	-- accent, the tones) at 3 on the window, and each tone's ink, where it
	-- is words (a badge, a file's status, a diff's count), at 4.5.
	local text = ensure("text.primary", p.text, surfaces, 4.5)
	local muted = ensure("text.secondary", p.muted, surfaces, 4.5)
	local faint = ensure("text.tertiary", p.faint, surfaces, 3)
	local ansi = p.ansi or {}
	local fill = {
		accent = ensure("accent", p.accent, { bg }, 3),
		danger = ensure("danger", p.danger, { bg }, 3),
		warning = ensure("warning", p.warning, { bg }, 3),
		success = ensure("success", p.success, { bg }, 3),
		info = ensure("info", ansi[if dark then 13 else 5] or p.accent, { bg }, 3),
	}
	local ink: { [string]: string } = { faint = faint }
	for tone, c in fill do
		ink[tone] = ensure(`{tone} ink`, c, surfaces, 4.5)
	end

	local tint = a(bg, pick(0.8, 0.72))
	local out: { [string]: any } = {
		-- The window: opaque where it has no glass, its frosted tint where
		-- it does (Zeron's own strength), and its surfaces over that glass.
		["zeron.window"] = bg,
		background = tint,
		["glass.tint"] = tint,
		["popover.bg"] = pick(M.mix(raised, card, 0.25), card),
		["surface.sunken"] = pick(a(M.mix(shell, BLACK, 0.45), 0.76), a(text, 0.05)),
		["surface.base"] = pick(a(text, 0.03), a(card, 0.4)),
		["surface.raised"] = pick(a(text, 0.06), a(card, 0.78)),
		["surface.overlay"] = pick(a(menu, 0.88), a(card, 0.9)),
		["text.primary"] = text,
		["text.secondary"] = muted,
		["text.tertiary"] = faint,
		["text.disabled"] = a(text, 0.28),
		-- Hairlines and washes: the text's own ink, faint.
		border = a(text, pick(0.07, 0.14)),
		["border.subtle"] = a(text, pick(0.04, 0.09)),
		["border.strong"] = a(text, 0.18),
		hover = a(text, 0.05),
		pressed = a(text, pick(0.09, 0.08)),
		["selection.item"] = a(text, pick(0.1, 0.07)),
		["zeron.edge"] = a(text, pick(0.05, 0.18)),
		["zeron.well.edge"] = a(text, pick(0.09, 0.07)),
		-- The accent, and what follows it (written against `accent`, so an
		-- accent chosen in Settings carries them: the focus ring, a
		-- switch's fill, a search's wash, the caret, the editor's selection
		-- are Zeron's own, written so already).
		accent = fill.accent,
		["accent.hover"] = M.mix(fill.accent, pick(WHITE, BLACK), pick(0.12, 0.1)),
		["accent.text"] = M.on(fill.accent),
		["zeron.accent.active"] = M.mix(fill.accent, BLACK, 0.15),
		["zeron.accents.default"] = fill.accent,
		selection = pick("alpha({accent}, 0.35)", "alpha({accent}, 0.24)"),
		["focus.ring"] = "alpha({accent}, 0.6)",
		danger = fill.danger,
		warning = fill.warning,
		success = fill.success,
		info = fill.info,
		-- Chosen things: a card's raised chip, a sidebar row's wash, a
		-- segmented control's thumb, a menu's row.
		["zeron.selected.bg"] = pick(a(text, 0.11), a(card, 0.92)),
		["zeron.selected.ring"] = a(text, pick(0.08, 0.2)),
		["zeron.selected.flat.bg"] = a(text, pick(0.14, 0.07)),
		["zeron.segmented.thumb"] = pick(a(text, 0.14), a(card, 0.55)),
		["zeron.menu.tint"] = a(shell, pick(0.65, 0.45)),
		["zeron.menu.solid"] = menu,
		["zeron.menu.selected"] = a(text, pick(0.09, 0.06)),
		-- The composer: the chrome's glass, opaque the card's.
		["zeron.composer.tint"] = pick(a(shell, 0.35), a(card, 0.55)),
		["zeron.composer.solid"] = pick(M.mix(card, raised, 0.18), card),
		["zeron.composer.edge"] = a(text, pick(0.09, 0.12)),
		-- Buttons: the way on in the text's ink, beside it a wash of it,
		-- destructive in the danger's fill.
		["zeron.primary.bg"] = text,
		["zeron.primary.hover"] = M.mix(text, pick(WHITE, BLACK), 0.15),
		["zeron.primary.active"] = M.mix(text, pick(BLACK, WHITE), 0.12),
		["zeron.primary.text"] = bg,
		["zeron.secondary.bg"] = a(text, pick(0.08, 0.05)),
		["zeron.secondary.hover"] = a(text, pick(0.12, 0.08)),
		["zeron.secondary.active"] = a(text, pick(0.16, 0.11)),
		["zeron.destructive.bg"] = fill.danger,
		["zeron.destructive.hover"] = M.mix(fill.danger, pick(WHITE, BLACK), 0.1),
		["zeron.destructive.active"] = M.mix(fill.danger, BLACK, 0.15),
		["zeron.destructive.text"] = M.on(fill.danger),
		-- Fields and switches.
		["zeron.switch.track"] = a(text, pick(0.14, 0.09)),
		["zeron.field.edge"] = a(text, pick(0.08, 0.12)),
		["zeron.field.edge_hover"] = a(text, pick(0.15, 0.22)),
		["zeron.check.edge"] = a(text, 0.22),
		["zeron.lightbox.scrim"] = pick(a(M.mix(shell, BLACK, 0.4), 0.88), a(bg, 0.92)),
		-- Diffs, git, pull requests, reviews and checks: each tone's ink.
		["zeron.diff.added"] = ink.success,
		["zeron.diff.removed"] = ink.danger,
		["zeron.git.modified"] = ink.warning,
		["zeron.git.added"] = ink.success,
		["zeron.git.untracked"] = ink.success,
		["zeron.git.renamed"] = ink.accent,
		["zeron.git.deleted"] = ink.danger,
		["zeron.git.conflict"] = ink.danger,
		["zeron.pr.open"] = ink.success,
		["zeron.pr.merged"] = ink.accent,
		["zeron.pr.closed"] = ink.danger,
		["zeron.pr.draft"] = faint,
		["zeron.review.approved"] = ink.success,
		["zeron.review.changes"] = ink.danger,
		["zeron.review.pending"] = ink.warning,
		["zeron.checks.passing"] = ink.success,
		["zeron.checks.failing"] = ink.danger,
		["zeron.checks.running"] = ink.warning,
		-- Code: its body, and Markdown's quieter parts in the text's steps.
		["zeron.syntax.text"] = text,
		["zeron.syntax.quote"] = muted,
		["zeron.syntax.meta"] = muted,
		["zeron.syntax.inserted"] = ink.success,
		["zeron.syntax.deleted"] = ink.danger,
	}
	if not dark then
		-- Light glass: fields and checks the card's white, not a darkening.
		out["zeron.field.bg"] = a(card, 0.6)
		out["zeron.field.hover"] = a(card, 0.8)
		out["zeron.check.bg"] = a(card, 0.8)
	end
	for tone, c in fill do
		out[`zeron.tone.{tone}.ink`] = ink[tone]
		out[`zeron.tone.{tone}.wash`] = a(c, pick(0.14, 0.1))
		out[`zeron.tone.{tone}.edge`] = a(c, pick(0.3, 0.26))
	end
	for state, tone in STATUS do
		out[`zeron.status.{state}`] = ink[tone]
		out[`zeron.status.dot.{state}`] = ink[tone]
	end
	for i, names in SYNTAX do
		local color = p.syntax[i]
		if color and #names > 0 then
			local min = if QUIET[i] then 3 else 4.5
			local legible = ensure(`zeron.syntax.{names[1]}`, color, code, min)
			for _, name in names do
				out[`zeron.syntax.{name}`] = legible
			end
		end
	end
	for name, i in SERIES do
		local color = ansi[if dark then i + 8 else i]
		if color then
			out[`zeron.visual.series.{name}`] = ensure(`zeron.visual.series.{name}`, color, { bg }, 3)
		end
	end
	return out, fixes
end

-- The themes each scheme wears now: Zeron's own, or a template's.
M.scheme = { dark = "zeron-dark", light = "zeron-light" }

-- A template's palettes (nil for Zeron's own) registered as the themes each
-- scheme wears, each over Zeron's own of its scheme (its glass, type and
-- shape kept), and their names (`palettes.scheme`). A family with one
-- palette wears it in either scheme. (Call after Zeron's themes load.)
function M.wear(family: Family?): { dark: string, light: string }
	local names = { dark = "zeron-dark", light = "zeron-light" }
	if type(family) == "table" and (family.dark or family.light) then
		for _, kind in { "dark", "light" } do
			local p = (family :: any)[kind] :: Palette?
			if p then
				local own = table.clone(p)
				own.kind = kind
				local name = `template-{kind}`
				ui.theme.register {
					name = name,
					type = kind,
					extends = `zeron-{kind}`,
					tokens = (M.tokens(own)) or {},
				}
				names[kind] = name
			end
		end
		if not family.dark then
			names.dark = names.light
		elseif not family.light then
			names.light = names.dark
		end
	end
	M.scheme.dark, M.scheme.light = names.dark, names.light
	return names
end

return M
