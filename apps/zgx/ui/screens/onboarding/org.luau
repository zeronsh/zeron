-- Naming your organization, for someone signed in and in none: one field,
-- the address the name makes on the line under it, and the way on, which
-- makes it (Zeron's `POST /auth/orgs`, you its first member), works in it
-- from now on and goes on to inviting to it. An account makes one of its
-- own; one already in an organization works in that one and passes over
-- this step and inviting (asked at sign-in, and again here in case the
-- answer came late). The gallery's Name your organization is the design. Zeron has no address to check yet, so
-- the line says the address and nothing of whether it's free. Gone wrong,
-- the line under the title says so in the danger's ink and the button
-- offers another go; nothing else moves.

local ds = require("@ds")
local Onboarding = ds.Onboarding
local api = require("@account/api")
local state = require("./state")
local actions = require("./actions")

export type Props = { step: number?, count: number?, on_back: () -> () }

local function slug(s: string): string
	local out = string.gsub(string.lower(s), "[^%w]+", "-")
	return (string.gsub(out, "^%-*(.-)%-*$", "%1"))
end

-- The edge's limit on a name.
local MAX = 80

return function(props: Props): Node
	local name = ui.use_state("")
	local saving = ui.use_state(false)
	local failed = ui.use_state(nil :: string?)

	-- Already in one (the answer at sign-in came late, or never came): work
	-- in it, and on. The field stays hidden until that's known, so an
	-- account in one never sees it.
	-- (Asked once a visit: a hot reload doesn't ask again, the answer kept.)
	local known = {
		get = function(_): boolean
			return state.answered("orgs")
		end,
	}
	-- (Its answer heard only while the step is here: Back while it's asked
	-- leaves it unanswered.)
	local here = true
	ui.on_cleanup(function()
		here = false
	end)
	state.ask_until_answered("orgs", function()
	api.orgs():after(function(orgs: { api.Org })
		if not here then
			return
		end
		local org = actions.existing_org(orgs)
		if org and not saving:peek() and state.step:peek() == "org" then
			actions.work_in(org)
			actions.next()
		else
			state.answer("orgs")
		end
	end, function(err: any)
		if not here then
			return
		end
		if err.message == "signed_out" then
			actions.signed_out()
		else
			state.answer("orgs")
		end
	end)
	end)

	local function trimmed(): string
		return (string.gsub(name:get(), "^%s*(.-)%s*$", "%1"))
	end

	local function create()
		local n = trimmed()
		if saving:peek() or n == "" then
			return
		end
		saving:set(true)
		failed:set(nil)
		api.create_org(n):after(function(made: { organization_id: string, existing: boolean? })
			-- Already made (an account makes one of its own): work in that
			-- one, past inviting, as an account already in one does.
			if made.existing then
				saving:set(false)
				actions.work_in({ id = made.organization_id, organization_id = made.organization_id, name = n, role = "admin" })
				actions.next()
				return
			end
			-- Made: work in it (a refused scope still leaves the account in
			-- it; on as well), then invite to it.
			api.choose_org(made.organization_id):after(function()
				saving:set(false)
				actions.org_made(made.organization_id, n)
			end, function()
				saving:set(false)
				actions.org_made(made.organization_id, n)
			end)
		end, function(err: any)
			saving:set(false)
			if err.message == "signed_out" then
				actions.signed_out()
			else
				failed:set(err.message)
			end
		end)
	end

	return Onboarding.Step {
		step = props.step,
		count = props.count,
		back = Onboarding.Back { on_click = props.on_back },
		continue = {
			label = function()
				return if saving:get()
					then ui.t("onboarding.org.creating"):get()
					elseif failed:get() then ui.t("onboarding.try_again"):get()
					else ui.t("common.continue"):get()
			end,
			disabled = function()
				local n = trimmed()
				return saving:get() or n == "" or #n > MAX
			end,
			on_click = create,
		},
		ui.show(function()
			return known:get()
		end, Onboarding.Form {
			title = ui.t("onboarding.org.title") :: any,
			body = function()
				return if failed:get()
					then ui.t("onboarding.org.failed", { name = trimmed() }):get()
					else ui.t("onboarding.org.body"):get()
			end,
			tone = function()
				return if failed:get() then "$zeron.tone.danger.ink" else "$text.secondary"
			end,
			ui.column {
				w_full = true,
				gap = "$space.2",
				ds.TextField {
					value = name,
					name = ui.t("onboarding.org.field") :: any,
					placeholder = ui.t("onboarding.org.placeholder") :: any,
					size = "lg",
					w_full = true,
					autofocus = true,
				},
				ds.AddressLine {
					prefix = "zeron.sh/",
					value = function()
						return slug(name:get())
					end,
					status = function()
						return ""
					end,
				},
			},
		}),
	}
end
