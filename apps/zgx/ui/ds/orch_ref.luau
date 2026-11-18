--!strict
-- A reference to something in the Control room (a card, a thread, an
-- orchestrator, a machine, an ask, a Q&A entry, a Comms row), dragged onto
-- Capture's composer: a chip there, sent with the dump (`op.refs`).
--
--   Ref.make(kind, id, label, context?) -> Ref     its short strings trimmed
--   Ref.token(ref)                                  `@card:"Topology lines…"`
--   Ref.of(payload) -> Ref?                         a drag payload's ref
--   Ref.source(function() return ref end)           a drag source's props
--   Ref.Pill(ref)                                   the chip-like preview
--   Ref.ICON[kind]                                  its glyph
--
-- The drag payload is `{ ref = Ref }` (a Board card's also carries its
-- column move: `{ tag, id, ref }`); the drop target decides which it takes.

local Icon = require("@self/icon")
local glass = require("@self/glass")

local M = {}

export type Kind = "card" | "chat" | "orchestrator" | "device" | "ask" | "qa" | "comm"
export type Ref = { kind: string, id: string, label: string, context: { [string]: string }? }

M.KINDS = { card = true, chat = true, orchestrator = true, device = true, ask = true, qa = true, comm = true } :: { [string]: boolean }

M.ICON = {
	card = "checklist",
	chat = "side-chat",
	orchestrator = "users-group-rounded",
	device = "monitor",
	ask = "status-input",
	qa = "lightbulb-minimalistic",
	comm = "chat-round-line",
} :: { [string]: Icon.Name }

-- A context value's longest (characters).
local CONTEXT_MAX = 200
-- A token's label's longest (characters).
local LABEL_MAX = 32

local function squash(s: string): string
	local t = string.gsub(s, "%s+", " ")
	return string.match(t, "^%s*(.-)%s*$") or ""
end

-- `s` cut to `n` characters (UTF-8), "…" in place of the rest.
local function cut(s: string, n: number): string
	local len = utf8.len(s)
	if not len then
		return string.sub(s, 1, n)
	end
	if len <= n then
		return s
	end
	local stop = utf8.offset(s, n) :: number
	local kept = string.match(string.sub(s, 1, stop - 1), "^(.-)%s*$") or ""
	return kept .. "…"
end
M.cut = cut

function M.make(kind: string, id: string, label: string?, context: { [string]: any }?): Ref
	local ctx: { [string]: string } = {}
	local any_ctx = false
	for k, v in context or {} do
		if type(v) == "string" or type(v) == "number" then
			local s = squash(tostring(v))
			if s ~= "" then
				ctx[k] = cut(s, CONTEXT_MAX)
				any_ctx = true
			end
		end
	end
	local name = squash(label or "")
	return { kind = kind, id = id, label = if name ~= "" then name else id, context = if any_ctx then ctx else nil }
end

function M.token(ref: Ref): string
	local label = cut(squash(ref.label or ""), LABEL_MAX)
	if label == "" then
		label = ref.id
	end
	if not string.match(label, "^[%w%-_.]+$") then
		label = `"{string.gsub(label, '"', "'")}"`
	end
	return `@{ref.kind}:{label}`
end

function M.of(payload: any): Ref?
	if type(payload) ~= "table" then
		return nil
	end
	local r = payload.ref
	if type(r) == "table" and type(r.kind) == "string" and M.KINDS[r.kind] and type(r.id) == "string" and r.id ~= "" then
		return r :: Ref
	end
	return nil
end

-- A thread as a reference: its orchestrator (its crew's, else its parent)
-- and its machine. `now`: orchestration.now's; `chat`: its session row.
function M.thread(now: any, chat: any, id: string, title: string?): Ref
	local orchestrator: string? = nil
	for _, m in (now and now.crew) or {} do
		if m.chat_id == id and not m.released then
			orchestrator = m.orchestrator_id
		end
	end
	return M.make("chat", id, title or (chat and chat.title), {
		orchestrator = orchestrator or (chat and chat.parent),
		device = chat and chat.device,
	})
end

-- An orchestrator (`o`, orchestration.now's) as a reference: its goal.
function M.orchestrator(o: any, id: string): Ref
	return M.make("orchestrator", id, o and o.name, { goal = o and o.goal })
end

-- A machine (sessions' device row) as a reference: its platform and
-- hostname.
function M.device(d: any): Ref?
	if not d or type(d.id) ~= "string" then
		return nil
	end
	local name = d.name
	if type(name) ~= "string" or name == "" then
		name = d.hostname
	end
	return M.make("device", d.id, name, { os = d.platform, hostname = d.hostname })
end

-- The chip a drag carries where its source has no preview of its own.
function M.Pill(ref: Ref): Node
	-- (The menus' glass, as a dragged file's or sidebar row's card.)
	local card: { [any]: any } = {
		class = "orch-ref-pill",
		h = 28,
		px = "$space.2",
		gap = "$zeron.space.tight",
		items = "center",
		rounded = "$radius.full",
		ui.div { color = "$text.secondary", shrink = 0, Icon { name = M.ICON[ref.kind] or "tag", size = 13 } },
		ui.text { M.token(ref), literal = true, max_w = 260, truncate = true, text_size = "$font.size.sm", color = "$text.primary" },
	}
	glass.style(card :: any, "menu")
	return ui.row(card :: any)
end

-- A drag source's props: its payload `{ ref = ... }` (nil: no drag), the
-- pill as its preview, hung below the pointer.
function M.source(get: () -> Ref?): { [string]: any }
	return {
		on_drag = function(): any
			local r = get()
			return if r then { ref = r } else nil
		end,
		drag_preview = function(payload: any): Node
			local r = M.of(payload) or get()
			return if r then M.Pill(r) else ui.div {}
		end,
		drag_preview_anchor = { 0, 0.5 },
		drag_preview_offset = { 12, 0 },
	}
end

-- `node` (a table of props for ui.*) made a drag source of `get`'s ref.
function M.on(node: { [any]: any }, get: () -> Ref?): { [any]: any }
	for k, v in M.source(get) do
		node[k] = v
	end
	return node
end

return M
