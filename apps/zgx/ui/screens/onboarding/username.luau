-- Right after sign-in, for an account with no username yet: choosing one.
-- It's the card's address (zeron.sh/c/mara), so there's no card without
-- one, and it's chosen once. The gallery's Claim your username
-- (demos/example_username) is the design; this asks Zeron for real.
--
-- One field; under it the address it makes, then, a beat after the typing
-- stops, what Zeron says of it (free, taken, reserved) or what keeps it
-- from being a username yet, said before anyone is asked. The way on opens
-- only once it's free; it claims the name and goes on to the card. Gone
-- wrong, the line under the title says so and the button offers another
-- go; nothing else moves. Signed into the wrong account: Sign out is on
-- the look (Make it yours), one step back.

local ds = require("@ds")
local Onboarding = ds.Onboarding
local api = require("@account/api")
local Icon = require("@ds/icon")
local state = require("./state")

export type Props = {
	-- Its place among setup's numbered steps, and the way back.
	step: number?,
	count: number?,
	on_back: () -> (),
	-- Claimed: on to the card, with it.
	on_claimed: (card: api.Card) -> (),
	-- Signed out from here (or the session went): back to the start.
	on_signed_out: () -> (),
	-- Already has one (claimed elsewhere a moment ago): past this step.
	on_has_one: () -> (),
}

-- How long the typing has to rest before Zeron is asked.
local REST = 400

return function(props: Props): Node
	local typed = ui.use_state("")
	-- What the address line says: a problem (empty, invalid, start, short,
	-- long), or "checking", "available", "taken", "reserved", "unknown"
	-- (Zeron couldn't be asked).
	local check = ui.use_state("empty")
	-- Claiming, and what the last claim ended in (an api word), if not a card.
	local saving = ui.use_state(false)
	local failed = ui.use_state(nil :: string?)
	-- Whether the account has one is asked at sign-in, but the answer can
	-- come after this step opens (or not at all): asked again here, and the
	-- field stays hidden until it's known, so an account with a username
	-- never sees it.
	-- (Asked once a visit: a hot reload doesn't ask again, the answer kept.)
	local known = {
		get = function(_): boolean
			return state.answered("username")
		end,
	}
	-- (Answers heard only while the step is here: Back while one is asked
	-- leaves it unanswered.)
	local here = true
	ui.on_cleanup(function()
		here = false
	end)
	state.ask_until_answered("username", function()
		api.username():after(function(name: string?)
			if not here then
				return
			end
			state.answer("username")
			if name then
				props.on_has_one()
			end
		end, function(err: any)
			if not here then
				return
			end
			if err.message == "signed_out" then
				props.on_signed_out()
			else
				state.answer("username")
			end
		end)
	end)

	local function handle(): string
		return api.normalise(typed:get())
	end

	-- Asked a beat after the typing stops; only the last question asked
	-- is answered.
	local asked = 0
	ui.effect(function()
		local u = api.normalise(typed:get())
		asked += 1
		local mine = asked
		local why = api.problem(u)
		if why then
			check:set(why)
			return
		end
		check:set("checking")
		ui.after(REST, function()
			if mine ~= asked or not here then
				return
			end
			api.check(u):after(function(answer: api.Check)
				if mine == asked and here then
					check:set(if answer.available then "available" else answer.reason or "taken")
				end
			end, function(err: any)
				if mine ~= asked or not here then
					return
				end
				if err.message == "signed_out" then
					props.on_signed_out()
				else
					check:set("unknown")
				end
			end)
		end)
	end)

	local function claim()
		if saving:peek() then
			return
		end
		local u = handle()
		saving:set(true)
		failed:set(nil)
		api.claim(u):after(function(card: api.Card)
			saving:set(false)
			props.on_claimed(card)
		end, function(err: any)
			saving:set(false)
			local word = err.message
			if word == "taken" or word == "reserved" or word == "invalid" then
				-- Said where it's said for a name: on the address line.
				check:set(if word == "invalid" then "invalid" else word)
			elseif word == "already_claimed" then
				props.on_has_one()
			elseif word == "signed_out" then
				props.on_signed_out()
			else
				failed:set(word)
			end
		end)
	end

	local function free(): boolean
		return check:get() == "available"
	end

	-- What the address line says at its end: a glyph and a word in the ink
	-- of the news, one at a time, in a slot that keeps one line's height
	-- whatever it says.
	local function status(kind: string, glyph: Node?, label: Label, ink: string): Node
		return ui.show(function()
			return check:get() == kind
		end, ui.row {
			gap = "$space.1",
			items = "center",
			shrink = 0,
			color = ink,
			glyph,
			ui.text { label, text_size = "$font.size.sm", line_height = "$font.line.sm" },
		})
	end
	local function mark(name: Icon.Name): Node
		return ui.div { y = -0.5, ds.Icon { name = name, size = 12 } }
	end

	local address = ui.row {
		w_full = true,
		items = "center",
		justify = "between",
		gap = "$space.3",
		ui.row {
			items = "center",
			min_w = 0,
			ui.text { "zeron.sh/c/", text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
			ds.FadeText {
				handle,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = function()
					local c = check:get()
					return if c == "taken" or c == "reserved" or c == "invalid" or c == "start" then "$zeron.tone.danger.ink" else "$text.secondary"
				end,
			},
		},
		ui.row {
			items = "center",
			-- (The ring's name is a string, read as it's built: it says it with
			-- its progress.)
			-- zgx-lint: allow(untracked-get)
			status("checking", ds.ProgressRing { size = 12, tone = "neutral", label = ui.t("onboarding.username.checking_label"):get() }, ui.t("onboarding.username.checking"), "$text.tertiary"),
			status("available", mark("check-circle"), ui.t("onboarding.username.available"), "$zeron.tone.success.ink"),
			status("taken", mark("close-circle"), ui.t("onboarding.username.taken"), "$zeron.tone.danger.ink"),
			status("reserved", mark("close-circle"), ui.t("onboarding.username.reserved"), "$zeron.tone.danger.ink"),
			status("invalid", mark("info-circle"), ui.t("onboarding.username.invalid"), "$zeron.tone.danger.ink"),
			status("start", mark("info-circle"), ui.t("onboarding.username.start"), "$zeron.tone.danger.ink"),
			status("short", nil, ui.t("onboarding.username.short", { n = api.MIN }), "$text.tertiary"),
			status("long", nil, ui.t("onboarding.username.long", { n = api.MAX }), "$text.tertiary"),
			status("unknown", mark("info-circle"), ui.t("onboarding.username.unknown"), "$text.tertiary"),
		},
	}

	return Onboarding.Step {
		step = props.step,
		count = props.count,
		-- Back to the look, and no skipping (a username is the card;
		-- skipping was at sign-in). Signed into the wrong account: Sign out
		-- is on the look, one step back.
		back = Onboarding.Back { on_click = props.on_back },
		continue = {
			label = function()
				return if saving:get()
					then ui.t("onboarding.username.saving"):get()
					elseif failed:get() then ui.t("onboarding.try_again"):get()
					else ui.t("common.continue"):get()
			end,
			disabled = function()
				return saving:get() or not known:get() or not free()
			end,
			on_click = claim,
		},
		ui.show(function()
			return known:get()
		end, Onboarding.Form {
			title = ui.t("onboarding.username.title") :: any,
			body = function()
				local word = failed:get()
				return if word
					then ui.t("onboarding.username.failed", { name = handle(), says = api.says(word) }):get()
					else ui.t("onboarding.username.body"):get()
			end,
			tone = function()
				return if failed:get() then "$zeron.tone.danger.ink" else "$text.secondary"
			end,
			ui.column {
				w_full = true,
				gap = "$space.2",
				ds.TextField {
					value = typed,
					name = ui.t("onboarding.username.field") :: any,
					placeholder = ui.t("onboarding.username.placeholder") :: any,
					size = "lg",
					w_full = true,
					autofocus = true,
					-- Enter claims it, once it's free (as the way on would).
					on_key = function(e)
						if e.key == "Enter" and check:peek() == "available" and not saving:peek() then
							claim()
							return true
						end
						return nil
					end,
				},
				address,
			},
		}),
	}
end
