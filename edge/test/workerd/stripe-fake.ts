/**
 * A fake Stripe API for the workerd tier (Miniflare outbound, host
 * `stripe.test`): Checkout and Billing Portal sessions answer a URL and the
 * decoded form body is recorded.
 *
 *   GET https://stripe.test/__calls   every call: {path, form, authorization}
 */
import { Response, type Request } from "miniflare";

interface Call {
  path: string;
  form: Record<string, string>;
  authorization: string;
}

export const createStripeFake = () => {
  const calls: Call[] = [];
  let next = 0;
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    if (url.pathname === "/__calls") return reply(200, { calls });
    const form = Object.fromEntries(new URLSearchParams(await request.text()));
    calls.push({ path: url.pathname, form, authorization: request.headers.get("authorization") ?? "" });
    next += 1;
    if (url.pathname === "/v1/checkout/sessions") {
      return reply(200, { id: `cs_test_${next}`, url: `https://checkout.stripe.test/c/${next}` });
    }
    if (url.pathname === "/v1/billing_portal/sessions") {
      return reply(200, { id: `bps_test_${next}`, url: `https://billing.stripe.test/p/${next}` });
    }
    return reply(404, { error: { message: `fake Stripe has no ${url.pathname}` } });
  };
};
