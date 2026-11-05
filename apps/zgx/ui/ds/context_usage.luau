-- How full a model's context is, as the composer's ring shows it and its
-- card says it (Zeron's crates/ui/src/context_usage.rs): what the agent
-- reports (tokens in it, the window's size, either missing), how full that
-- is, the ink it reads in (the text's own, amber from three quarters, red
-- from nine tenths), and the card itself.
--
--   context_usage.fraction(u)  0 to 1, or nil while it can't be said
--   context_usage.lines(u)     the card's two lines, as Zeron writes them
--   context_usage.ink(f)       the ring's and the bar's ink at `f`
--   Card { usage = fn }        the card (ds/hover_card's), "Context window"
--                              and how full at its head, a slim bar, the
--                              lines (followed as the usage changes)

local locale = require("@self/locale")
local HoverCard = require("@self/hover_card")
local progress = require("@self/progress")

export type Usage = { tokens: number?, window: number? }

local M = {}

-- A count of tokens, its thousands grouped ("408,389").
function M.grouped(n: number): string
	local digits = string.format("%d", math.max(0, math.floor(n + 0.5)))
	local out = string.reverse((string.gsub(string.reverse(digits), "(%d%d%d)", "%1,")))
	return (string.gsub(out, "^,", ""))
end

function M.fraction(u: Usage?): number?
	if u and u.tokens and u.window and u.window > 0 then
		return math.clamp(u.tokens / u.window, 0, 1)
	end
	return nil
end

-- Used of the window and what is left; or what is known of it (tokens
-- with no window, a window with nothing counted yet, nothing at all).
function M.lines(u: Usage?): (string, string?)
	local tokens = if u then u.tokens else nil
	local window = if u then u.window else nil
	if tokens and window and window > 0 then
		return locale.s("ds.composer.context.of", { used = M.grouped(tokens), window = M.grouped(window) }),
			locale.s("ds.composer.context.left", { n = M.grouped(math.max(window - tokens, 0)) })
	elseif tokens then
		return locale.s("ds.composer.context.used_only", { n = M.grouped(tokens) }), locale.s("ds.composer.context.no_limit")
	elseif window and window > 0 then
		return locale.s("ds.composer.context.capacity", { n = M.grouped(window) }), locale.s("ds.composer.context.waiting")
	end
	return locale.s("ds.composer.context.unreported"), nil
end

function M.ink(f: number): string
	return if f >= 0.9 then "$danger" elseif f >= 0.75 then "$warning" else "$text.secondary"
end

local function tone(f: number): progress.Tone
	return if f >= 0.9 then "danger" elseif f >= 0.75 then "warning" else "neutral"
end

-- The card: a hover card (ds/hover_card: the menus' glass, framed once),
-- a group's inset (its heading's row, then the
-- body on the heading's text edge). The percentage sits at the heading's
-- end in the ring's ink; the figures are tabular, so they don't jitter as
-- they count.
function M.Card(props: { usage: () -> Usage? }): Node
	local usage = props.usage
	local function fraction(): number?
		return M.fraction(usage())
	end
	local function known(): boolean
		return fraction() ~= nil
	end
	return HoverCard.Card {
		id = "composer-context-card",
		w = 248,
		p = "$space.1",
		gap = 0,
		ui.row {
			h = "$zeron.menu.heading",
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			ui.text {
				locale.t("ds.composer.context.heading"),
				grow = 1,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$text.tertiary",
			},
			ui.show(known, ui.text {
				function(): string
					return locale.s("ds.composer.context.percent", { n = math.floor((fraction() or 0) * 100 + 0.5) })
				end,
				tabular = true,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = function(): string
					return M.ink(fraction() or 0)
				end,
			}),
		},
		ui.column {
			px = "$space.2",
			pt = "$space.1",
			pb = "$space.2",
			gap = "$space.2",
			ui.show(known, progress.ProgressBar {
				value = fraction,
				size = "thin",
				w_full = true,
				label = locale.t("ds.composer.context.heading"),
				tone = function(): progress.Tone
					return tone(fraction() or 0)
				end,
			}),
			ui.column {
				gap = "$zeron.space.hair",
				ui.text {
					function(): string
						return (M.lines(usage()))
					end,
					id = "composer-context-used",
					tabular = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.show(function(): boolean
					local _, second = M.lines(usage())
					return second ~= nil
				end, ui.text {
					function(): string
						local _, second = M.lines(usage())
						return second or ""
					end,
					id = "composer-context-left",
					tabular = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
				}),
			},
		},
	}
end

return M
