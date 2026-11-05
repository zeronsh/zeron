-- Setup for someone who already belongs somewhere: the organizations
-- they're in, the last used picked, a click picking another, and an
-- invitation waiting, answered in its row. The way on says where you'll
-- work ("Continue in Halcyon Labs": the pill as wide as its widest name, so
-- nothing jumps as the pick changes); a double click or Enter on a row goes
-- on at once. A sibling of Name and Invite: the same title in the same
-- place, the same measure, the same quiet surface (rows on hairlines, no
-- panel), everything in the window at its real size. Someone has one
-- organization of their own, so there's no making another here: shown only
-- when there is something to choose, and someone with neither an
-- organization nor an invitation goes straight to naming theirs.

local ds = require("@ds")
local kit = require("@demos/example_kit")

local shown = ui.state("ex_orgs_state", "ready")
-- The invitation: waiting, accepted (its row joins the organizations), or
-- declined (its row goes).
local invite = ui.state("ex_orgs_invite", "waiting")
-- The organization picked (its place in the list): the last used, first.
local pick = ui.state("ex_orgs_pick", 1)

local ORGS = {
	{ id = "halcyon", name = "Halcyon Labs", detail = "Owner · 12 members" },
	{ id = "northwind", name = "Northwind Robotics", detail = "Member · 48 members" },
}
-- Five, for a list longer than the card holds (it scrolls inside it).
local FIVE = {
	ORGS[1],
	ORGS[2],
	{ id = "lumen", name = "Lumen Health", detail = "Member · 230 members" },
	{ id = "orbital", name = "Orbital Freight", detail = "Admin · 31 members" },
	{ id = "kestrel", name = "Kestrel Studio", detail = "Member · 6 members" },
}
-- Twelve, for a long list (the whole list scrolls, as Your agents does).
local TWELVE = table.clone(FIVE)
for _, o in {
	{ id = "fjord", name = "Fjord Analytics", detail = "Member · 74 members" },
	{ id = "ember", name = "Ember Robotics", detail = "Member · 19 members" },
	{ id = "quill", name = "Quill & Ink", detail = "Admin · 4 members" },
	{ id = "harbor", name = "Harbor Labs", detail = "Member · 112 members" },
	{ id = "sable", name = "Sable Systems", detail = "Member · 58 members" },
	{ id = "tandem", name = "Tandem Works", detail = "Owner · 3 members" },
	{ id = "verdant", name = "Verdant Bio", detail = "Member · 41 members" },
} do
	table.insert(TWELVE, o)
end
-- The organizations the state shows: one, five, twelve, or the two.
local function orgs(): { any }
	local s = shown:get()
	return if s == "one" then { ORGS[1] } elseif s == "five" then FIVE elseif s == "twelve" then TWELVE else ORGS
end
local INVITATION = { id = "finch", name = "Finch & Co", from = "Jonas Weber", when = "2 days ago", members = 9 }

-- The list, as the invitation stands (ds.OrgList, setup's own): the
-- organizations (left out with an invitation waiting and nothing else),
-- then the invitation, waiting or accepted. Each row is its entry's: the
-- invitation answered, the rows above it stay as they were.
type Entry = { key: string, org: { name: string, detail: string? }?, at: number }
local function entries(): { Entry }
	local out: { Entry } = {}
	if shown:get() ~= "invited" then
		for i, o in orgs() do
			table.insert(out, { key = o.id, org = o, at = i })
		end
	end
	-- (The invitation only where there is one: Ready, Invitation only.)
	local s = shown:get()
	local state = if s == "ready" or s == "invited" then invite:get() else "none"
	if state == "accepted" then
		table.insert(out, { key = INVITATION.id, org = { name = INVITATION.name, detail = `Member · {INVITATION.members} members` }, at = #out + 1 })
	elseif state == "waiting" then
		table.insert(out, { key = "invitation", at = #out + 1 })
	end
	return out
end
local listed = ui.computed(entries)
local function list(): Node
	return ds.OrgList.Each(listed, function(e: Entry, _i: number, now: State<Entry>): Node
		if not e.org then
			return ds.OrgList.Invitation {
				name = INVITATION.name,
				from = INVITATION.from,
				on_accept = function()
					invite:set("accepted")
				end,
				on_decline = function()
					invite:set("declined")
				end,
			}
		end
		local function at(): number
			return now:get().at
		end
		return ds.OrgList.Org {
			org = e.org,
			selected = function()
				return pick:get() == at()
			end,
			on_select = function()
				pick:set(at())
			end,
			on_open = function() end,
		}
	end, {
		key = function(e: Entry): string
			return e.key
		end,
	})
end

-- What the way on can say: "Continue in" each organization (the accepted
-- invitation's too).
local function choices(): { string }
	local out: { string } = {}
	if shown:get() ~= "invited" then
		for _, o in orgs() do
			table.insert(out, `Continue in {o.name}`)
		end
	end
	local s = shown:get()
	if invite:get() == "accepted" and (s == "ready" or s == "invited") then
		table.insert(out, `Continue in {INVITATION.name}`)
	end
	return out
end

return {
	id = "ex_orgs",
	title = "Choose an organization",
	about = "Shown only when there is something to choose: an organization you're in, or an invitation waiting. A sibling of Name and Invite: the same title, measure and quiet surface; the last used picked and a click picking another, an invitation answered in its row, and Continue in the one picked as the way on (as wide as its widest name, so nothing jumps). Someone has one organization of their own, so there's no making another here; someone with neither never sees this. And what shows while the list loads, and when it fails.",
	demo = function()
		local function view(which: string): Node
			if which == "loading" then
				return ds.OrgList.Loading()
			elseif which == "failed" then
				return ds.Callout {
					tone = "danger",
					title = "Couldn't load your organizations",
					body = "Check your connection, then try again. You're still signed in.",
					action = {
						label = "Try again",
						on_click = function()
							shown:set("ready")
						end,
					},
				}
			end
			-- (The organizations as the state shows them, the invitation as it
			-- stands, each answered in place.)
			return list()
		end
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "one", label = "One" },
				{ value = "five", label = "Five" },
				{ value = "twelve", label = "Twelve" },
				{ value = "invited", label = "Invitation only" },
				{ value = "loading", label = "Loading" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 680,
			h = 500,
			kit.scroll_step {
				back = kit.back {},
				aside = kit.quiet { label = "Sign out" },
				-- "Continue in" the one picked; with only an invitation, it opens
				-- once the invitation is accepted (its organization joins the
				-- list); while the list loads or fails, nothing to go on to.
				continue = {
					label = "Continue",
					choices = function()
						local c = choices()
						return if #c > 0 then c else { "Continue" }
					end,
					choice = function()
						return math.min(pick:get(), math.max(#choices(), 1))
					end,
					max_w = 260,
					disabled = function()
						local s = shown:get()
						return s == "loading" or s == "failed" or (s == "invited" and invite:get() ~= "accepted")
					end,
				},
				-- The organization's first step, as Name's is when there is
				-- none to choose.
				step = 1,
				count = 3,
				-- The words and the list scroll together (the app's step's,
				-- ds.Onboarding.ScrollStep), as Your agents' do.
				form = {
					measure = ds.Onboarding.LIST_MEASURE,
					title = "Choose an organization",
					body = "You can switch any time from the sidebar.",
				},
				ui.presence(function(): string
					local s = shown:get()
					return if s == "loading" or s == "failed" then s else "list"
				end, view, { w_full = true, enter = { opacity = { 0, 1 }, duration = 140 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
			},
		}
	end,
}
