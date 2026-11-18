# zeron.sh

The landing page and the plugin store's web pages, as an
[Astro](https://astro.build) site on a Cloudflare Worker
(`@astrojs/cloudflare`). Every page that can be is prerendered and served as
a file (the landing page, privacy, 404); the store's routes say
`prerender = false` and are rendered by the Worker, which reads the store and
the visitor's sign-in from the edge worker through its `EDGE` service
binding. The site holds no secret and never talks to WorkOS: web sign-in is
the edge's (`/auth/web/*`).

```sh
npm install
npm run dev        # http://localhost:4321, with the fake edge (fake-edge/)
npm run check      # types, in .astro and .ts
npm test           # builds ./dist, then every test (Chromium for the browser ones:
                   # `npx playwright install chromium` once)
npm run test:edge  # the same site in front of the real edge worker (tests/edge);
                   # needs `npm ci` in ../../edge and scripts/plugin-store-e2e
npm run deploy             # install, check, build, test, then deploy zeron.sh
npm run deploy:staging     # the same for https://site-staging.zeron.sh
```

`wrangler deploy` on its own never builds: the build writes the deployable
config (`dist/server/wrangler.json`) and points wrangler at it, so deploy
through the scripts above, which hold the gate. Staging is `env.staging` in
`wrangler.jsonc`, chosen at build time with `CLOUDFLARE_ENV=staging`.

## Layout

```
src/
  pages/           index, privacy, 404 (prerendered); robots, sitemaps
    plugins/       the catalog, a plugin's page, its report form, package pictures
    account/       your plugins: versions, review state, deploy keys
    admin/         the review dashboard (admins only; anyone else gets the 404)
    auth/web/      hands sign-in to the edge where its own route doesn't
  middleware.ts    each rendered page's headers and CSP (lib/store/security.ts)
  layouts/         Base (the landing page), Store (the store's pages)
  components/
    ui/            Button, Action, Badge, fields, Select, Tabs, Pagination,
                   Dialog, Toast, Glyph, Mark, agent and platform marks
    site/          Head, Nav, Footer, GitHubStars
    store/         plugin cards and tiles, what a plugin asks for, Install in
                   Zeron, screenshots, file tree, code and diff views
    app/           the session window, recreated in HTML (generated)
    home/          one file per section of the landing page
  lib/store/       the edge's contract (types, edge.ts), the README pipeline
                   (markdown.ts), the app's words for permissions, catalog
                   addresses, review helpers; tested, no DOM
  data/            releases (every download), agents, voices, site links, icons
  lib/downloads    which build a visitor is offered; tested, no DOM
  scripts/         downloads.ts (landing), store.ts (the store's forms)
  styles/          global (the tokens, from the app's dark theme), store
fake-edge/         the edge's store contract and sign-in over fixtures, for
                   dev and tests (never deployed)
tests/             the built pages and routes against the fake edge
                   (harness.ts), the store's pages in Chromium (axe, keyboard)
  edge/            the same against the real edge worker: plugins published
                   as a device publishes them, a real web sign-in, every page
                   and form, and a browser on the site's https origin
public/
  telemetry.js     the page-view counter, served exactly as written
tools/
  recreate/        builds the app's windows from its layout tree, and diffs them
```

### The plugin store's pages

- **Caching.** Public pages read no cookie, so they are the same for
  everyone: the Worker cache keeps each address 30 s and serves it 30 s
  stale while it asks again (at most a minute old). Package pictures are
  kept by shared caches a minute at most (a takedown shows within it).
  Signed-in pages are `private, no-store` and `noindex`.
- **READMEs** are rendered on the server: CommonMark + GFM, raw HTML
  dropped, links and pictures rewritten (pictures only from the package,
  through the site's file route; a remote picture becomes a link), then a
  strict sanitizer, then highlighting by class. A version's README never
  changes, so each is rendered once and kept.
- **Forms** post with the session's CSRF token (the edge checks it with the
  Origin; Astro refuses a cross-site post first), answer with 303 and a
  one-shot message. A destructive one asks in a dialog; without a script,
  its link opens a page that asks the same.
- **CSP**: scripts only from the site's own files (no inline script on these
  pages; `astro.config.mjs` keeps the store's scripts out of the page),
  styles from its files and the exact inline blocks a page carries,
  pictures and fonts from the site, nothing from anyone else.

The site wears the app's design: `src/styles/global.css` copies its colours,
radii and control sizes from `apps/zgx/ui/ds/themes/zeron-dark.json`, and the
fonts are the app's (Geist, Geist Mono). Change a token there first.

## The app on the page

Nothing on the page is a screenshot. The app is recreated in HTML and CSS,
and the recreation is held to the real app by a diff against native
captures.

- **The session window** (`src/components/app/SessionWindow.astro`): a
  session with its panel open, in the app's dark and light themes. Whole, it
  is the hero's tilted plane; with `crop`, one part of its 1320 × 840 stage
  at any size (the pieces inside cards). Being HTML, a piece shown large
  stays as sharp as the window.
- **The themes window** (`ThemesWindow.astro`): the same session with its
  panel closed, in six of the app's themes, with the window's frosted glass
  kept: every surface carries the alpha the app drew, so the desktop behind
  it shows through.
- Both are generated by `tools/recreate` from the app's own layout tree:
  `*.fragment.html`, `*.icons.css`, `*.vars.css`, `themes.themes.json` and
  `zw.css` are outputs. Change the tools, not the files.
- **The machine picker and the chart reply** (`WorkIn.astro`,
  `ChartReply.astro`): HTML written by hand from the numbers of the canvas
  drawing on the `zeron/landing-newchat-hero` branch
  (`public/hero/chrome.js`), which that branch's rig checked against the
  app. They are transcriptions of that code, not diffed again.

### tools/recreate

`reference/` holds what each window was built from: the layout tree of
`apps/zgx` in one state (`tree.json`, `tree.txt`, from the app's inspector)
and a native capture of that state in each theme, with alpha
(`ref-<theme>.png`). `reference/artwork` is the sample projects' icons. The
captures are the Linux build's; the title strip is laid out as macOS draws
it (traffic lights, no caption buttons, ⌘ in the shortcuts) and is left out
of the diff.

Run from a scratch copy of `reference/session` or `reference/themes`, with
the tools beside it:

```sh
# the session window (the defaults: THEMES=dark,light NAME=session)
python3 stage_a.py      # the text and image nodes the capture shows
python3 fit.py          # each text's font, by measuring it in the browser
python3 gen.py          # boxes, icons, text and code colours -> the fragment
python3 check.py dark   # render at 2x and diff against that theme's capture
python3 tune.py         # move each node by what the diff finds; repeat gen + check

# the themes window
export THEMES=zeron-dark,tokyo-night,catppuccin,dracula,gruvbox,github-light NAME=themes GLASS=1
```

What each does: boxes come from the tree; fills, edges, corner radii and
hairlines from the reference's pixels; icons are matched against the app's
own SVG set (`ui/ds/icon_svgs.luau`); fonts are the ones whose measured
width fits the app's box; code is coloured cell by cell from the capture;
avatars are `ui/ds/avatar.luau`'s waves; a project's tile is its
repository's own artwork; the sidebar's branch lines and working rings are
drawn from the tree's rows. Every colour is emitted once a theme
(`--c0`, `--c1`, …), so all themes share one markup. With `GLASS=1` each
surface's fill is solved back from the capture's alpha (source-over,
undone), so the stage composites as the app's window does.

Mean absolute error of 255 per channel, below the title strip, at 2x:
session 0.79 dark, 0.93 light; themes 0.6 to 1.2 across the six (laid on
each theme's own window colour). The transcript's prose is the worst part
(1.8), where the browser and the app rasterize the same outlines
differently.

Needs Python with Pillow, numpy and cairosvg, and a Chromium (`CHROME`).
To retake a reference, run the app with `--inspect` (`inspect.py` sends it
commands) on a display that composites, and save the window's pixels.

### The phone

`src/components/app/PhoneSession.astro` is the iOS app's session screen, a
402 × 874 point stage written by hand (`phone.fragment.html`, `phone.css`)
against the app's own screenshot,
`docs/screenshots/message-captions/ios-dark.png`.
`python3 tools/recreate/phone_check.py --tune` renders it at 3x, diffs it,
and prints how far each element sits from the reference. Mean absolute
error of 255: 2.9 over the screen. The app's text is Geist, as here; its
glyphs are SF Symbols, which a web page can't carry, so the back arrow, the
stack, the dots and the status bar are drawn as plain shapes and differ
from the app's by a little.
