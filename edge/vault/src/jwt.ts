/**
 * Unverified JWT payload reads. The vault never trusts these claims for
 * authorization — it only reads them off tokens it already holds, to label an
 * account (`email`) and to schedule refreshes (`exp`). The provider verifies
 * its own tokens when they are used.
 */
import { fromBase64Url, fromUtf8 } from "./encoding";

export const decodeJwtPayload = (token: string): Record<string, unknown> | undefined => {
  const parts = token.split(".");
  if (parts.length !== 3 || !parts[1]) return undefined;
  try {
    const payload: unknown = JSON.parse(fromUtf8(fromBase64Url(parts[1])));
    return payload && typeof payload === "object" && !Array.isArray(payload)
      ? (payload as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
};

/** `exp` claim in Unix ms, or undefined when absent / not a JWT. */
export const jwtExpMs = (token: string): number | undefined => {
  const exp = decodeJwtPayload(token)?.exp;
  return typeof exp === "number" && Number.isFinite(exp) && exp > 0 ? exp * 1000 : undefined;
};
