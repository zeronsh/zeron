import { afterEach, describe, expect, it, vi } from "vitest";
import { billingAvailable, createPortal, formEncode, parseEvent, signPayload, verifySignature, type StripeEnv } from "./stripe";

const env: StripeEnv = {
  STRIPE_SECRET_KEY: "sk",
  STRIPE_WEBHOOK_SECRET: "whsec",
  STRIPE_PRICE_BASE: "price_base",
  STRIPE_PRICE_PRO: "price_pro",
  STRIPE_PRICE_MAX: "price_max",
  STRIPE_PRICE_CREDITS: "price_credits"
};
const account = { orgId: "org1", userId: "u1" };

describe("stripe", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("is available only when every key and price is set", () => {
    expect(billingAvailable(env)).toBe(true);
    expect(billingAvailable({ ...env, STRIPE_PRICE_MAX: undefined })).toBe(false);
    expect(billingAvailable({})).toBe(false);
  });

  it("opens Cloud's own portal when one is configured", async () => {
    const bodies: string[] = [];
    vi.stubGlobal("fetch", async (_url: string, init: RequestInit) => {
      bodies.push(String(init.body));
      return new Response(JSON.stringify({ url: "https://portal" }));
    });
    await createPortal(env, "cus_1", "https://edge/billing/done");
    await createPortal({ ...env, STRIPE_PORTAL_CONFIGURATION: "bpc_cloud" }, "cus_1", "https://edge/billing/done");
    expect(bodies[0]).not.toContain("configuration");
    expect(bodies[1]).toContain("configuration=bpc_cloud");
  });

  it("form-encodes nested keys the way Stripe reads them", () => {
    expect(
      formEncode({ mode: "payment", line_items: [{ price: "p", quantity: 2 }], metadata: { orgId: "o 1" }, skip: undefined })
    ).toBe("mode=payment&line_items%5B0%5D%5Bprice%5D=p&line_items%5B0%5D%5Bquantity%5D=2&metadata%5BorgId%5D=o%201");
  });

  it("verifies signatures within tolerance only", async () => {
    const payload = '{"id":"evt_1"}';
    const t = 1_800_000_000;
    const v1 = await signPayload("whsec", t, payload);
    expect(await verifySignature("whsec", `t=${t},v1=${v1}`, payload, t + 10)).toBe(true);
    expect(await verifySignature("whsec", `t=${t},v1=deadbeef,v1=${v1}`, payload, t)).toBe(true);
    expect(await verifySignature("whsec", `t=${t},v1=${v1}`, payload + " ", t)).toBe(false);
    expect(await verifySignature("other", `t=${t},v1=${v1}`, payload, t)).toBe(false);
    expect(await verifySignature("whsec", `t=${t},v1=${v1}`, payload, t + 301)).toBe(false);
    expect(await verifySignature("whsec", null, payload, t)).toBe(false);
  });

  it("reads invoices of old and new API shapes, skipping credit lines", () => {
    const base = { id: "evt", type: "invoice.paid" };
    const old = parseEvent(env, {
      ...base,
      data: {
        object: {
          id: "in_1",
          billing_reason: "subscription_cycle",
          subscription_details: { metadata: account },
          lines: { data: [{ amount: 2000, price: { id: "price_pro" }, period: { start: 10, end: 20 } }] }
        }
      }
    });
    expect(old).toMatchObject({ type: "period_paid", plan: "pro", start: 10_000, end: 20_000, invoice: "in_1" });
    const upgrade = parseEvent(env, {
      ...base,
      data: {
        object: {
          id: "in_2",
          billing_reason: "subscription_create",
          parent: { subscription_details: { metadata: account } },
          lines: {
            data: [
              { amount: -500, pricing: { price_details: { price: "price_base" } }, period: { start: 15, end: 20 } },
              { amount: 4000, pricing: { price_details: { price: "price_max" } }, period: { start: 15, end: 20 } }
            ]
          }
        }
      }
    });
    expect(upgrade).toMatchObject({ plan: "max", start: 15_000 });
    // A plan change's mid-period invoice, or a proration line, grants nothing.
    const change = (billing_reason: string, line: Record<string, unknown> = {}) =>
      parseEvent(env, {
        ...base,
        data: {
          object: {
            id: "in_4",
            billing_reason,
            parent: { subscription_details: { metadata: account } },
            lines: { data: [{ amount: 4000, price: { id: "price_max" }, period: { start: 15, end: 20 }, ...line }] }
          }
        }
      });
    expect(change("subscription_update")).toBeUndefined();
    expect(change("manual")).toBeUndefined();
    expect(change("subscription_cycle", { proration: true })).toBeUndefined();
    expect(change("subscription_cycle", { parent: { subscription_item_details: { proration: true } } })).toBeUndefined();
    // No account in the metadata: not ours to apply.
    expect(parseEvent(env, { ...base, data: { object: { id: "in_3", lines: { data: [] } } } })).toBeUndefined();
  });

  it("reads subscriptions, purchases, and ignores the rest", () => {
    const sub = parseEvent(env, {
      id: "evt_s",
      type: "customer.subscription.updated",
      data: {
        object: {
          id: "sub_1",
          status: "past_due",
          cancel_at_period_end: true,
          metadata: account,
          items: { data: [{ price: { id: "price_base" }, current_period_end: 99 }] }
        }
      }
    });
    expect(sub).toMatchObject({ subscription: { plan: "base", status: "past_due", periodEnd: 99_000, cancelAtPeriodEnd: true } });
    const bought = parseEvent(env, {
      id: "evt_b",
      type: "checkout.session.completed",
      data: { object: { id: "cs_1", mode: "payment", payment_status: "paid", customer: "cus_1", metadata: { ...account, credits: "12000" } } }
    });
    expect(bought).toMatchObject({ type: "credits_purchased", credits: 12000, reference: "cs_1", customer: "cus_1" });
    expect(parseEvent(env, { id: "evt_x", type: "charge.refunded", data: { object: {} } })).toBeUndefined();
    expect(
      parseEvent(env, {
        id: "evt_y",
        type: "checkout.session.completed",
        data: { object: { mode: "payment", payment_status: "paid", metadata: { orgId: "../x", userId: "u" } } }
      })
    ).toBeUndefined();
  });
});
