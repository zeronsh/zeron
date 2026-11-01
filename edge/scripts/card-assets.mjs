// Copies the member card pages' static files into public/card: card.js,
// card-page.js, card.css, the fonts and the scenes' art with its manifest,
// from apps/card-web. Run before a deploy (`npm run deploy` does); public/
// isn't committed.
import { rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { copyAssets } from "../../apps/card-web/tools/build.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const into = join(here, "..", "public", "card");
await rm(join(here, "..", "public"), { recursive: true, force: true });
await copyAssets(into);
console.log(`card assets in ${into}`);
