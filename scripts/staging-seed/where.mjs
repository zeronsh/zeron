// Where a seed may write. An id published is never released, so a wrong
// address would leave the seed's items in that store for good: only a
// loopback edge, or a staging one (https, "staging" a word of its host) named
// twice, by the address and by --host.

const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]"]);

/** Why `edge` isn't one a seed may write to, `host` being what --host said;
 * undefined when it is. */
export const whyNotSeeded = (edge, host) => {
  let url;
  try {
    url = new URL(edge);
  } catch {
    return `${JSON.stringify(edge)} isn't an address`;
  }
  if (url.username || url.password || url.pathname !== "/" || url.search || url.hash) return "the edge's address is its scheme and host alone";
  if (LOOPBACK.has(url.hostname)) return undefined;
  if (url.protocol !== "https:") return "an edge that isn't on this machine is reached over https";
  if (!url.hostname.split(/[.-]/).includes("staging")) return `${url.hostname} isn't a staging edge: a seed never writes to any other`;
  if (host !== url.hostname) return `the edge is ${url.hostname}: say --host ${url.hostname} to write to it`;
  return undefined;
};

/** Why a Zeron's session (`bearer`, its access token) isn't one of the WorkOS
 * the accounts file names, read without sending it anywhere; undefined when
 * it is. */
export const whyNotItsSession = (bearer, clientId) => {
  if (!clientId) return "the accounts file names no workos_client_id";
  let issuer;
  try {
    issuer = String(JSON.parse(Buffer.from(String(bearer).split(".")[1], "base64url").toString()).iss);
  } catch {
    return "its access token can't be read";
  }
  return issuer.split("/").includes(clientId) ? undefined : "it's signed in to another WorkOS than the accounts file's";
};
