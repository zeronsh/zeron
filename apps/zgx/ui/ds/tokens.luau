-- The theme layer: Zeron's two themes (themes/*.json, which restyle in
-- place as they are saved) and the app's own tokens, declared in standard
-- terms so any theme styles them. Values live here and in the theme files,
-- nowhere else.

local M = {}

function M.setup()
	-- The palette (palette.luau): every hue by step, the same in both themes;
	-- the themes pick from it.
	ui.theme.tokens { palette = require("@self/palette") }
	-- zgx's own menus, where they still show (a plugin's own fields: a
	-- plugin's VM can't reach the app's host for ds/editor_menu): our menus'
	-- panel tint and row wash, in every theme and template, rather than
	-- zgx's (a saturated highlight). Their rows and panel keep zgx's shape.
	-- The app's own editors draw ds/editor_menu.
	ui.theme.tokens {
		menu = {
			bg = "{zeron.menu.tint}",
			hover = "{hover}",
		},
	}
	ui.theme.tokens {
		zeron = {
			-- Half-steps of the spacing scale (space.1 to 8: 4, 8, 12, 16, 24,
			-- 32, 48, 64), for controls: a thumb's inset in its track, a chip's
			-- padding and gap, a small button's padding.
			space = { hair = 2, tight = 6, snug = 10 },
			-- Between the items of any list (a menu's rows, a sidebar's pages):
			-- enough that one row's wash (hover, selected) never touches the
			-- next's, no more.
			list = { gap = 2 },
			-- An icon button (ds/icon_button): a glyph alone on a square. Three
			-- sizes: a row's corner or a bar's tight controls, a title bar's, a
			-- toolbar's; the glyph at each; and its corner, a button's 8.
			icon_button = {
				sm = 22,
				md = 24,
				lg = 28,
				glyph = { sm = 14, md = 16, lg = 16 },
				radius = "{radius.lg}",
			},
			-- The lines of a tree (a group's sessions off its header, as
			-- Zeron's transcript draws its tools): the text's own ink, faint
			-- (Zeron's hairline at 12%), so it reads in both themes.
			tree = "alpha({text.primary}, 0.12)",
			-- The open session's title in the window's top strip: the text's
			-- own ink a little under full (Zeron's at 85%), so it reads as the
			-- window's, not as the page's heading.
			title = "alpha({text.primary}, 0.85)",
			-- A raised thing: its edge, the light on its top (an inset shadow;
			-- none by default) and the shadow under it.
			edge = "{border.subtle}",
			highlight = "0 0 0 0 #00000000",
			lift = "{shadow.sm}",
			-- Sunken: its rim and its shadowed top lip (an inset shadow).
			well = { edge = "{border.subtle}", inset = "0 0 0 0 #00000000" },
			-- Selected: its fill, its ring, and its own highlight and lift. The
			-- themes give each its own relationships (lighter and lit in dark,
			-- a white chip with a ring in light).
			selected = {
				bg = "{selection.item}",
				ring = "{border}",
				highlight = "0 0 0 0 #00000000",
				lift = "0 0 0 0 #00000000",
				-- Flat (sidebar rows): the wash alone, no ring, light or lift.
				flat = { bg = "{selection.item}" },
			},
			-- A menu or dropdown panel's tint over the platform's frosted
			-- material.
			menu = {
				tint = "{surface.overlay}",
				solid = "{surface.overlay}",
				shadow = "{shadow.lg}",
				-- A menu's chosen row: quieter than a sidebar's (many rows
				-- close together), a step above the hover wash.
				selected = "{zeron.selected.flat.bg}",
				-- A destructive row's wash, lit or asking once more: the
				-- danger's hue, as faint as the hover wash reads.
				danger = "alpha({danger}, 0.14)",
				-- Its geometry (ds/menu): a row's height, a heading's, the box a
				-- row's glyph (or a tile) sits in and the glyph's size, a
				-- panel's width, and how far it blurs what is under it.
				row = 28,
				heading = 24,
				glyph = { box = 18, size = 16 },
				width = 232,
				blur = 30,
			},
			-- An island: controls gathered on glass over the page (the title
			-- bar's at the window's top left as the sidebar goes, the sidebar's
			-- rail at its bottom left), one look for both, as Zeron's
			-- (shell.rs `titlebar_island_vertical_geometry`): its corner, its
			-- inset from the window's edges, its padding round its controls,
			-- its controls' size, the gap between them and between their
			-- groups; and its glass.
			island = {
				radius = 12,
				inset = 6,
				pad = 4,
				control = 24,
				gap = 2,
				group = 8,
				tint = "{zeron.menu.tint}",
				blur = 20,
				shadow = "{shadow.sm}",
			},
			-- The composer: its pill (frosted, a tint over a backdrop blur;
			-- opaque, a solid fill with a shadow under it), its edge, its corner,
			-- and the draft's type (a touch larger than body: what is being
			-- written).
			composer = {
				tint = "{surface.raised}",
				solid = "{surface.overlay}",
				edge = "{border}",
				shadow = "{shadow.lg}",
				-- Frosted, nothing under it: the glass is its depth.
				rest = "0 0 0 0 #00000000",
				radius = 26,
				text = 14,
				line = 22,
				-- The caret: the accent, two points wide, as Zeron's.
				caret = "{accent}",
				caret_width = 2,
				-- What the message carries, as chips in a strip over the draft
				-- (ds/composer_cargo), at Zeron's inline chip's scale (chip.rs:
				-- a row less three, corner 5, a 14 glyph): a chip's height and
				-- corner, its glyph, how wide it may grow, the gap between chips,
				-- the strip's inset from the pill's edge (the draft's own, so
				-- chips and words share a left edge), its room above, and the
				-- room the draft keeps under it.
				chip = {
					h = 40,
					-- (Its corner is made below: the pill's less its edge and
					-- the chip's inset.)
					glyph = 18,
					-- The hollow round its corner ×: the gap between the button
					-- and the chip, what is behind showing through it.
					hollow = 2,
					-- The × in it: the chip's wash a step stronger, so it reads
					-- on the chip and on the pill alike.
					cross = "{pressed}",
					-- An image's thumbnail: the chip's height, wide as a window
					-- is (a screenshot crops to almost nothing).
					thumb = 56,
					max = 214,
					gap = "{space.2}",
					inset = 16,
					-- (As far down as it is in: the chips' corners nest in the
					-- pill's only with the same gap above as beside.)
					top = 16,
					below = 12,
					bg = "{zeron.secondary.bg}",
					edge = "{border}",
					-- Over a thumbnail while it uploads or after it fails: the
					-- scrim, and the ink of the ring or the glyph on it.
					scrim = "#00000073",
					scrim_ink = "#ffffff",
				},
				-- The trays behind the pill (ds/composer_dock): the menus' glass,
				-- their top corners, the step each sits in from the one below,
				-- how far one tucks under the next (and the last under the pill),
				-- a header's height and a row's.
				tray = {
					tint = "{zeron.menu.tint}",
					solid = "{zeron.menu.solid}",
					edge = "{border}",
					blur = "{zeron.menu.blur}",
					radius = 16,
					step = 16,
					tuck = 10,
					header = 32,
					row = 32,
					-- A tray's body never grows past this; past it, it scrolls.
					body = 216,
				},
				-- A question's option (ds/composer_ask): its row, its corner, the
				-- number cap at its start; chosen, a ring (never a fill).
				-- (Its corner and its number cap's are made below, concentric
				-- with the pill's.)
				option = { h = 36, key = 20, ring = "{accent}" },
				-- Something mentioned in the draft, in type alone (zgx's text
				-- has no inline chips): its trigger greyed as a markdown marker
				-- is, a file's name in the text's own ink and a command's or a
				-- skill's in the accent, both a weight up. And the track that
				-- unrolls from the microphone while dictating.
				token = { trigger = "{text.tertiary}", file = "{text.primary}", action = "{accent}", weight = 500 },
				voice = { h = 28, ink = "{accent}", bg = "{zeron.secondary.bg}" },
			},
			-- A line glyph's stroke, in points at 16 (the sidebar icon's). The
			-- themes may weigh it differently: a dark line on light reads
			-- thinner than the same line light on dark.
			glyph = { stroke = 1.1667 },
			-- Inline code and code blocks in what is typed: a wash behind them.
			code = { wash = "{zeron.secondary.bg}" },
			-- The code editor: Geist Mono at the transcript's code size; the
			-- wash behind the caret's row (Zeron's 2.5%, a breath); a matched
			-- bracket's; a search's matches, the current one firmer; a line
			-- just gone to, washed in the accent as it lands; its selection (the
			-- accent, as Zeron's editor); a folded line's summary; and its status
			-- bar's height.
			editor = {
				size = 13,
				line = 20,
				current = "alpha({text.primary}, 0.03)",
				bracket = "alpha({text.primary}, 0.14)",
				match = "alpha({warning}, 0.16)",
				match_current = "alpha({warning}, 0.55)",
				reveal = "alpha({accent}, 0.22)",
				selection = "alpha({accent}, 0.28)",
				fold = "alpha({text.primary}, 0.08)",
				status = 26,
			},
			-- A search's matched letters (a palette's, a picker's): their ink,
			-- and a faint wash of the accent behind them.
			match = { ink = "{text.primary}", wash = "alpha({accent}, 0.2)" },
			-- The command palette's card: in the window over its dimmed page
			-- (not over the platform's material, as a menu is), so a denser
			-- glass than a menu's tint over its own blur.
			palette = { tint = "{surface.overlay}", solid = "{zeron.menu.solid}" },
			-- A transcript, set for reading at length. Prose a step up from
			-- Zeron's 14 on 22: 15 on 24 (a 1.6 leading) reads easier over a
			-- long reply on glass. Headings in three real steps, tight above
			-- what they head; tables a touch smaller than prose; code in Geist
			-- Mono at 13 on 20, which sits at the prose's x-height, and inline
			-- a half point larger, so a `name` in a sentence doesn't shrink.
			-- `measure`: the column, never wider, about 85 characters of prose.
			-- An agent's visual (ds/visual): its six type sizes on the
			-- transcript's scale (`body` the prose's, `xl` and `xxl` its
			-- headings'); and the chart colours after the accent, six fixed
			-- hues of which the five farthest from the accent show
			-- (ds/visual_ink), at a middle step here and at each theme's own
			-- in the themes (brighter on dark, deeper on light).
			visual = {
				text = {
					xs = { size = 12, line = 16 },
					sm = { size = 13, line = 20 },
					body = { size = "{zeron.prose.size}", line = "{zeron.prose.line}" },
					lg = { size = 16, line = 24 },
					xl = { size = "{zeron.prose.h2.size}", line = "{zeron.prose.h2.line}" },
					xxl = { size = "{zeron.prose.h1.size}", line = "{zeron.prose.h1.line}" },
				},
				series = {
					teal = "{palette.teal.500}",
					amber = "{palette.amber.500}",
					rose = "{palette.rose.500}",
					violet = "{palette.violet.500}",
					lime = "{palette.lime.500}",
					sky = "{palette.sky.500}",
				},
			},
			-- The bubble: Zeron's wash (one step softer than a selection) and
			-- its 16 corner; an attachment's thumbnail, Zeron's size.
			prose = {
				size = 15,
				line = 24,
				h1 = { size = 22, line = 30 },
				h2 = { size = 18, line = 26 },
				h3 = { size = 15, line = 24 },
				small = { size = 14, line = 20 },
				code = {
					size = 13,
					line = 20,
					inline = 13.5,
					bg = "alpha({text.primary}, 0.04)",
					edge = "{border.subtle}",
				},
				measure = 660,
				bubble = { bg = "{zeron.secondary.bg}", radius = 16 },
				thumb = { w = 120, h = 84, radius = 10 },
				-- A picture a reply shows (ds/reply_picture): at most the
				-- reading column's width by 420, never past its own size; a
				-- screenshot's shape (16:10) while it comes; its corner; a small
				-- one's least tile; how long (as wide as tall, at least) and how
				-- wide (as tall, at most) before it's cut; a run's tray (its
				-- inset, so the corner is the picture's plus it, and its tallest
				-- row); the controls' inset from the corner.
				picture = {
					w = 660,
					h = 420,
					place = 1.6,
					radius = 12,
					min = 64,
					tall = 0.5,
					wide = 5,
					tray = { inset = 4, row = 300 },
					control = { inset = 6 },
				},
			},
			-- Reasoning effort, a colour a level, cool to warm as it rises: mid
			-- tones that read on a light track and a dark one alike (the
			-- slider's ASCII fill draws in them, and shaders take no tokens).
			effort = {
				none = "#8b8b93",
				minimal = "#8a9fa5",
				low = "#7d9bb8",
				medium = "#8d86d8",
				high = "#b47ec2",
				xhigh = "#cda34d",
				max = "#d9845a",
			},
			-- A pull request's badge, by its state (its glyph and number in it,
			-- on a wash of it).
			pr = { open = "{success}", merged = "{accent}", closed = "{danger}", draft = "{text.tertiary}" },
			-- A file's git status: its name's ink and its letter in a tree, and
			-- a folder's dot for the most pressing inside it. Each theme picks
			-- its own steps.
			git = {
				modified = "{warning}",
				added = "{success}",
				untracked = "{success}",
				renamed = "{accent}",
				deleted = "{danger}",
				conflict = "{danger}",
			},
			-- A file tree and a file shown: the guide of the folder you're in
			-- (firmer than the rest), where a dragged file would land, and the
			-- checks behind an image's transparency (over the panel's own fill).
			-- The pane's band (ds/files_pane): the tree's head and the file's
			-- header share it, one line high (`head`), or two (`stack`); the
			-- tree's width beside the file (Zeron's files panel, 286), and the
			-- least a file wants beside it (Zeron's narrowest pane, 360): with
			-- less, the tree hides and comes over the file instead.
			-- Files carried over a chat: the scrim over all of it (Zeron's at
			-- two thirds; the light theme's own in its file).
			drop = { scrim = "#00000066" },
			files = {
				guide = "alpha({text.primary}, 0.24)",
				drop = "alpha({accent}, 0.14)",
				checker = "alpha({text.primary}, 0.06)",
				head = 40,
				stack = 48,
				tree = 288,
				view = 360,
				-- Which of a file icon's two drawings shows (as drawn, for
				-- light; its accents lifted, for dark).
				light = 1,
				dark = 0,
			},
			-- An agent's status: its glyph's and label's colour (`zeron.status.*`)
			-- and its dot's in a list (`zeron.status.dot.*`), a hue a state,
			-- each theme its own.
			status = {
				working = "{info}",
				connecting = "{info}",
				waiting = "{text.tertiary}",
				input = "{accent}",
				approval = "{warning}",
				plan = "{accent}",
				done = "{success}",
				failed = "{danger}",
				limited = "{warning}",
				dot = {
					working = "{info}",
					connecting = "{info}",
					waiting = "{text.tertiary}",
					input = "{accent}",
					approval = "{warning}",
					plan = "{accent}",
					done = "{success}",
					failed = "{danger}",
					limited = "{warning}",
				},
			},
			-- A slider's track: an inset groove; and how strongly its fill
			-- takes the level's colour (its tint and its ring), each theme its
			-- own.
			slider = { track = "{surface.sunken}", tint = 0.14, ring = 0.3 },
			-- A segmented control's thumb.
			segmented = { thumb = "{zeron.selected.flat.bg}" },
			-- A switch: its track (off, and on: translucent, the glass showing
			-- through) and its knob.
			switch = {
				track = "{surface.sunken}",
				on = "{accent}",
				knob = "#ffffff",
				knob_lift = "{shadow.sm}",
			},
			-- Buttons, each tone at rest, under the pointer and pressed.
			primary = { bg = "{text.primary}", hover = "{text.primary}", active = "{text.primary}", text = "{background}" },
			secondary = { bg = "{hover}", hover = "{pressed}", active = "{pressed}" },
			accent = { active = "{accent.hover}" },
			-- Destructive: a confirm that deletes or discards. Red filled,
			-- white on it; each theme picks its steps.
			destructive = { bg = "{danger}", hover = "{danger}", active = "{danger}", text = "#ffffff" },
			-- The primitives (fields, checks, tabs, badges, notices, dialogs,
			-- settings and onboarding pieces) share these, so they read as one
			-- set: one focus ring, one field, one wash for a tone.
			--
			-- The focus ring: one look for everything that takes the keys, a
			-- two point outline of the accent a point off the control's edge;
			-- and the ring an invalid field wears instead.
			focus = { ring = "{focus.ring}", width = 2, offset = 1, error = "alpha({danger}, 0.5)" },
			-- What can't be used now: dimmed as a whole, as Button is.
			disabled = 0.4,
			-- A field: a quiet well with a hairline edge that firms under the
			-- pointer and takes the accent focused (the ring outside it); its
			-- placeholder; a field gone wrong, its edge the danger's.
			field = {
				bg = "{surface.sunken}",
				hover = "{surface.sunken}",
				edge = "{border}",
				edge_hover = "{border.strong}",
				edge_focus = "alpha({accent}, 0.7)",
				edge_error = "alpha({danger}, 0.7)",
				placeholder = "{text.tertiary}",
				-- Its type at each size, and a multi-line field's line.
				line = 20,
			},
			-- A check or a radio: its box (sunken until it is on), its edge,
			-- the accent it fills with on, and the mark's ink.
			check = { bg = "{zeron.field.bg}", edge = "{border.strong}", on = "{accent}", mark = "{accent.text}" },
			-- A tone's ink and the wash and edge that carry it (a badge, a
			-- banner, a toast's glyph), each theme its own steps: pale inks on
			-- dark, deep inks on light.
			tone = {
				neutral = { ink = "{text.secondary}", wash = "{zeron.secondary.bg}", edge = "{border}" },
				accent = { ink = "{accent}", wash = "alpha({accent}, 0.14)", edge = "alpha({accent}, 0.3)" },
				info = { ink = "{info}", wash = "alpha({info}, 0.12)", edge = "alpha({info}, 0.28)" },
				success = { ink = "{success}", wash = "alpha({success}, 0.12)", edge = "alpha({success}, 0.28)" },
				warning = { ink = "{warning}", wash = "alpha({warning}, 0.12)", edge = "alpha({warning}, 0.28)" },
				danger = { ink = "{danger}", wash = "alpha({danger}, 0.12)", edge = "alpha({danger}, 0.28)" },
			},
			-- A badge on any colour (a pull request's state, a tone's ink): the
			-- colour laid under it this faint, its ink this strong (the
			-- sidebar's pull request badge, generalised).
			-- Lit (under the pointer, a badge that's a link): stronger, as
			-- Zeron's sidebar badge brightens.
			badge = { wash = 0.12, ink = 0.9, lit = { wash = 0.22, ink = 1 } },
			-- A review's verdict, as a chip: its hue.
			review = { approved = "{success}", changes = "{danger}", commented = "{text.secondary}", pending = "{warning}" },
			-- Checks on a commit: passing, failing, running.
			checks = { passing = "{success}", failing = "{danger}", running = "{warning}" },
			-- A diff's counts: what it adds and what it takes away.
			-- (And a line a comment rests on, lit: its chip pointed at, or a
			-- range being chosen.)
			diff = { added = "{success}", removed = "{danger}", comment_wash = "alpha({accent}, 0.14)" },
			-- A dialog or sheet: its panel over the dimmed window (the palette's
			-- denser glass: a menu's tint would go black on the dim), its
			-- corner, the dim, and its width at each size.
			dialog = {
				tint = "{surface.overlay}",
				solid = "{zeron.menu.solid}",
				shadow = "{shadow.lg}",
				-- The bare stage it stands on: no shadow of its own.
				stage = "0 0 0 0 #00000000",
				radius = 16,
				-- (The dim is zgx's own `backdrop`, set by each theme.)
				scrim = "{backdrop}",
				sm = 380,
				md = 460,
				lg = 560,
				sheet = 400,
			},
			-- A toast: its card (as a menu's panel), and its width.
			toast = { width = 340 },
			-- The lightbox (ds/lightbox): the room a fitted picture keeps from
			-- the window's edges, and under it for the controls; their island's
			-- inset from the foot; the scrim's blur (its fill is each theme's).
			lightbox = { inset = 40, foot = 72, island_inset = 16, blur = 24, scrim = "{backdrop}", shadow = "{zeron.dialog.stage}" },
			-- Settings: the column a page reads in, a row's least height, the
			-- room a row's control keeps at its end, and a group's card (a
			-- quiet fill with a hairline, the rows' dividers fainter).
			settings = {
				measure = 640,
				row = 52,
				control = 240,
				card = "{surface.base}",
				edge = "{border.subtle}",
				rule = "{border.subtle}",
			},
			-- Onboarding: the hero's column; an option card's fill, its edge,
			-- and the ring it wears chosen (the accent, inside its edge: a
			-- ring, never a fill or a shadow).
			hero = { measure = 440 },
			-- A setup step that asks for something: the one width its field,
			-- the address or note under it and the way on share (a name's
			-- worth of field, narrower than the hero's measure, so the three
			-- read as one shape); and a step that lists (the organizations
			-- you're in): a row's tile, its two lines and the control at its
			-- end.
			form = { measure = 360, list = 480 },
			option = { bg = "{surface.base}", edge = "{border}", ring = "{accent}" },
			-- Display type: Geist, large and tight, for the few words that open
			-- a screen.
			display = { size = 40, line = 46 },
			-- A window's own opaque background, where it has no glass
			-- (onboarding): the themes give it a colour of its own.
			window = "{surface.base}",
			-- A wall of member cards beside or behind a screen's words: the
			-- window's own colour laid over each card, so a printed card
			-- stands in the room's light rather than shining out of it (the
			-- dark theme dims them a little; light, where the paper is near
			-- the window's own white, leaves them be). Lifted from the card
			-- under the pointer.
			-- `veil`: the same laid thick, for a wall that is the room's
			-- wallpaper behind the words (a field of small cards, each one
			-- shown whole only under the pointer). And each card's shadow
			-- there: the card's own on a light window, none on a dark one,
			-- where many pooled into a halo.
			wall = {
				scrim = "alpha({zeron.window}, 0)",
				veil = "alpha({zeron.window}, 0.56)",
				shadow = "{zeron.card.shadow}",
			},
			-- A member card: a printed card, the same in either theme. Paper,
			-- its inks (strong, muted, faint, and a violet one),
			-- rules; its type (sans for the name, mono for the rest) and its
			-- shadow.
			card = {
				paper = "#f1efe8",
				ink = "#161616",
				muted = "#55534e",
				faint = "#9a978e",
				violet = "#5a46c8",
				rule = "#16161633",
				light = "#ffffff",
				pill = "#16161680",
				edge = "#0000001f",
				caption = "{text.tertiary}",
				mono = "{font.mono}",
				size = { name = 34, tagline = 12, number = 12, mark = 16, tag = 11, caption = 11, badge = 10 },
				shadow = "0 24 48 0 #0000004d",
				-- Its corners: a printed card's, apart from the interface's
				-- radius scale.
				radius = 16,
				-- A card shown as a gift: how much its own sky, blurred large
				-- behind it, lights the window.
				glow = 0.5,
			},
			-- A harness mark's tile (light in both themes: the marks are drawn
			-- for white).
			mark = { bg = "#ffffff", border = "#ffffffcc" },
			-- What stands on the intro's scene (night, then sunrise), the same
			-- in either theme, since it stands on art, not on a window: the
			-- words, light; and the way in, setup's forward pill
			-- (ds.Onboarding.WayIn), solid white with dark words, a touch
			-- greyer as it's touched. (`blur` is the glass it once was, still
			-- warmed by the intro.)
			scene = {
				words = "#ffffff",
				pill = {
					tint = "#ffffff",
					hover = "#ededed",
					active = "#e2e2e2",
					edge = "#ffffff",
					ink = "#0a0a0a",
					blur = 18,
				},
			},
			-- Code's colours, by what each piece of it is (ds/syntax). Here in
			-- the standard tokens, so any theme colours code; Zeron's themes pick
			-- palette hues for them. The structure (comments, punctuation,
			-- operators) is the text's own ink, fainter, so it takes the colour
			-- of whatever is behind it, glass or opaque, and the words carry the
			-- hue.
			syntax = {
				text = "{text.primary}",
				keyword = "{accent}",
				string = "{success}",
				escape = "{warning}",
				regex = "{info}",
				number = "{warning}",
				constant = "{warning}",
				comment = "{text.tertiary}",
				["function"] = "{info}",
				macro = "{accent}",
				type = "{info}",
				variable = "{danger}",
				property = "{text.primary}",
				attribute = "{warning}",
				label = "{danger}",
				operator = "{text.secondary}",
				punctuation = "{text.tertiary}",
				tag = "{danger}",
				heading = "{accent}",
				link = "{info}",
				url = "{text.tertiary}",
				emphasis = "{text.primary}",
				strong = "{text.primary}",
				quote = "{text.secondary}",
				inserted = "{success}",
				deleted = "{danger}",
				hunk = "{accent}",
				meta = "{text.secondary}",
			},
		},
	}
	ui.theme.load("ds/themes/zeron-dark.json")
	ui.theme.load("ds/themes/zeron-light.json")
	-- A template's colours (its theme.luau; Zeron's own returns nil), worn
	-- over those themes through the one mapping (ds/palettes): what each
	-- scheme wears follows. (Guarded: a plugin, lent the design system,
	-- has no app of its own to read.)
	local palettes = require("@self/palettes")
	local ok, family = pcall(require, "@app/theme")
	ui.theme.follow_system(palettes.wear(if ok then family else nil))
	-- Corners made from others (a theme's tokens take no arithmetic), made
	-- again as the theme's change, each its container's corner less its
	-- edge and how far in it sits (inner = outer − inset, always):
	-- - a menu row's, in its panel (the group's inset);
	-- - a tag's ×, in the tag (the 3 it stands in);
	-- - in the composer's pill: a chip of what it carries, and anything at
	--   the strip's inset (an approval's command); a question's option,
	--   which hangs 8 past that inset, and its number cap, 8 inside the
	--   option;
	-- - a tray's row (and its head), the tray's padding in; a queued
	--   message's thumbnail, its row's inset in.
	local control = require("@self/control")
	ui.effect(function()
		local chip = control.nest("$zeron.composer.radius", 1, "$zeron.composer.chip.inset")
		local option = control.nest("$zeron.composer.radius", 1, control.nest("$zeron.composer.chip.inset", 8))
		local tray_row = control.nest("$zeron.composer.tray.radius", 1, "$space.1")
		ui.theme.tokens {
			zeron = {
				menu = { row_radius = control.nest("$radius.lg", 1, "$space.1") },
				tag = { remove_radius = control.nest("$radius.md", 3) },
				composer = {
					chip = { radius = chip },
					option = { radius = option, key_radius = control.nest(option, "$space.2") },
					tray = { row_radius = tray_row, thumb_radius = control.nest(tray_row, "$space.1") },
				},
			},
		}
	end)
end

return M
