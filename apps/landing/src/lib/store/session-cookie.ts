// The cookie the edge keeps a web session in (edge/src/web-cookies.ts), and
// whether a request carries one: signed in once, maybe no longer. Read by the
// Worker's front door (src/worker.ts) before anything else, so it imports
// nothing.

export const SESSION_COOKIE = "__Host-zeron_session";

const CARRIES = new RegExp(`(?:^|;\\s*)${SESSION_COOKIE}=[^;]`);

/** Whether the request carries a session cookie with a value (the edge alone says whose it is, if anyone's). */
export const hasSessionCookie = (request: Request): boolean => CARRIES.test(request.headers.get("cookie") ?? "");
