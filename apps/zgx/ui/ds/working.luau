-- That an agent is working on a reply, as Zeron's transcript says it under
-- the turn: the working loader, a word for it ("Noodling…") that rolls to
-- another every seven seconds, and how long it has been at it.
--
-- `Working { since = seconds }`: `since` in `ui.now() / 1000` seconds, when
-- the turn began (or a function of it); `seed` picks where the words start
-- (a chat's own). `sending`: the message is on its way, its turn not yet
-- begun: "Sending…" and no clock, as Zeron's trailer bridges that gap
-- (transcript.rs `sending_bridge`); the word and the clock start with the
-- turn. `size = "sm"`: smaller, for a row in a list (a crew thread's, a
-- machine's).

local RollText = require("@self/roll_text")
local status = require("@self/status")
local locale = require("@self/locale")

export type Props = { since: number | () -> number, seed: number?, sending: (() -> boolean)?, size: ("md" | "sm")? }

-- Zeron's words (transcript.rs, `FLAVOUR_WORDS`, their keys), each held
-- this long.
local WORDS = {
	"ds.working.words.zeroning",
	"ds.working.words.thinking",
	"ds.working.words.pondering",
	"ds.working.words.scheming",
	"ds.working.words.brewing",
	"ds.working.words.weaving",
	"ds.working.words.tinkering",
	"ds.working.words.musing",
	"ds.working.words.composing",
	"ds.working.words.sifting",
	"ds.working.words.untangling",
	"ds.working.words.distilling",
	"ds.working.words.sketching",
	"ds.working.words.plotting",
	"ds.working.words.riffing",
	"ds.working.words.combobulating",
	"ds.working.words.percolating",
	"ds.working.words.marinating",
	"ds.working.words.noodling",
	"ds.working.words.puzzling",
	"ds.working.words.conjuring",
}
local ROTATE = 7

-- Once a second, one clock for every line showing (the script's own): the
-- clock shows whole seconds, and the word turns on one of them. One timer,
-- so all of them change in the same frame rather than each in its own. It
-- runs only while a line shows (the script's effect starts it as the first
-- comes, and its rerun drops it as the last goes): with none, no script
-- runs at all.
local now = ui.state("ds_working_now", ui.now() / 1000)
local ticking = ui.state("ds_working_ticking", false)
local showing = 0
ui.effect(function()
	if ticking:get() then
		ui.every(1000, function()
			now:set(ui.now() / 1000)
		end)
	end
end)

return ui.component(function(props: Props): Node
	showing += 1
	-- (Shown after none did: the clock as it is now, not as it last was.)
	if showing == 1 then
		now:set(ui.now() / 1000)
		ticking:set(true)
	end
	ui.on_cleanup(function()
		showing -= 1
		if showing == 0 then
			ticking:set(false)
		end
	end)
	local seed = props.seed or 0
	local function since(): number
		local given: any = props.since
		if type(given) == "function" then
			local call: () -> number = given
			return call()
		end
		return given
	end
	local function sending(): boolean
		return props.sending ~= nil and props.sending()
	end
	local function seconds(): number
		return math.max(0, math.floor(now:get() - since()))
	end
	local small = props.size == "sm"
	local text_size = if small then "$font.size.sm" else "$font.size.md"
	local h = if small then 20 else 26
	-- The line's room, as wide as where it stands and its own height, read
	-- once: the line itself is then a box of a set size, laid out by itself
	-- (zgx's layout boundary), so its clock ticking, its word rolling and
	-- its ring stepping lay out the line alone, never the transcript or the
	-- crew around it. (Its own width given in points: a fraction of its
	-- parent's isn't a boundary.)
	local room = ui.use_state(0)
	return ui.div {
		w_full = true,
		h = h,
		min_w = 0,
		on_resize = function(sz: { w: number, h: number })
			if math.abs(sz.w - room:peek()) >= 0.5 then
				room:set(sz.w)
			end
		end,
		ui.row {
			class = "ds-working",
			w = function(): number
				return room:get()
			end,
			h = h,
			shrink = 0,
			gap = if small then "$zeron.space.tight" else "$space.2",
			min_w = 0,
			items = "center",
			selectable = false,
			label = locale.t("ds.working.label"),
			status.StatusIcon { kind = "working", size = if small then 14 else 16 },
			RollText {
				function(): string
					if sending() then
						return locale.s("ds.working.sending")
					end
					local word = WORDS[(seed + seconds() // ROTATE) % #WORDS + 1]
					return locale.s("ds.working.word", { word = locale.s(word) })
				end,
				text_size = text_size,
				color = "$text.secondary",
				shrink = 0,
			},
			ui.text {
				function(): string
					return if sending() then "" else status.elapsed(seconds())
				end,
				tabular = true,
				shrink = 0,
				text_size = text_size,
				color = "$text.tertiary",
			},
		},
	}
end)
