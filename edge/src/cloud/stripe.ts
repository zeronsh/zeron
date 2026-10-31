/**
 * Cloud billing through Stripe: the plan catalog, Checkout and Billing Portal
 * sessions, and webhook verification + normalization.
 *
 * - Plans are monthly subscriptions. Each paid billing period grants the
 *   plan's credits for that period; they expire when it ends.
 * - Credit packs are one-off payments. Their credits never expire and are
 *   spent after the period's plan credits.
 * - The account a payment belongs to rides Stripe metadata (`orgId`,
 *   `userId`) on the Checkout session, its subscription and its payment
 *   intent, so every webhook routes straight to the user's CloudAccount DO.
 *
 * Configuration (Worker secrets/vars): `STRIPE_SECRET_KEY`,
 * `STRIPE_WEBHOOK_SECRET`, and the Price ids `STRIPE_PRICE_BASE`,
 * `STRIPE_PRICE_PRO`, `STRIPE_PRICE_MAX` (monthly) and `STRIPE_PRICE_CREDITS`
 * (one-off, one pack). Billing reads as unavailable until all are set.
 */
import type { Env } from "../env";

export type StripeEnv = Pick<
  Env,
  | "STRIPE_SECRET_KEY"
  | "STRIPE_WEBHOOK_SECRET"
  | "STRIPE_PRICE_BASE"
  | "STRIPE_PRICE_PRO"
  | "STRIPE_PRICE_MAX"
  | "STRIPE_PRICE_CREDITS"
  | "STRIPE_PORTAL_CONFIGURATION"
  | "STRIPE_API_URL"
>;

export type PlanId = "base" | "pro" | "max";

export interface Plan {
  readonly id: PlanId;
  readonly name: string;
  readonly priceCents: number;
  readonly monthlyCredits: number;
}

/** The monthly plans, cheapest first. A credit is a minute of a small machine. */
export const PLANS: readonly Plan[] = [
  { id: "base", name: "Base", priceCents: 2_000, monthlyCredits: 45_000 },
  { id: "pro", name: "Pro", priceCents: 5_000, monthlyCredits: 112_500 },
  { id: "max", name: "Max", priceCents: 20_000, monthlyCredits: 450_000 }
];

export interface CreditPack {
  readonly id: "credits";
  readonly priceCents: number;
  readonly credits: number;
}

/** Credits that never expire, a little dearer than plan credits. */
export const CREDIT_PACK: CreditPack = { id: "credits", priceCents: 1_000, credits: 12_000 };

/** Most packs one Checkout may buy. */
export const MAX_PACKS = 20;

export const planById = (id: string): Plan | undefined => PLANS.find((plan) => plan.id === id);

const priceEnv: Record<PlanId, keyof StripeEnv> = {
  base: "STRIPE_PRICE_BASE",
  pro: "STRIPE_PRICE_PRO",
  max: "STRIPE_PRICE_MAX"
};

export const planPrice = (env: StripeEnv, plan: PlanId): string | undefined => env[priceEnv[plan]] || undefined;

export const planByPrice = (env: StripeEnv, price: string | undefined): Plan | undefined =>
  price ? PLANS.find((plan) => planPrice(env, plan.id) === price) : undefined;

/** Everything billing needs is configured on this deployment. */
export const billingAvailable = (env: StripeEnv): boolean =>
  Boolean(
    env.STRIPE_SECRET_KEY &&
      env.STRIPE_WEBHOOK_SECRET &&
      env.STRIPE_PRICE_CREDITS &&
      PLANS.every((plan) => planPrice(env, plan.id))
  );

// ── API ─────────────────────────────────────────────────────────────────────

export class StripeError extends Error {
  constructor(
    message: string,
    readonly status: number
  ) {
    super(message);
  }
}

type FormValue = string | number | boolean | undefined | FormObject | FormValue[];
interface FormObject {
  readonly [key: string]: FormValue;
}

/** Stripe's form encoding: nested keys as `a[b][0][c]`. */
export const formEncode = (body: FormObject): string => {
  const pairs: string[] = [];
  const walk = (prefix: string, value: FormValue): void => {
    if (value === undefined) return;
    if (Array.isArray(value)) {
      value.forEach((item, i) => walk(`${prefix}[${i}]`, item));
    } else if (typeof value === "object") {
      for (const [key, item] of Object.entries(value)) walk(prefix ? `${prefix}[${key}]` : key, item);
    } else {
      pairs.push(`${encodeURIComponent(prefix)}=${encodeURIComponent(String(value))}`);
    }
  };
  walk("", body);
  return pairs.join("&");
};

const stripePost = async <T>(env: StripeEnv, path: string, body: FormObject): Promise<T> => {
  if (!env.STRIPE_SECRET_KEY) throw new StripeError("Billing isn't set up on this server.", 503);
  const base = (env.STRIPE_API_URL || "https://api.stripe.com").replace(/\/+$/, "");
  const response = await fetch(`${base}/v1/${path}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
      "content-type": "application/x-www-form-urlencoded"
    },
    body: formEncode(body)
  });
  const text = await response.text();
  if (!response.ok) {
    let message = `Stripe answered ${response.status}`;
    try {
      const error = (JSON.parse(text) as { error?: { message?: string } }).error;
      if (error?.message) message = error.message;
    } catch {
      // keep the status line
    }
    throw new StripeError(message, response.status >= 500 ? 502 : 400);
  }
  return JSON.parse(text) as T;
};

export interface AccountRef {
  readonly orgId: string;
  readonly userId: string;
}

export type CheckoutRequest =
  | { readonly kind: "subscription"; readonly plan: PlanId }
  | { readonly kind: "credits"; readonly packs: number };

/** A Stripe-hosted Checkout page for a plan or for credit packs. */
export const createCheckout = async (
  env: StripeEnv,
  account: AccountRef,
  request: CheckoutRequest,
  returnUrl: string,
  customer?: string
): Promise<string> => {
  const metadata = { orgId: account.orgId, userId: account.userId };
  const common = {
    success_url: `${returnUrl}?result=success`,
    cancel_url: `${returnUrl}?result=canceled`,
    client_reference_id: `${account.orgId}:${account.userId}`,
    ...(customer ? { customer } : {}),
    // The account's coupons also serve an older product: no codes here.
    allow_promotion_codes: false
  };
  if (request.kind === "subscription") {
    const price = planPrice(env, request.plan);
    if (!price) throw new StripeError("Billing isn't set up on this server.", 503);
    const session = await stripePost<{ url: string }>(env, "checkout/sessions", {
      ...common,
      mode: "subscription",
      line_items: [{ price, quantity: 1 }],
      metadata: { ...metadata, plan: request.plan },
      subscription_data: { metadata: { ...metadata, plan: request.plan } }
    });
    return session.url;
  }
  if (!env.STRIPE_PRICE_CREDITS) throw new StripeError("Billing isn't set up on this server.", 503);
  const credits = CREDIT_PACK.credits * request.packs;
  const session = await stripePost<{ url: string }>(env, "checkout/sessions", {
    ...common,
    mode: "payment",
    line_items: [{ price: env.STRIPE_PRICE_CREDITS, quantity: request.packs }],
    metadata: { ...metadata, credits },
    payment_intent_data: { metadata: { ...metadata, credits } },
    // A customer for later purchases and the portal, even on a one-off.
    ...(customer ? {} : { customer_creation: "always" })
  });
  return session.url;
};

/** The Stripe Billing Portal: change plan, payment method, invoices, cancel. */
export const createPortal = async (env: StripeEnv, customer: string, returnUrl: string): Promise<string> => {
  const session = await stripePost<{ url: string }>(env, "billing_portal/sessions", {
    customer,
    return_url: `${returnUrl}?result=portal`,
    // Cloud's own portal: the account's default may serve other products.
    ...(env.STRIPE_PORTAL_CONFIGURATION ? { configuration: env.STRIPE_PORTAL_CONFIGURATION } : {})
  });
  return session.url;
};

// ── webhooks ────────────────────────────────────────────────────────────────

/** How far a webhook's signed timestamp may drift (Stripe's default). */
export const SIGNATURE_TOLERANCE_S = 300;

const hex = (bytes: ArrayBuffer): string =>
  [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");

const timingSafeEqual = (a: string, b: string): boolean => {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
};

/** Sign `payload` the way Stripe does (tests and the verifier share it). */
export const signPayload = async (secret: string, timestamp: number, payload: string): Promise<string> => {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  return hex(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${timestamp}.${payload}`)));
};

/** `Stripe-Signature: t=…,v1=…[,v1=…]` over the raw body, within tolerance. */
export const verifySignature = async (
  secret: string,
  header: string | null,
  payload: string,
  nowS = Math.floor(Date.now() / 1000)
): Promise<boolean> => {
  if (!header) return false;
  let timestamp: number | undefined;
  const signatures: string[] = [];
  for (const part of header.split(",")) {
    const [key, value] = part.trim().split("=", 2);
    if (key === "t" && value) timestamp = Number(value);
    if (key === "v1" && value) signatures.push(value);
  }
  if (!timestamp || !Number.isFinite(timestamp) || signatures.length === 0) return false;
  if (Math.abs(nowS - timestamp) > SIGNATURE_TOLERANCE_S) return false;
  const expected = await signPayload(secret, timestamp, payload);
  return signatures.some((signature) => timingSafeEqual(signature, expected));
};

export interface SubscriptionState {
  readonly id: string;
  readonly plan: PlanId;
  /** Stripe status: `active`, `trialing`, `past_due`, `canceled`, … */
  readonly status: string;
  /** Unix ms the current period ends (renews or lapses). */
  readonly periodEnd: number;
  readonly cancelAtPeriodEnd: boolean;
}

/** A webhook reduced to what the account acts on. */
export type BillingEvent =
  | {
      readonly type: "credits_purchased";
      readonly eventId: string;
      readonly account: AccountRef;
      readonly customer?: string;
      readonly reference: string;
      readonly credits: number;
    }
  | {
      readonly type: "customer";
      readonly eventId: string;
      readonly account: AccountRef;
      readonly customer: string;
    }
  | {
      readonly type: "subscription";
      readonly eventId: string;
      readonly account: AccountRef;
      readonly customer?: string;
      readonly subscription: SubscriptionState;
      /** The event's own time (Unix ms): Stripe doesn't deliver in order. */
      readonly createdAt?: number;
    }
  | {
      readonly type: "period_paid";
      readonly eventId: string;
      readonly account: AccountRef;
      readonly customer?: string;
      readonly invoice: string;
      readonly plan: PlanId;
      /** Unix ms. */
      readonly start: number;
      readonly end: number;
    };

type Json = Record<string, unknown>;
const obj = (value: unknown): Json | undefined =>
  typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : undefined;
const str = (value: unknown): string | undefined => (typeof value === "string" && value ? value : undefined);
const num = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined);
const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;

const accountOf = (metadata: unknown): AccountRef | undefined => {
  const m = obj(metadata);
  const orgId = str(m?.orgId);
  const userId = str(m?.userId);
  return orgId && userId && ID_RE.test(orgId) && ID_RE.test(userId) ? { orgId, userId } : undefined;
};

const idOf = (value: unknown): string | undefined => str(value) ?? str(obj(value)?.id);

const subscriptionState = (env: StripeEnv, sub: Json): SubscriptionState | undefined => {
  const item = obj((obj(sub.items)?.data as unknown[] | undefined)?.[0]);
  const plan = planByPrice(env, idOf(obj(item?.price)) ?? str(obj(item?.price)?.id));
  const id = str(sub.id);
  // Newer API versions moved the period onto the item.
  const periodEnd = num(sub.current_period_end) ?? num(item?.current_period_end);
  if (!id || !plan || periodEnd === undefined) return undefined;
  return {
    id,
    plan: plan.id,
    status: str(sub.status) ?? "active",
    periodEnd: periodEnd * 1000,
    cancelAtPeriodEnd: sub.cancel_at_period_end === true
  };
};

/** Normalize a verified webhook; `undefined` for events billing ignores. */
export const parseEvent = (env: StripeEnv, event: Json): BillingEvent | undefined => {
  const eventId = str(event.id);
  const type = str(event.type);
  const data = obj(obj(event.data)?.object);
  if (!eventId || !type || !data) return undefined;
  switch (type) {
    case "checkout.session.completed":
    case "checkout.session.async_payment_succeeded": {
      const account = accountOf(data.metadata);
      if (!account) return undefined;
      const customer = idOf(data.customer);
      if (data.mode === "payment") {
        if (data.payment_status !== "paid") return undefined;
        const credits = Number(obj(data.metadata)?.credits);
        if (!Number.isFinite(credits) || credits <= 0) return undefined;
        return { type: "credits_purchased", eventId, account, customer, reference: str(data.id) ?? eventId, credits };
      }
      return customer ? { type: "customer", eventId, account, customer } : undefined;
    }
    case "customer.subscription.created":
    case "customer.subscription.updated":
    case "customer.subscription.deleted": {
      const account = accountOf(data.metadata);
      const subscription = subscriptionState(env, data);
      if (!account || !subscription) return undefined;
      return {
        type: "subscription",
        eventId,
        account,
        customer: idOf(data.customer),
        subscription: type === "customer.subscription.deleted" ? { ...subscription, status: "canceled" } : subscription,
        ...(num(event.created) !== undefined ? { createdAt: num(event.created)! * 1000 } : {})
      };
    }
    case "invoice.paid": {
      // The subscription's metadata, wherever this API version puts it.
      const account =
        accountOf(obj(data.subscription_details)?.metadata) ??
        accountOf(obj(obj(data.parent)?.subscription_details)?.metadata);
      const invoice = str(data.id);
      if (!account || !invoice) return undefined;
      // A plan's credits come with the invoice that starts or renews it. A
      // plan change's mid-period invoice grants nothing: switching back and
      // forth must not mint a month of credits each time (Cloud's portal
      // switches plans without proration, from the next period).
      const reason = str(data.billing_reason);
      if (reason !== "subscription_create" && reason !== "subscription_cycle") return undefined;
      const lines = (obj(data.lines)?.data as unknown[] | undefined) ?? [];
      for (const raw of lines) {
        const line = obj(raw);
        if (!line) continue;
        const price =
          idOf(line.price) ??
          str(obj(obj(line.pricing)?.price_details)?.price) ??
          str(obj(obj(line.plan))?.id);
        const plan = planByPrice(env, price);
        // A plan change's invoice credits the unused old plan with a
        // negative line: only what was paid for grants credits.
        if ((num(line.amount) ?? 0) < 0) continue;
        if (line.proration === true || obj(obj(line.parent)?.subscription_item_details)?.proration === true) continue;
        const period = obj(line.period);
        const start = num(period?.start);
        const end = num(period?.end);
        if (!plan || start === undefined || end === undefined || end <= start) continue;
        return {
          type: "period_paid",
          eventId,
          account,
          customer: idOf(data.customer),
          invoice,
          plan: plan.id,
          start: start * 1000,
          end: end * 1000
        };
      }
      return undefined;
    }
    default:
      return undefined;
  }
};
