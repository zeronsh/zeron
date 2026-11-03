-- How much of each agent's plan its live account has used, on the device a
-- composer works on, as the host reads it from the engine (src/plan_usage.rs,
-- `ListAgentAccounts`): the composer's plan ring.
--
--   plan_usage.now:get()   { device?, agents = { [harness] = Plan } }, an
--                          agent's entry while its live account reports
--                          usage. A plan: { email?, plan?, windows =
--                          { { label, used, resets? } } } (ds/plan_usage).
--   plan_usage.track(device)  follows a device (nil: this one): read at
--                          once, then asked of the providers, every five
--                          minutes after
--   plan_usage.refresh()   asked again (its card opening; at most every
--                          30 seconds)
--   plan_usage.of(harness, device, name?)  that agent's on that device, as
--                          the composer takes it

local M = {}

export type Window = { label: string, used: number, resets: number? }
export type Plan = { email: string?, plan: string?, windows: { Window } }
export type Now = { device: string?, agents: { [string]: Plan } }

M.now = ui.shared("plan_usage", { agents = {} } :: Now)

-- (A host without the engine's, a fixture's, has neither: the ring stays
-- away.)
function M.track(device: string?)
	pcall(ui.invoke, "plan_usage.track", device or "")
end

function M.refresh()
	pcall(ui.invoke, "plan_usage.refresh", nil)
end

-- Agent `harness`'s plan on `device` (nil: this one), named `name` for its
-- card; nil while it reports none (no live account, or one whose provider
-- says nothing of its limits) or another device's is followed.
function M.of(harness: string?, device: string?, name: string?): any
	local now = M.now:get() :: Now
	if not harness or now.device ~= device then
		return nil
	end
	local p = now.agents[harness]
	if not p or #p.windows == 0 then
		return nil
	end
	return { name = name, email = p.email, plan = p.plan, windows = p.windows }
end

return M
