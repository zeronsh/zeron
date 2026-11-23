/**
 * GitHub Copilot sign-ins (`copilot`): the GitHub token of Copilot's editor
 * app, from GitHub's device flow on the user's laptop, and the login it
 * belongs to. Agents exchange it for short-lived Copilot sessions themselves
 * (Pi reads it as `COPILOT_GITHUB_TOKEN`), so the vault stores it as it came:
 * GitHub OAuth tokens of that app don't expire, they are revoked on GitHub.
 * Grants still expire hourly, so a disconnect reaches machines within the hour.
 */
import { HOUR_MS, isObject, type ParseResult, type ProviderAdapter } from "./types";

export interface CopilotSecret {
  readonly token: string;
  readonly login: string;
}

/** GitHub tokens: `gho_…` / `ghu_…` and older 40-hex ones. */
const TOKEN_RE = /^[A-Za-z0-9_]{20,255}$/;
/** GitHub logins: 1–39 alphanumerics or single inner hyphens. */
export const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}$/;

export const parseCopilotUpload = (material: unknown): ParseResult<CopilotSecret> => {
  if (!isObject(material) || typeof material.copilotToken !== "string" || typeof material.login !== "string") {
    return { ok: false, message: "copilot material must be {copilotToken, login}" };
  }
  const token = material.copilotToken.trim();
  const login = material.login.trim();
  if (!TOKEN_RE.test(token)) return { ok: false, message: "copilot token is malformed" };
  if (!LOGIN_RE.test(login)) return { ok: false, message: "copilot login is not a GitHub login" };
  return { ok: true, secret: { token, login } };
};

export const copilotAdapter: ProviderAdapter<CopilotSecret> = {
  id: "copilot",
  parseUpload: (material) => parseCopilotUpload(material),
  account: (secret) => secret.login,
  identity: async (secret) => `login:${secret.login.toLowerCase()}`,
  refreshable: () => false,
  refreshDue: () => false,
  grant: (secret, now) => ({ accessToken: secret.token, expiresAt: now + HOUR_MS, account: secret.login })
};
