-- A dump's part as its hover card shows it (ds/orch_part_card), from the
-- sample's newest dump: the question routed to Exnomic VPN, answered, and
-- beside it the unsure part of the dump before; for
-- tests/control_receipt.test.luau and the receipt's shots. An Open is noted
-- in `opened`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local PartCard = require("@ds/orch_part_card")
local data = require("@ds/orch_data")
local locale = require("@ds/locale")
local sample = require("@account/orchestration_sample")

local opened = ui.state("opened", "")
-- Either theme, for its pictures.
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }
ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
local chats: { [string]: any } = {}
for _, c in sample.chats do
	chats[c.id] = c
end
local function card(dump: number, route: number, id: string): Node
	local r = sample.now.dumps[dump].routes[route]
	return ui.div {
		id = id,
		PartCard {
			route = function(): any
				return r
			end,
			target = function(): data.Target
				return data.target(sample.now, chats, r.target, function(k: string): string
					return locale.s(`ds.orch.target.{k}`)
				end)
			end,
			on_open = function(t: any)
				opened:set(`{t.kind}:{t.id or ""}`)
			end,
		},
	}
end

return ui.row {
	w_full = true,
	h_full = true,
	-- (The window's glass, as a popup's card sits over it.)
	bg = "$zeron.window",
	p = "$space.6",
	gap = "$space.6",
	items = "start",
	card(1, 3, "card-answered"),
	card(2, 2, "card-unsure"),
}
