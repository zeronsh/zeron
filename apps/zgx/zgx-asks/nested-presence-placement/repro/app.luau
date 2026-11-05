-- Slots a plugin replaces (plugins/swap), built five ways: bare; as a
-- `ui.presence` branch's root; as a child inside one; and the same two
-- inside a presence nested in another. Seen on zgx f6506b6e6, a slot that
-- is a presence branch's root is never replaced (nested or not); one a
-- child inside the branch is.
local key = ui.state("key", "a")
local inner = ui.state("inner", "x")

local function slot(id: string): Node
	return ui.div { id = id, w = 120, h = 24, ui.text { "app's own" } }
end
local function presence(state: State<string>, build: () -> Node): Node
	return ui.presence(function(): string
		return state:get()
	end, function(): Node
		return build()
	end)
end

return ui.column {
	gap = 8,
	p = 16,
	slot("slot-bare"),
	presence(key, function()
		return slot("slot-root")
	end),
	presence(key, function()
		return ui.div { slot("slot-child") }
	end),
	presence(key, function()
		return ui.div {
			presence(inner, function()
				return slot("slot-nested-root")
			end),
		}
	end),
	presence(key, function()
		return ui.div {
			presence(inner, function()
				return ui.div { slot("slot-nested-child") }
			end),
		}
	end),
}
