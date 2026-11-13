-- The palettes a window can wear, and the one mapping from a palette to
-- zgx's tokens (`tokens(p)`): every theme is a few colours it's grown from,
-- in the window's glass as Zeron's are: the window and its frosted tint
-- from its background, menus and the composer from its chrome and card,
-- every hairline and wash from its text's own ink, its accent and tones,
-- its code's and Markdown's colours, diffs, git, statuses, and a visual's
-- chart colours from its terminal's ANSI ones, and the terminal's own
-- colours (`terminal.*`: its text, cursor, selection and sixteen ANSI
-- colours, which zgx gives the terminal over the user's Ghostty ones).
-- Zeron's own (zeron-dark, zeron-light) are the design system's themes
-- (themes/*.json) and change nothing but the terminal's.
--
-- Each palette stays legible: an ink that reads too faint on the palette's
-- surfaces is moved toward black or white until it reads (body text and
-- code at 4.5:1, faint text, comments and fills at 3:1; the terminal's ANSI
-- colours at 4.5:1, 3:1 for the two that are the background's own shade),
-- and `tokens` says what it moved.
--
-- The themes themselves are the host's (src/themes.rs): Zeron's built in,
-- each template's own and your imported ones, and the one chosen for each
-- scheme. A scheme with none chosen wears the running template's own
-- (its theme.luau), else Zeron's.
--
--   palettes.library()       the host's list ({ state, list, entries, chosen })
--   palettes.find(id)        one theme's palette by its id (nil if none)
--   palettes.slots(own)      what each scheme wears now: the theme chosen for
--                            it, else `own`'s (a template's theme.luau:
--                            `{ name?, dark?, light? }`), else nil (Zeron's)
--   palettes.wear(slots)     those palettes worn as the window's two themes
--   palettes.scheme          their names ({ dark, light }), which never change
--   palettes.tokens(p)       the token overrides that make a window wear `p`,
--                            and the inks moved to stay legible ({ Fix })
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
	-- The theme it's listed under (src/themes.rs).
	entry: string?,
	-- The shipping app's chrome and card surfaces (the window's where
	-- absent), its terminal's background and its sixteen ANSI colours
	-- (black, red, green, yellow, blue, magenta, cyan, white, then bright).
	shell: string?,
	card: string?,
	terminal: string?,
	ansi: { string }?,
}

-- A theme's palettes, one per scheme it has (a template's theme.luau).
export type Family = { name: string?, dark: Palette?, light: Palette? }
-- What each scheme wears: a palette, or nil for Zeron's.
export type Slots = { dark: Palette?, light: Palette? }

-- An ink `tokens` moved to keep it legible: its role, the source's colour,
-- what it wears, and the contrast it was held to.
export type Fix = { role: string, from: string, to: string, min: number }

-- The design system's own: nothing to override.
local OWN = { ["zeron-dark"] = true, ["zeron-light"] = true }

-- Every theme, as the host publishes them (src/themes.rs): `{ state,
-- list = { Palette }, entries = { { id, name, source, variants, … } },
-- chosen = { light?, dark? } }`. (Made when first read, so this module
-- needs no window to load, and a plugin lent it never reads the app's.)
local library: State<any>? = nil
function M.library(): State<any>
	if library == nil then
		library = ui.shared("themes.library", { state = "loading", list = {}, entries = {} } :: any)
	end
	return library :: State<any>
end

-- One theme's palette by its id (read tracked: it follows the list).
function M.find(id: string?): Palette?
	if type(id) ~= "string" then
		return nil
	end
	for _, p in (M.library():get().list or {}) :: { Palette } do
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

-- The terminal's colours (zgx's `terminal.*`), into `out`: its text the
-- palette's (legible already), the cursor its accent with black or white
-- under it, the selection its accent over the card (Ghostty's colours are
-- opaque: a wash is mixed in), its background the card the pane shows
-- (the app draws the pane; Ghostty's own background is clear), and the
-- sixteen ANSI colours held legible on the window's surfaces as its inks
-- are (`ensure`), at 3:1 the two that are the background's own shade
-- (black and bright black on dark, white and bright white on light: a
-- light theme's pale white turns grey, never white on white), else 4.5:1.
-- A palette without ANSI colours leaves those the terminal's own.
local function terminal(out: { [string]: any }, p: Palette, dark: boolean, surfaces: { string }, card: string, text: string, accent: string, ensure: (string, string, { string }, number) -> string)
	out["terminal.background"] = card
	out["terminal.foreground"] = text
	out["terminal.cursor"] = accent
	out["terminal.cursor_text"] = M.on(accent)
	out["terminal.selection_background"] = M.mix(card, accent, if dark then 0.35 else 0.24)
	out["terminal.selection_foreground"] = text
	for i, c in p.ansi or {} do
		local n = i - 1
		local dim = if dark then n == 0 or n == 8 else n == 7 or n == 15
		out[`terminal.ansi.{n}`] = ensure(`terminal.ansi.{n}`, c, surfaces, if dim then 3 else 4.5)
	end
end

-- The overrides a window wears for `p` (for the design system's own, the
-- terminal's alone), and the inks moved to stay legible.
function M.tokens(p: Palette): ({ [string]: any }?, { Fix })
	local fixes: { Fix } = {}
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
	if OWN[p.id] then
		-- (Its surfaces and inks are its theme file's, legible as made.)
		local card = p.card or p.background
		local menu = M.mix(card, p.raised, pick(0.34, 0.02))
		local own: { [string]: any } = {}
		terminal(own, p, dark, { p.background, p.shell or p.background, card, menu }, card, p.text, p.accent, ensure)
		return own, fixes
	end

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
		-- The frosted surfaces, each by Zeron's own recipe from this palette
		-- rather than Zeron's greys (zeron-dark/light.json have them written
		-- out): the island the overlay's glass, the panel its background at
		-- 0.4, a field dark a settings select's wash (light the input's), the
		-- sidebar, tabs and cards a wash of the ink.
		["zeron.island.tint"] = a(menu, pick(0.75, 0.45)),
		["zeron.panel.tint"] = a(bg, 0.4),
		["zeron.field.glass"] = pick(a(text, 0.06), a(card, 0.35)),
		["zeron.field.glass_hover"] = pick(a(text, 0.1), a(card, 0.8)),
		["zeron.sidebar.tint"] = a(text, 0.05),
		["zeron.tab.selected"] = a(text, 0.1),
		["zeron.tab.hover"] = a(text, 0.06),
		["zeron.settings.tint"] = a(text, 0.045),
		["zeron.settings.note"] = a(text, 0.035),
		["zeron.settings.comment"] = a(text, 0.05),
		-- The title bar's controls and Windows' caption buttons: their glyph
		-- the secondary ink, on their own firmer wash (0.7 of it pressed, for
		-- a caption button).
		["zeron.bar.ink"] = muted,
		["zeron.bar.hover"] = a(text, pick(0.11, 0.06)),
		["zeron.bar.pressed"] = a(text, pick(0.16, 0.1)),
		["caption.pressed"] = a(text, pick(0.08, 0.04)),
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
	terminal(out, p, dark, surfaces, card, text, fill.accent, ensure)
	return out, fixes
end

-- The names of the window's two themes: one for each scheme, each over
-- Zeron's own of its scheme (its glass, type and shape kept), worn whatever
-- palette is in them, so a change of theme restyles in place.
M.scheme = { dark = "theme-dark", light = "theme-light" }

-- What each scheme wears now: the theme chosen for it, if it's there and of
-- its scheme; else `own`'s palette for it (the running template's theme);
-- else nil, Zeron's. (Read tracked: it follows the host's list.)
function M.slots(own: Family?): Slots
	local chosen = M.library():get().chosen or {}
	local out: { [string]: Palette? } = {}
	for _, kind in { "light", "dark" } do
		local p = M.find(chosen[kind])
		if p and p.kind ~= kind then
			p = nil
		end
		if not p and type(own) == "table" and type((own :: any)[kind]) == "table" then
			p = (own :: any)[kind]
		end
		out[kind] = p
	end
	return out :: any
end

-- A palette, as words to tell a change by.
local function signature(p: Palette?): string
	if not p then
		return ""
	end
	local parts = {}
	for k, v in p :: any do
		table.insert(parts, `{k}={if type(v) == "table" then table.concat(v, ",") else tostring(v)}`)
	end
	table.sort(parts)
	return table.concat(parts, ";")
end

-- What each of the window's themes was last registered with.
local worn: { [string]: string } = {}

-- `slots` worn (nil: Zeron's in both): each scheme's theme registered again
-- where its palette changed. Their names (`palettes.scheme`). A scheme
-- wearing Zeron's own takes its terminal's colours from Zeron's palette
-- (the host's list) once that's there; with no slots (a plugin, lent the
-- design system) nothing is looked up.
function M.wear(slots: Slots?): { dark: string, light: string }
	for _, kind: any in { "dark", "light" } do
		local p = if slots then (slots :: any)[kind] :: Palette? else nil
		if slots and p == nil then
			p = M.find(`zeron-{kind}`)
		end
		local said = signature(p)
		if worn[kind] ~= said then
			worn[kind] = said
			-- (No tokens at all for Zeron's: an empty table isn't an object.)
			local tokens = nil
			if p then
				local own = table.clone(p)
				own.kind = kind
				tokens = M.tokens(own)
			end
			ui.theme.register {
				name = (M.scheme :: any)[kind],
				type = kind,
				extends = `zeron-{kind}`,
				tokens = tokens,
			}
		end
	end
	return { dark = M.scheme.dark, light = M.scheme.light }
end

return M
