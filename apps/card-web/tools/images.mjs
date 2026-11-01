// The still picture for each card (twitter:image / og:image): the player at
// the post's size, 2.5 device pixels a CSS pixel, the card at rest, written
// to dist/@u/card.png. `node tools/images.mjs [username ...]`.
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { dist } from "./build.mjs";
import { launch, settle } from "./browser.mjs";
import { startServer } from "./serve.mjs";
import { stripPng } from "./png-strip.mjs";
import { IMAGE, PLAYER } from "../site/pages.mjs";

export async function renderImages(usernames) {
  const { server, url } = await startServer();
  const browser = await launch();
  const scale = IMAGE.width / PLAYER.width;
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: PLAYER.width, height: PLAYER.height, deviceScaleFactor: scale });
    for (const u of usernames) {
      const t0 = performance.now();
      await page.goto(`${url}/@${encodeURIComponent(u)}/player?theme=dark&arrive=0&still=1`, { waitUntil: "load" });
      await settle(page);
      // Stripped to the picture: a generated image carries no metadata.
      const png = stripPng(await page.screenshot({ type: "png", omitBackground: false }));
      await writeFile(join(dist, `@${u}`, "card.png"), png);
      console.log(`@${u}: card.png ${png.length} bytes in ${Math.round(performance.now() - t0)} ms`);
    }
  } finally {
    await browser.close();
    server.close();
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  let names = process.argv.slice(2);
  if (!names.length) {
    const { readFile } = await import("node:fs/promises");
    names = JSON.parse(await readFile(join(dist, "..", "mock", "members.json"), "utf8")).members.map((m) => m.username);
  }
  await renderImages(names);
}
