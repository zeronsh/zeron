import { SELF, env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { addCredits, dayKey, ensureMeteringTables, recordSandbox, upsertUsage } from "../../src/cloud/metering";
import { cloudAccountName } from "../../src/cloud/policy";
import { signPayload } from "../../src/cloud/stripe";
import { call, newUser, userBearer, type TestUser } from "./cloud-helpers";

const DAY = 86_400_000;

type Billing = {
  available: boolean;
  plan?: { id: string; name: string; monthlyCredits: number; status: string; periodEnd: number; cancelAtPeriodEnd: boolean };
  credits: { monthly: number; monthlyTotal: number; monthlyEndsAt?: number; extra: number; total: number };
  plans: { id: string; priceCents: number; monthlyCredits: number }[];
  pack: { priceCents: number; credits: number };
  hasCustomer: boolean;
};

const billing = async (u: TestUser) => (await (await call("GET", "/cloud/org1/billing", userBearer(u))).json()) as Billing;

const stripeCalls = async () =>
  ((await (await fetch("https://stripe.test/__calls")).json()) as { calls: { path: string; form: Record<string, string> }[] }).calls;

let eventSeq = 0;
/** POST a Stripe-signed webhook (or a badly signed one). */
const webhook = async (
  type: string,
  object: Record<string, unknown>,
  opts: { id?: string; badSignature?: boolean; created?: number } = {}
) => {
  const payload = JSON.stringify({
    id: opts.id ?? `evt_${++eventSeq}_${crypto.randomUUID().slice(0, 6)}`,
    type,
    created: opts.created ?? Math.floor(Date.now() / 1000),
    data: { object }
  });
  const t = Math.floor(Date.now() / 1000);
  const v1 = await signPayload(opts.badSignature ? "whsec_wrong" : "whsec_test_fake", t, payload);
  return SELF.fetch("https://edge.test/stripe/webhook", {
    method: "POST",
    headers: { "stripe-signature": `t=${t},v1=${v1}`, "content-type": "application/json" },
    body: payload
  });
};

const metadata = (u: TestUser) => ({ orgId: u.orgId, userId: u.userId });

describe("billing", () => {
  it("lists the plans and the credit pack; nothing bought yet", async () => {
    const u = newUser("bill");
    const view = await billing(u);
    expect(view.available).toBe(true);
    expect(view.plan).toBeUndefined();
    expect(view.plans.map((p) => [p.id, p.priceCents, p.monthlyCredits])).toEqual([
      ["base", 2000, 45000],
      ["pro", 5000, 112500],
      ["max", 20000, 450000]
    ]);
    expect(view.pack).toMatchObject({ priceCents: 1000, credits: 12000 });
    expect(view.credits).toMatchObject({ monthly: 0, extra: 0, total: 0 });
    expect(view.hasCustomer).toBe(false);
  });

  it("opens Checkout for a plan and for credit packs, naming the account", async () => {
    const u = newUser("bill");
    const sub = await call("POST", "/cloud/org1/billing/checkout", userBearer(u), { kind: "subscription", plan: "pro" });
    expect(sub.status).toBe(200);
    expect(((await sub.json()) as { url: string }).url).toMatch(/^https:\/\/checkout\.stripe\.test\//);
    const packs = await call("POST", "/cloud/org1/billing/checkout", userBearer(u), { kind: "credits", packs: 2 });
    expect(packs.status).toBe(200);
    const calls = (await stripeCalls()).filter((c) => c.form.client_reference_id === `org1:${u.userId}`);
    expect(calls[0]!.form).toMatchObject({
      mode: "subscription",
      "line_items[0][price]": "price_pro",
      "subscription_data[metadata][userId]": u.userId,
      success_url: "https://edge.test/billing/done?result=success"
    });
    expect(calls[1]!.form).toMatchObject({
      mode: "payment",
      "line_items[0][price]": "price_credits",
      "line_items[0][quantity]": "2",
      "metadata[credits]": "24000",
      "payment_intent_data[metadata][orgId]": "org1"
    });
    for (const bad of [{ kind: "subscription", plan: "gold" }, { kind: "credits", packs: 0 }, { kind: "credits", packs: 1.5 }, {}]) {
      expect((await call("POST", "/cloud/org1/billing/checkout", userBearer(u), bad)).status).toBe(400);
    }
    expect((await call("POST", "/cloud/org1/billing/checkout", `Bearer runner:${u.userId}@org1:cloud-x`, { kind: "credits", packs: 1 })).status).toBe(403);
  });

  it("adds bought credits once, from a signed webhook only", async () => {
    const u = newUser("bill");
    const session = { id: `cs_${crypto.randomUUID()}`, mode: "payment", payment_status: "paid", customer: "cus_1", metadata: { ...metadata(u), credits: "24000" } };
    expect((await webhook("checkout.session.completed", session, { badSignature: true })).status).toBe(400);
    expect((await billing(u)).credits.extra).toBe(0);
    const reply = await webhook("checkout.session.completed", session, { id: "evt_buy_1" });
    expect(reply.status).toBe(200);
    expect((await billing(u)).credits).toMatchObject({ extra: 24000, total: 24000 });
    // The same event again, and the async-success twin of the same session.
    await webhook("checkout.session.completed", session, { id: "evt_buy_1" });
    await webhook("checkout.session.async_payment_succeeded", session);
    const view = await billing(u);
    expect(view.credits.extra).toBe(24000);
    expect(view.hasCustomer).toBe(true);
    // An unpaid (pending) payment grants nothing.
    await webhook("checkout.session.completed", { ...session, id: "cs_pending", payment_status: "unpaid" });
    expect((await billing(u)).credits.extra).toBe(24000);
  });

  it("a plan grants its credits per paid period; they lapse when it ends", async () => {
    const u = newUser("bill");
    const now = Date.now();
    const start = Math.floor((now - 2 * DAY) / 1000);
    const end = Math.floor((now + 28 * DAY) / 1000);
    const subscription = {
      id: "sub_1",
      customer: "cus_2",
      status: "active",
      cancel_at_period_end: false,
      metadata: metadata(u),
      items: { data: [{ price: { id: "price_base" }, current_period_end: end }] }
    };
    await webhook("customer.subscription.created", subscription);
    await webhook("invoice.paid", {
      id: "in_1",
      billing_reason: "subscription_create",
      customer: "cus_2",
      parent: { subscription_details: { metadata: metadata(u) } },
      lines: { data: [{ amount: 2000, pricing: { price_details: { price: "price_base" } }, period: { start, end } }] }
    });
    let view = await billing(u);
    expect(view.plan).toMatchObject({ id: "base", status: "active", periodEnd: end * 1000, cancelAtPeriodEnd: false });
    expect(view.credits).toMatchObject({ monthly: 45000, monthlyTotal: 45000, monthlyEndsAt: end * 1000, extra: 0, total: 45000 });

    // Usage in the period comes out of plan credits first.
    await runInDurableObject(env.CLOUD_ACCOUNTS.get(env.CLOUD_ACCOUNTS.idFromName(cloudAccountName(u.orgId, u.userId))), (_i, state) => {
      ensureMeteringTables(state.storage.sql);
      recordSandbox(state.storage.sql, { provider: "boat", sandboxId: "bx_bill", type: "default", createdAt: now - DAY, chatId: "c" });
      upsertUsage(state.storage.sql, { day: dayKey(now - DAY), provider: "boat", sandboxId: "bx_bill", sandboxType: "default", seconds: 600 * 60, dollars: 0, running: false, reconciledAt: now, closed: true });
      addCredits(state.storage.sql, { at: now, credits: 100, reason: "promo" });
    });
    view = await billing(u);
    expect(view.credits).toMatchObject({ monthly: 44400, extra: 100, total: 44500 });

    // Already on a plan: changes go through the portal, not a second Checkout.
    const again = await call("POST", "/cloud/org1/billing/checkout", userBearer(u), { kind: "subscription", plan: "max" });
    expect(again.status).toBe(409);
    const portal = await call("POST", "/cloud/org1/billing/portal", userBearer(u));
    expect(((await portal.json()) as { url: string }).url).toMatch(/^https:\/\/billing\.stripe\.test\//);

    // Canceled: no plan any more (its period's credits still run to its end).
    await webhook("customer.subscription.deleted", { ...subscription, status: "canceled" });
    view = await billing(u);
    expect(view.plan).toBeUndefined();
    expect(view.credits.monthly).toBe(44400);
  });

  it("a late subscription event never undoes a newer one, nor revives a canceled plan", async () => {
    const u = newUser("bill");
    const end = Math.floor((Date.now() + 28 * DAY) / 1000);
    const t = Math.floor(Date.now() / 1000);
    const subscription = {
      id: "sub_late",
      customer: "cus_late",
      status: "active",
      cancel_at_period_end: false,
      metadata: metadata(u),
      items: { data: [{ price: { id: "price_pro" }, current_period_end: end }] }
    };
    await webhook("customer.subscription.created", subscription, { created: t - 60 });
    await webhook("customer.subscription.deleted", { ...subscription, status: "canceled" }, { created: t });
    // Delivered late: an update from before the cancel.
    await webhook("customer.subscription.updated", { ...subscription, cancel_at_period_end: true }, { created: t - 30 });
    expect((await billing(u)).plan).toBeUndefined();
    // Even at the same second, a canceled subscription stays canceled.
    await webhook("customer.subscription.updated", subscription, { created: t });
    expect((await billing(u)).plan).toBeUndefined();

    // A new plan replaces it; the old one's late events don't erase the new.
    const next = { ...subscription, id: "sub_next", items: { data: [{ price: { id: "price_max" }, current_period_end: end }] } };
    await webhook("customer.subscription.created", next, { created: t + 10 });
    await webhook("customer.subscription.deleted", { ...subscription, status: "canceled" }, { created: t + 20 });
    await webhook("customer.subscription.updated", { ...subscription, status: "active" }, { created: t + 5 });
    expect((await billing(u)).plan).toMatchObject({ id: "max", status: "active" });

    // A replacement's cancel that outruns its own creation: once canceled,
    // that subscription never shows as the plan.
    const third = { ...subscription, id: "sub_third", items: { data: [{ price: { id: "price_base" }, current_period_end: end }] } };
    await webhook("customer.subscription.deleted", { ...third, status: "canceled" }, { created: t + 40 });
    await webhook("customer.subscription.created", third, { created: t + 30 });
    expect((await billing(u)).plan).toMatchObject({ id: "max", status: "active" });
    // Ended plan on record, then a stale event for an older one: still none.
    await webhook("customer.subscription.deleted", { ...next, status: "canceled" }, { created: t + 50 });
    expect((await billing(u)).plan).toBeUndefined();
    await webhook("customer.subscription.updated", { ...subscription, id: "sub_ancient", status: "active" }, { created: t + 1 });
    expect((await billing(u)).plan).toBeUndefined();
  });

  it("the portal needs a Stripe customer first", async () => {
    const u = newUser("bill");
    expect((await call("POST", "/cloud/org1/billing/portal", userBearer(u))).status).toBe(409);
  });

  it("the return page answers without a bearer", async () => {
    const page = await SELF.fetch("https://edge.test/billing/done?result=success");
    expect(page.status).toBe(200);
    expect(await page.text()).toContain("You're all set");
  });
});
