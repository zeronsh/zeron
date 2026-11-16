-- What a pushed orchestration change does to the scripts reading it
-- (src/orchestration_push_tests.rs): each card read through a selector of its
-- own, as the Board's cards are; `runs` counts each card's reruns, `kept`
-- whether a card's row is the very table it was before the host's last push.

local orchestration = require("@app/account/orchestration")

local IDS = { "it-titlebar", "it-live", "it-new" }
local runs = ui.state("runs", {} :: { [string]: number })
local kept = ui.state("kept", {} :: { [string]: boolean })
local seen: { [string]: any } = {}

for _, id in IDS do
	local card = ui.computed(function(): any
		for _, it in orchestration.now:get().items or {} do
			if it.id == id then
				return it
			end
		end
		return nil
	end)
	ui.effect(function()
		local it = card:get()
		local _ = it
		local r = table.clone(runs:peek())
		r[id] = (r[id] or 0) + 1
		runs:set(r)
	end)
end

-- On every push, whether each card is the table it was on the one before.
ui.effect(function()
	local now = orchestration.now:get()
	local k = {}
	for _, it in now.items or {} do
		k[it.id] = seen[it.id] ~= nil and rawequal(seen[it.id], it)
		seen[it.id] = it
	end
	kept:set(k)
end)

return ui.column {
	ui.text(function(): string
		return `{#(orchestration.now:get().items or {})} cards`
	end),
}
