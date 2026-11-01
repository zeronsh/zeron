// Per-run account rotation: send these Pi providers to Zeron's account
// proxy route for this chat. Only the base URL changes; the catalog, the
// login and the API implementation stay Pi's own.
export default function (pi) {
  const routes = JSON.parse(process.env.ZERON_PI_PROXY || "{}");
  for (const [provider, baseUrl] of Object.entries(routes)) {
    pi.registerProvider(provider, { baseUrl });
  }
}
