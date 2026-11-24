// The built site, with the fake edge beside it, served as the tests serve
// it (tests/harness.ts): the production build, its CSP exactly as it ships.
// `npm run preview:fake` builds first. `astro dev` can't show the site
// styled: it injects styles as inline tags the site's CSP doesn't list.
// HOST and PORT choose the address (127.0.0.1:4321); nothing else listens.
import { startSite } from "../tests/harness.ts";

const host = process.env.HOST ?? "127.0.0.1";
const port = Number(process.env.PORT ?? 4321);
const site = await startSite({ host, port });
console.log(`The built site, with the fake store: ${site.url.href}`);
console.log("Sign in at /auth/web/login: @ada publishes, @grace is staff. Its state is in memory: a restart begins it again.");
const stop = async () => {
  await site.dispose();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
