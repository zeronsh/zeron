--!strict
-- A card dropped on the board: a shortcut to its own menu's action, never
-- a column set (where a card stands stays the engine's). The action is
-- sent as the menu sends it (ds/orch_board_actions: its toast, or why it
-- was refused), and the card moves when the engine says it has, with the
-- board's own hold, travel and landing (ds/orch_board_motion).
--
--   from \ onto     Needs you      Running            Planned     Done
--   Needs you       (home)         refused: answer    refused     Mark done
--   Snoozed         Bring back     refused: answer    refused     Mark done
--   Running         refused        (home)             Pause       Mark done
--   Planned paused  refused        Resume             (home)      Mark done
--   Planned, none   refused        Start now          (home)      Mark done
--     started
--   Planned, parked refused        refused: parked    (home)      Mark done
--     with a thread
--   Done, dropped   refused        Reopen and start   Reopen      (home)
--
-- Needs you takes no card: only a question puts one there. A card asking
-- the person stays in Needs you paused (proposal decision 18), so it isn't
-- paused by a drop. Drop (closed as dropped) is the menu's alone: it asks
-- once more there.
--
-- While a card is carried, each target that takes it is edged; the one
-- under the pointer is washed and names what a drop there does in its
-- head; one that refuses it fades, and says why in its head while the
-- pointer is over it. A drop it refuses, or one on the card's own place,
-- sends and says nothing. The card still carries its reference
-- (ds/orch_ref) to a composer: the payload is the reference's own.
--
-- (The verb is said on the target, not under the dragged picture: macOS
-- draws that picture once into a window of its own.)
--
--   local drop = Drop.make(ctx, act)
--   drop.target(place) -> props      a column's, or Planned's drawer's
--   drop.Lit(place, radius) -> Node  its edge and wash, over it
--   drop.Badge(place) -> Node        its verb, or why not, in its head
--   drop.faded(place) -> boolean     refusing the card carried
--   drop.carried() -> string?        the card carried (faded where it stands)
--   drop.source(node, id) -> node    a card's drag, told to the board
--   Drop.verdict(entry, onto, can) -> Verdict?

local Icon = require("@self/icon")
local locale = require("@self/locale")
local Ref = require("@self/orch_ref")
local Ctx = require("@self/orch_board_ctx")

local M = {}

-- The card carried over the board, and the target under the pointer: one
-- at a time on every board (one pointer drags).
M.carry = ui.shared("orch_board_carry", nil :: { id: string, over: string? }?)

-- The target a card's place is: a drawer's cards are its column's, but
-- Planned is a target of its own.
M.HOME = {
	needs_you = "needs_you",
	snoozed = "needs_you",
	running = "running",
	planned = "planned",
	done = "done",
	dropped = "done",
} :: { [string]: string }

export type Verdict = {
	ok: boolean,
	-- Taken: what it does (an Actions call), its icon and words' key.
	action: string?,
	icon: Icon.Name?,
	-- Its words (taken), or why not (refused).
	key: string,
}

local function yes(action: string, icon: Icon.Name, key: string): Verdict
	return { ok = true, action = action, icon = icon, key = key }
end
local function no(key: string): Verdict
	return { ok = false, key = key }
end

-- What dropping `e` on `onto` does, given which host actions there are
-- (`can("pause")`, …): nil on its own place, or where no action applies.
function M.verdict(e: Ctx.Entry, onto: string, can: (string) -> boolean): Verdict?
	local from = e.place
	-- (A snoozed card's column takes it back.)
	local home = M.HOME[from]
	if home == nil or (home == onto and not (from == "snoozed" and onto == "needs_you")) then
		return nil
	end
	local closed = from == "done" or from == "dropped"
	local asking = from == "needs_you" or from == "snoozed"
	local v: Verdict? = nil
	if onto == "needs_you" then
		if from == "snoozed" and e.asks[1] then
			v = yes("bring_back", "bell", "orch.board.bring_back")
		else
			v = no("orch.board.refused.question")
		end
	elseif onto == "running" then
		if closed then
			v = yes("reopen_start", "action-play", "orch.board.reopen_start")
		elseif asking then
			v = no("orch.board.refused.answer")
		elseif e.status == "paused" then
			v = yes("resume", "action-play", "orch.board.resume")
		elseif #(e.item.threads or {}) == 0 then
			v = yes("start", "action-play", "orch.board.start_now")
		else
			v = no("orch.board.refused.parked")
		end
	elseif onto == "planned" then
		if closed then
			v = yes("reopen", "restart", "orch.board.reopen")
		elseif asking then
			v = no("orch.board.refused.answer")
		else
			v = yes("pause", "action-pause", "orch.board.pause")
		end
	elseif onto == "done" then
		v = yes("close", "check-circle", "orch.board.mark_done")
	end
	-- (A host without the action: nothing to do there.)
	local needs: { [string]: { string } } = {
		bring_back = { "snooze" },
		reopen_start = { "resume" },
		resume = { "resume" },
		start = { "start" },
		reopen = { "reopen" },
		pause = { "pause" },
		close = { "close" },
	}
	if v and v.action then
		for _, what in needs[v.action] do
			if not can(what) then
				return nil
			end
		end
	end
	return v
end

export type Drop = {
	carried: () -> string?,
	over: () -> string?,
	verdict_of: (onto: string) -> Verdict?,
	faded: (onto: string) -> boolean,
	target: (onto: string) -> { [string]: any },
	source: (node: { [any]: any }, id: string) -> { [any]: any },
	Lit: (onto: string, radius: string) -> Node,
	Badge: (onto: string) -> Node,
}

function M.make(ctx: Ctx.Ctx, act: any): Drop
	local drop = {} :: Drop

	-- The card carried, when it is this board's.
	local function entry(): Ctx.Entry?
		local c = M.carry:get()
		return if c then ctx.entry(c.id) else nil
	end
	function drop.carried(): string?
		local e = entry()
		return e and e.id
	end
	function drop.over(): string?
		local c = M.carry:get()
		return if c and entry() then c.over else nil
	end
	function drop.verdict_of(onto: string): Verdict?
		local e = entry()
		return if e then M.verdict(e, onto, act.can) else nil
	end
	function drop.faded(onto: string): boolean
		local v = drop.verdict_of(onto)
		return v ~= nil and not v.ok
	end

	-- The card a payload carries, when it is this board's.
	local function card_of(payload: any): Ctx.Entry?
		local r = Ref.of(payload)
		if not r or r.kind ~= "card" then
			return nil
		end
		return ctx.entry(r.id)
	end
	local function set_over(id: string, onto: string?)
		local c = M.carry:peek()
		if not c or c.id ~= id or c.over ~= onto then
			M.carry:set({ id = id, over = onto })
		end
	end

	-- What a drop does: the menu's own action.
	local function run(e: Ctx.Entry, v: Verdict)
		local a = v.action
		if a == "bring_back" then
			act.snooze(e.asks[1], nil)
		elseif a == "close" then
			act.close(e, false)
		elseif a == "reopen_start" then
			act.reopen_start(e)
		elseif a then
			act[a](e)
		end
	end

	function drop.target(onto: string): { [string]: any }
		return {
			-- (Every card of this board is heard, a refusing target too:
			-- under the pointer it says why. Its own place takes it too, to
			-- do nothing: a Planned card let go on Planned isn't Running's.)
			drop_when = function(payload: any): boolean
				return card_of(payload) ~= nil
			end,
			on_drag_over = function(payload: any)
				local e = card_of(payload)
				if e then
					set_over(e.id, onto)
				end
			end,
			on_drag_leave = function(payload: any)
				local c = M.carry:peek()
				if c and c.over == onto then
					M.carry:set({ id = c.id, over = nil })
				end
			end,
			on_drop = function(payload: any)
				local e = card_of(payload)
				M.carry:set(nil)
				if not e then
					return
				end
				local v = M.verdict(e, onto, act.can)
				if v and v.ok then
					run(e, v)
				end
			end,
		}
	end

	-- A card's drag (`node`, its props as a drag source of its reference):
	-- the same payload, the board told it's carried, and when it's let go.
	function drop.source(node: { [any]: any }, id: string): { [any]: any }
		local drag: any = node.on_drag
		node.on_drag = function(...: any): any
			local payload: any = nil
			if drag then
				payload = (drag :: (...any) -> any)(...)
			end
			if payload ~= nil then
				M.carry:set({ id = id, over = nil })
			end
			return payload
		end
		node.on_drag_end = function()
			local c = M.carry:peek()
			if c and c.id == id then
				M.carry:set(nil)
			end
		end
		return node
	end

	local function lit(onto: string): boolean
		local v = drop.verdict_of(onto)
		return v ~= nil and v.ok
	end
	local function under(onto: string): boolean
		return drop.over() == onto
	end

	function drop.Lit(onto: string, radius: string): Node
		return ui.show(function(): boolean
			return lit(onto)
		end, function(): Node
			return ui.div {
				class = "orch-drop-lit",
				-- (Drawn only: the pointer passes through to what is under it.)
				pointer_events = "none",
				absolute = true,
				top = 0,
				left = 0,
				right = 0,
				bottom = 0,
				z = 4,
				rounded = radius,
				border = function(): number
					return if under(onto) then 2 else 1
				end,
				border_dash = function(): { number }?
					return if under(onto) then nil else { 4, 3 }
				end,
				border_color = "$zeron.drop.ref.edge",
				bg = function(): string
					return if under(onto) then "$zeron.drop.ref.wash" else "#00000000"
				end,
			}
		end)
	end

	function drop.Badge(onto: string): Node
		return ui.show(function(): boolean
			return under(onto) and drop.verdict_of(onto) ~= nil
		end, function(): Node
			local function v(): Verdict
				return drop.verdict_of(onto) or no("")
			end
			return ui.row {
				id = `orch-drop-badge-{onto}`,
				class = "orch-drop-badge",
				shrink = 0,
				h = 22,
				px = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				rounded = "$radius.full",
				bg = function(): string
					return if v().ok then "$accent" else "$surface.raised"
				end,
				border = function(): number
					return if v().ok then 0 else 1
				end,
				border_color = "$border",
				color = function(): string
					return if v().ok then "$accent.text" else "$text.secondary"
				end,
				Icon {
					name = function(): Icon.Name
						local x = v()
						return if x.ok then x.icon or "check-circle" else "close-circle"
					end,
					size = 12,
				},
				ui.text {
					function(): string
						return locale.s(v().key)
					end,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				},
			}
		end)
	end

	return drop
end

return M
