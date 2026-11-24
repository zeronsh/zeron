/**
 * SSRF validation for remote HTTPS URLs stored in `VaultConnections` (both MCP
 * server URLs and OAuth `tokenEndpoint` URLs fetched server-side during token
 * refresh).
 */

const MAX_URL_LENGTH = 2048;
const CONTROL_CHAR_RE = /[\x00-\x1f\x7f]/;
const DNS_LABEL_RE = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const IPV4_PART_RE = /^(?:0x[0-9a-f]+|0[0-7]*|\d+)$/i;

const BLOCKED_EXACT_HOSTS = new Set([
  "localhost",
  "metadata.google.internal",
  "169.254.169.254",
  "100.100.100.200"
]);

const BLOCKED_SUFFIXES = [".localhost", ".local", ".internal", ".arpa"] as const;

export class UnsafeUrlError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "UnsafeUrlError";
  }
}

export interface SafeHttpsUrlOptions {
  /** Allow non-443 HTTPS ports (default false — tokenEndpoint must use default/443). */
  readonly allowCustomPort?: boolean;
}

const isIpv4Like = (host: string): boolean => {
  const clean = host.replace(/\.+$/, "");
  if (!clean) return false;
  const parts = clean.split(".");
  return parts.length >= 1 && parts.length <= 4 && parts.every((part) => IPV4_PART_RE.test(part));
};

/** Extract the raw host before WHATWG URL normalization, for defense-in-depth
 * against non-canonical decimal/hex/octal IPv4 spellings. */
const extractRawHost = (raw: string): string | undefined => {
  const match = /^https:\/\/([^/?#]+)/i.exec(raw);
  if (!match) return undefined;
  let authority = match[1]!;
  const at = authority.lastIndexOf("@");
  if (at !== -1) authority = authority.slice(at + 1);
  if (authority.startsWith("[")) return authority;
  const colon = authority.indexOf(":");
  return colon === -1 ? authority : authority.slice(0, colon);
};

/**
 * Validate a remote HTTPS URL for safe server-side / remote connection use.
 * Throws `UnsafeUrlError` if the URL is invalid or points at a forbidden host,
 * IP literal, embedded credential, non-HTTPS scheme, or disallowed port.
 */
export const assertSafeHttpsUrl = (raw: string, label: string, options?: SafeHttpsUrlOptions): URL => {
  if (typeof raw !== "string" || raw.length === 0 || raw.length > MAX_URL_LENGTH) {
    throw new UnsafeUrlError(`${label} must be a non-empty URL string`);
  }
  if (raw.trim() !== raw || CONTROL_CHAR_RE.test(raw)) {
    throw new UnsafeUrlError(`${label} must not contain whitespace padding or control characters`);
  }

  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    throw new UnsafeUrlError(`${label} is not a valid URL`);
  }

  if (parsed.protocol !== "https:") {
    throw new UnsafeUrlError(`${label} must use https`);
  }
  if (parsed.username !== "" || parsed.password !== "") {
    throw new UnsafeUrlError(`${label} must not contain a username or password`);
  }
  if (!options?.allowCustomPort && parsed.port !== "" && parsed.port !== "443") {
    throw new UnsafeUrlError(`${label} must use the default HTTPS port (443)`);
  }

  const normalizedHost = parsed.hostname.toLowerCase();
  if (!normalizedHost) {
    throw new UnsafeUrlError(`${label} must include a hostname`);
  }
  if (normalizedHost.startsWith("[") || normalizedHost.includes(":")) {
    throw new UnsafeUrlError(`${label} must not use an IPv6 literal`);
  }

  const rawHost = extractRawHost(raw)?.toLowerCase();
  if (rawHost !== undefined) {
    if (rawHost.startsWith("[") || rawHost.includes(":")) {
      throw new UnsafeUrlError(`${label} must not use an IPv6 literal`);
    }
    if (isIpv4Like(rawHost)) {
      throw new UnsafeUrlError(`${label} must not use an IPv4 literal`);
    }
  }

  // Reject leading dots, consecutive dots, or multiple trailing dots.
  if (normalizedHost.startsWith(".") || normalizedHost.includes("..")) {
    throw new UnsafeUrlError(`${label} has an invalid hostname`);
  }
  const host = normalizedHost.endsWith(".") ? normalizedHost.slice(0, -1) : normalizedHost;
  if (!host || host.length > 253) {
    throw new UnsafeUrlError(`${label} has an invalid hostname`);
  }

  if (BLOCKED_EXACT_HOSTS.has(host)) {
    throw new UnsafeUrlError(`${label} host "${host}" is not allowed`);
  }
  for (const suffix of BLOCKED_SUFFIXES) {
    if (host.endsWith(suffix)) {
      throw new UnsafeUrlError(`${label} host "${host}" is not allowed`);
    }
  }
  if (isIpv4Like(host)) {
    throw new UnsafeUrlError(`${label} must not use an IPv4 literal`);
  }

  const labels = host.split(".");
  if (labels.length < 2) {
    throw new UnsafeUrlError(`${label} must use a fully qualified public hostname`);
  }
  if (!labels.every((part) => DNS_LABEL_RE.test(part))) {
    throw new UnsafeUrlError(`${label} has an invalid hostname`);
  }
  if (/^\d+$/.test(labels[labels.length - 1]!)) {
    throw new UnsafeUrlError(`${label} has an invalid TLD`);
  }

  return parsed;
};
