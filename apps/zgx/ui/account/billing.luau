-- Billing, as the host reads it from the engine (src/cloud.rs): the
-- credits that run Cloud chats, the plan, and the packs of extra credits.
-- Buying and changing a plan happen on Stripe's own pages in the browser;
-- the host opens them.
--
--   billing.status()           a promise of { available, plan?, credits,
--                              plans, pack, has_customer }
--   billing.subscribe(plan)    Stripe's page for that plan (its id)
--   billing.buy(packs)         Stripe's page for that many packs
--   billing.manage()           Stripe's page for the plan you're on
--
-- `credits`: { monthly, monthly_total, monthly_ends?, extra, total };
-- `plan`: a plan's { id, name, price_cents, monthly_credits } and its
-- { status, period_end, cancel_at_period_end }; `pack`: { price_cents,
-- credits }. Times are Unix seconds. Every function here is a field, so a
-- fixture answers in the host's place.

local M = {}

export type Plan = { id: string, name: string, price_cents: number, monthly_credits: number }
export type Current = Plan & { status: string, period_end: number?, cancel_at_period_end: boolean }
export type Credits = { monthly: number, monthly_total: number, monthly_ends: number?, extra: number, total: number }
export type Status = {
	available: boolean,
	plan: Current?,
	credits: Credits,
	plans: { Plan },
	pack: { price_cents: number, credits: number },
	has_customer: boolean,
}

-- Most packs one purchase buys (the edge's cap).
M.MAX_PACKS = 20

function M.status(): any
	return ui.call("billing.status", nil)
end

function M.subscribe(plan: string): any
	return ui.call("billing.checkout", { kind = "subscription", plan = plan })
end

function M.buy(packs: number): any
	return ui.call("billing.checkout", { kind = "credits", packs = packs })
end

function M.manage(): any
	return ui.call("billing.portal", nil)
end

return M
