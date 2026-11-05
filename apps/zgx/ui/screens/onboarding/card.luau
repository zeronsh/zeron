-- Right after the username: your member card, the one gift in setup, as the
-- gallery's Your card draws it (demos/example_card, its Stage): the card
-- centred on a glow of its own sky, a line above it, its address and the
-- ways to share it under it, and Continue alone at the bottom right.
--
-- The card is the real one, from Zeron (`/cards/:username`): its scene,
-- foil and sky as the server made them, your number, first name, the day
-- you joined and any badges. Handed the card the claim returned, it shows
-- it at once; opened without one (a relaunch here), it asks for yours, and
-- until it comes the room stays empty, so the card is still dealt on.

local ds = require("@ds")
local Onboarding = ds.Onboarding
local CardActions = require("@ds/card_actions")
local card_scenes = require("@ds/card_scenes")
local api = require("@account/api")
local state = require("./state")

export type Props = {
	-- Its place among setup's numbered steps, and the way back (past the
	-- name, which is yours for good).
	step: number?,
	count: number?,
	on_back: () -> (),
	on_continue: () -> (),
}

-- Setup's window (the glow is laid out in it), and the card's zoom.
local WIN_W, WIN_H = 680, 500
local Z = 0.66

-- The card's own sky behind it, blurred large and faint, dissolving into
-- the window: the room lit by the card. Each fade band runs to the box's
-- middle, so it falls off evenly on every side (example_card's glow).
local GLOW_BLUR = 36
local function glow(sky: string): Node
	local box = { left = 40, right = 40, top = -40, bottom = 40 }
	local w = WIN_W - box.left - box.right
	local h = WIN_H - box.top - box.bottom
	return ui.div {
		absolute = true,
		left = box.left,
		right = box.right,
		top = box.top,
		bottom = box.bottom,
		overflow = "hidden",
		fade_sides = { h / 2, w / 2, h / 2, w / 2 },
		pointer_events = "none",
		ui.div {
			w_full = true,
			h_full = true,
			blur = GLOW_BLUR,
			opacity = "$zeron.card.glow",
			ui.image { src = sky, w_full = true, h_full = true, fit = "cover" },
		},
	}
end

-- Its title, as every step's (ds.Onboarding.Form's).
local function headline(text: Label): Node
	return ds.Text { text, style = "title", align = "center", wrap = true, heading = 1 }
end

local function quiet(text: string | () -> string, tone: string?): Node
	return ui.text {
		text,
		align = "center",
		wrap = true,
		text_size = "$font.size.sm",
		line_height = "$font.line.sm",
		color = tone or "$text.tertiary",
	}
end

-- A URL's query value, escaped.
local function escape(s: string): string
	return (string.gsub(s, "[^%w%-%._~]", function(ch: string): string
		return string.format("%%%02X", string.byte(ch))
	end))
end

-- What Share on X drafts: a line, and the card's link (its page draws the
-- card in the post).
-- (In the language chosen as it's shared.)
local function draft(card: api.Card): string
	local line = ui.t("onboarding.card.draft"):get()
	return `https://x.com/intent/post?text={escape(line)}&url={escape(api.link(card.username))}`
end

-- The card and all round it, once there is one.
local function stage(card: api.Card, shared: State<string>): Node
	local face = api.member(card)
	-- Dealt once: it rises and catches the light the first time it shows,
	-- not again as a hot reload builds it again.
	face.arrive = not state.card_dealt:peek()
	state.card_dealt:set(true)
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		glow(card_scenes.layers(card.scene)[1].src),
		ui.column {
			absolute = true,
			left = 0,
			right = 0,
			-- The card's top where every step's title stands.
			top = Onboarding.TITLE_TOP,
			items = "center",
			gap = "$space.3",
			-- (No title: the card is the page.)
			ui.div { zoom = Z, shrink = 0, ds.MemberCard(face :: any) },
			quiet(api.address(card.username)),
		},
	}
end

return function(props: Props): Node
	-- The card claimed a moment ago (the username step's), else yours,
	-- asked for: kept in setup's state, so a hot reload shows it at once.
	local card = state.card
	-- Why it couldn't be had, as an api word.
	local failed = ui.use_state(nil :: string?)
	local shared = ui.use_state("ready")
	local have = ui.computed(function(): boolean
		return card:get() ~= nil
	end)

	-- Asked for: the latest ask's answers only, and none once the step
	-- has gone (Back while it loads).
	local asked = 0
	ui.on_cleanup(function()
		asked += 1
	end)
	local function load()
		asked += 1
		local this = asked
		failed:set(nil)
		local function fail(why: string?)
			if this == asked then
				failed:set(why or "failed")
			end
		end
		api.username():after(function(name: string?)
			if this ~= asked then
				return
			end
			if not name then
				fail("failed")
				return
			end
			api.card(name):after(function(c: api.Card?)
				if this ~= asked then
					return
				end
				if c then
					card:set(c)
				else
					fail("failed")
				end
			end, function(err: any)
				fail(err.message)
			end)
		end, function(err: any)
			fail(err.message)
		end)
	end
	if not card:peek() then
		load()
	end

		-- The ways to share it, in the footer's middle (the card takes the
	-- room): its glyphs, each its words as a tooltip. Share on X opens a
	-- post's draft on X in the browser, the card's link in it (X draws the
	-- card from it); Copy link puts the link on the clipboard. (Save image
	-- is the gallery's design until the app can write a picture.)
	local share = ui.show(have, CardActions {
		state = shared,
		form = "glyphs",
		w = 156,
		-- As tall as Back and Continue beside it.
		h = ui.theme.get("control.height") :: number?,
		on_share = function()
			local c = card:peek()
			if c then
				ui.invoke("open_url", draft(c))
			end
		end,
		on_copy = function()
			local c = card:peek()
			if c then
				ui.clipboard.write(api.link(c.username))
			end
		end,
	})

	return Onboarding.Step {
		step = props.step,
		count = props.count,
		back = Onboarding.Back { on_click = props.on_back },
		middle = share,
		continue = {
			label = function()
				return if failed:get() then ui.t("onboarding.try_again"):get() else ui.t("common.continue"):get()
			end,
			on_click = function()
				if failed:peek() then
					load()
				else
					props.on_continue()
				end
			end,
		},
		-- Laid over the window: the footer paints after it, so the way on
		-- stands on the glow.
		ui.presence(have, function(there: boolean): Node
			local c = card:peek()
			if there and c then
				return stage(c, shared)
			end
			return ui.column {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				items = "center",
				justify = "center",
				gap = "$space.2",
				ui.show(function()
					return failed:get() ~= nil
				end, ui.column {
					items = "center",
					gap = "$space.2",
					headline(ui.t("onboarding.card.failed_title")),
					quiet(function()
						return ui.t("onboarding.card.failed_body", { says = api.says(failed:get()) }):get()
					end, "$text.secondary"),
				}),
			}
		end, { enter = { opacity = { 0, 1 }, duration = 240 }, exit = { opacity = 0, duration = 160 } }),
	}
end
