-- Inviting teammates, after naming the organization: emails become chips as
-- they're typed, one that can't be sent to stays and says so, and the way
-- on sends them (Zeron's `POST /auth/orgs/{id}/invitations`; Zeron sends
-- the emails, and they join as members). Optional: Skip for now stands
-- beside the way on until they've gone. The gallery's Invite your
-- teammates is the design, less its invite link (Zeron has none yet).
--
-- Sent, the step lists who got one; an address Zeron wouldn't send to
-- stays in the field with why, and the rest are listed as sent. Gone
-- wrong, the line under the title says so and nothing is sent.

local ds = require("@ds")
local PrivateText = require("@ds/private_text")
local Onboarding = ds.Onboarding
local api = require("@account/api")
local state = require("./state")
local actions = require("./actions")

export type Props = { step: number?, count: number?, on_back: () -> (), on_continue: () -> () }

local function valid(s: string): boolean
	return string.match(s, "^[^@%s]+@[^@%s]+%.[^@%s]+$") ~= nil
end

local function bad(list: { string }): number
	local n = 0
	for _, e in list do
		n += if valid(e) then 0 else 1
	end
	return n
end

-- The edge's limit on one sending.
local MAX = 20

-- Why Zeron wouldn't send to an address: the reasons the line has words
-- for (onboarding.invite.failed_*'s), any other said as couldn't be sent.
local WHY: { [string]: boolean } = { invalid = true, exists = true }

-- What went wrong with the last sending, kept as plain words (state holds
-- no messages) and said where it's drawn: who and why, or Zeron's word.
type Problem = { why: string?, email: string?, more: number?, word: string? }

return function(props: Props): Node
	local emails = ui.use_state({} :: { string })
	-- Who's been sent one, in this run (kept by setup, so a return here
	-- lists them again).
	local sent = state.invited
	local sending = ui.use_state(false)
	local problem = ui.use_state(nil :: Problem?)

	local function any_sent(): boolean
		return #sent:get() > 0
	end

	local function field_problem(): string?
		local list = emails:get()
		local n = bad(list)
		if n > 0 then
			return ui.t("onboarding.invite.bad_addresses", { n = n }):get()
		elseif #list > MAX then
			return ui.t("onboarding.invite.too_many", { max = MAX }):get()
		end
		return nil
	end

	local function send()
		local list = emails:peek()
		if sending:peek() or #list == 0 or bad(list) > 0 or #list > MAX then
			return
		end
		sending:set(true)
		problem:set(nil)
		api.invite(state.last_org:peek(), list):after(function(answer: api.Invited)
			sending:set(false)
			local all = table.clone(sent:peek())
			for _, e in answer.sent do
				table.insert(all, e)
			end
			sent:set(all)
			-- Those it wouldn't send to stay, and the line says why.
			local left: { string } = {}
			for _, f in answer.failed do
				table.insert(left, f.email)
			end
			emails:set(left)
			local first = answer.failed[1]
			if first then
				local why = if WHY[first.error] then first.error else "other"
				-- (While addresses are hidden, the line names none.)
				local email = if PrivateText.hidden:peek() then nil else first.email
				problem:set({ why = why, email = email, more = #answer.failed - 1 })
			end
		end, function(err: any)
			sending:set(false)
			if err.message == "signed_out" then
				actions.signed_out()
			else
				problem:set({ word = err.message })
			end
		end)
	end

	local function sending_now(): boolean
		return #emails:get() > 0
	end

	-- Who got one, a row each: those sent before stay as more are sent.
	local function invited(email: string): Node
		return ds.Settings.Row {
			label = email,
			private_label = true,
			truncate = true,
			lead = ds.Avatar { seed = email, size = 32, label = ui.t("onboarding.invite.invited_teammate") :: any },
			control = ds.Badge { ui.t("onboarding.invite.invited"), tone = "info" },
		}
	end

	return Onboarding.Step {
		step = props.step,
		count = props.count,
		back = Onboarding.Back { on_click = props.on_back },
		aside = ui.show(function()
			return not any_sent()
		end, Onboarding.Quiet { label = ui.t("onboarding.skip_for_now") :: any, on_click = props.on_continue }),
		continue = {
			label = function()
				if sending:get() then
					return ui.t("onboarding.invite.sending"):get()
				end
				local n = #emails:get()
				if n == 0 then
					return if any_sent() then ui.t("common.continue"):get() else ui.t("onboarding.invite.send_invites"):get()
				end
				return ui.t("onboarding.invite.send_n", { n = n }):get()
			end,
			disabled = function()
				if sending:get() then
					return true
				end
				local list = emails:get()
				if #list == 0 then
					return not any_sent()
				end
				return bad(list) > 0 or #list > MAX
			end,
			on_click = function()
				if sending_now() then
					send()
				else
					props.on_continue()
				end
			end,
		},
		Onboarding.Form {
			title = ui.t("onboarding.invite.title") :: any,
			body = function()
				local p = problem:get()
				if p then
					if p.word then
						return ui.t("onboarding.invite.nothing_sent", { says = api.says(p.word) }):get()
					end
					local who = p.email or ui.t("onboarding.invite.one_address"):get()
					local args = { why = p.why or "other", who = who, n = p.more or 0 }
					return if (p.more or 0) == 0
						then ui.t("onboarding.invite.failed_one", args):get()
						else ui.t("onboarding.invite.failed_many", args):get()
				end
				local n = #sent:get()
				if n > 0 then
					return ui.t("onboarding.invite.sent", { n = n }):get()
				end
				return ui.t("onboarding.invite.body"):get()
			end,
			tone = function()
				return if problem:get() then "$zeron.tone.danger.ink" else "$text.secondary"
			end,
			ui.column {
				w_full = true,
				gap = "$space.4",
				ds.TokenField {
					value = emails,
					name = ui.t("onboarding.invite.field") :: any,
					placeholder = "name@company.com",
					valid = valid,
					error = field_problem,
					hint = ui.t("onboarding.invite.hint") :: any,
					size = "lg",
					w_full = true,
					autofocus = true,
				},
				ds.Settings.Each(sent, invited, {
					key = function(email: string): string
						return email
					end,
				}),
			},
		},
	}
end
