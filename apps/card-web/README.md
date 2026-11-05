# The member card on the web

Zeron's member card (`apps/zgx/ui/ds/member_card.luau`, the native one) as a
web page: a canvas that draws the same card from the same numbers, a share
page whose meta tags make X show it inside a post as a Player Card, and a
still picture of it for everywhere else. The native card is the source of
truth; this folder ports it and measures the port against it.

A member's page is `zeron.sh/@username`, served by the edge worker
(`edge/src/card-routes.ts`). It shows only the public card
(`edge/src/user-directory.ts`'s `PublicCard`: `username`, `firstName`,
`userNumber`, `joinedOn`, `badges`, `scene`, `holo`, `sky`) and embeds
exactly that as its `#card` data block. The first name is the only name
anywhere: on the card, in the title, the tags, the alt text and the picture.
No tagline, no caption, no full name, nothing the edge doesn't return.

## What's here

```
site/
  card.js        the renderer: WebGL2 for the art, sky, paper, pills, foil,
                 border, shadow and tilt (the native shaders' maths, ported
                 to GLSL ES 3.00); Canvas 2D for the words (Geist, as subset
                 WOFF2); springs, tweens, drift and the arrival as the native
  card-page.js   mounts a page's card from its `#card` data block, resolving
                 the art from art/manifest.json by the public `scene`; the
                 player's query parameters for captures (`?pose`, `?lit`, …)
  pages.mjs      the share page (Player Card + Open Graph tags), the player
                 (the iframe), the embed test page and the mock index, as
                 strings; the edge imports it as it is (pages.d.mts types it)
  card.css       the four @font-face rules, the window colours per scheme
  fonts/         Geist SemiBold and Bold, Geist Mono and Mono Medium, subset
                 to Latin and punctuation (tools/fonts.mjs)
art/             the 21 scenes' layered WebP art and manifest.json (from
                 apps/zgx/ui/assets/cards; the edge serves them at /card/art)
mock/members.json      fictional members, each exactly a public card
tools/
  build.mjs      dist/: pages for every mock member, /cards/*.json, the
                 static files under /card, _headers (Cloudflare's format);
                 `copyAssets` is what the edge's deploy uses
  serve.mjs      a local server with the live routes and headers
  images.mjs     the still picture per member (dist/@u/card.png, 1200²,
                 stripped of every PNG chunk but the picture)
  weigh.mjs      bytes, time to first frame, frame times
  behaviour.mjs  hover, touch, Reduce motion, arrival, lost GL context, no
                 WebGL, the embed page at 480 and in a phone column
  capture-web.mjs, compare.mjs, compare-native.mjs
                 the comparison rig against the native card (below)
  fixtures/gallery.json  the gallery's cards, for the comparison only
  fonts.mjs, card-face.mjs (the hash function, with its test vectors),
  png-strip.mjs
```

## Running it locally

Node 22. Once: `cd tools && npm install` (puppeteer-core, subset-font,
pngjs; headless Chrome comes from `/Applications/Google Chrome.app`, or set
`CHROME`).

```sh
node tools/build.mjs                      # dist/ (add --site https://zeron.sh for absolute URLs)
node tools/images.mjs                     # dist/@*/card.png
node tools/serve.mjs 8787                 # http://127.0.0.1:8787/
```

Then open `/` for the members, `/@mara` for a page, `/@mara/player` for the
iframe alone, `/embed-test.html` for the player inside a post-shaped frame
(480 wide, or a phone column), `/cards/mara` for the JSON.

`node tools/weigh.mjs mara [--mobile]` prints what a cold load fetches and
the frame times; `node tools/behaviour.mjs` runs the behaviour checks.

### The player's query parameters

For captures and tests only (`card-page.js`): `theme=dark|light`,
`scale=1`, `origin=x,y` (CSS px), `frame=WxH`, `pose=x,y|rest` (the pointer
on the card, 0 to 1), `lit=1`, `strength=`, `drift=a,b,c|ms`, `tilt=0`,
`arrive=0`, `still=1`, `caption=1`, `dpr=`. `window.__card` is the
controller, `window.__cardReady` resolves when the first frame has drawn.

## On the edge

`edge/src/card-routes.ts` renders `/@:username` (`renderShare`) and
`/@:username/player` (`renderPlayer`) from the request's own origin, behind
`card-gate.ts`'s per-IP limit; `/@:username/card.png` is `card-image.ts`:
the player page drawn by Browser Run at 480×480 and 2.5×, the card at rest,
stripped to its picture and kept in R2 (`CARD_IMAGES`) under a key that
hashes the public card, so a changed card is drawn again and an unchanged
one never twice. Without the browser binding, or when it can't draw, a
picture not yet in R2 answers 404.

The script, style, fonts and art are the worker's static assets at
`/card/*`: `npm -C edge run card-assets` copies them into `edge/public`
(not committed; `npm run deploy` and the deploy workflow run it first).
The pages' CSP allows only the site's own script, style, fonts and images
(`PAGE_CSP`); the player's adds `frame-ancestors` for x.com and twitter.com
(`PLAYER_CSP`), and neither sends `X-Frame-Options`.

Tests: `edge/test/unit/card-web.test.ts` (the pages: public fields only,
first name only, tags, escaping, PNG stripping) and
`edge/test/workerd/card-pages.workerd.test.ts` (the routes, headers, data
blocks, the picture from R2, and a scan of every output for what the
public card doesn't carry).

## Comparing with the native card

The acceptance test is pixel difference, not a look. The rig captures the
gallery's Member card page (`./target/release/zeron-zgx gallery --inspect`
from `apps/zgx`, `zgx inspect snapshot` of the page pane) and the same card
on the web at the same size (300×420 points at 2×), pose and foil position
(the fixtures in `tools/fixtures/gallery.json`, served at
`/_gallery/<username>/player` by `tools/serve.mjs`), and diffs them:
`tools/compare-native.mjs` reads each layer's drift from the inspector's
tree (printed to whole points, read before and after the snapshot), refines
it within ±2 points, finds the card's sub-pixel origin from the words, and
reports mean absolute error, the share of pixels off by more than 8 and 32
of 255, and PSNR, for the card, the card with its caption, the art and the
words, with side-by-side pictures (native, web, difference ×4).

What the rig found (Oct 30, 2026, five cards, dark and light, at rest and
with the pointer on the card): at rest the card differs from the native by
a mean of 0.9–1.8 of 255 per channel, 1–6% of pixels differ by more than 8
and under 1% by more than 32 (PSNR 29.5–33.7 dB), mostly glyph edges (the
native rasterizes Geist with hinting through swash, the browser through
CoreText) and the cut-outs' edges where their drift sits between two
device pixels; with the pointer on the card (foil lit, parallax shifted)
the same, apart from the secret finish's hatch lines, which are a point wide
and sit differently on the pixel grid (13% of pixels off by more than 8,
still under 1% by more than 32). The tilt itself (the projective map about
the card's centre, perspective 900) is ported from zgx's
`LocalTransform::projective` term for term but is not pixel-verified: a
pointer injected through `zgx inspect` lights the card and shifts its
layers, but the gallery's card never turned in any capture, so there is no
native picture of the tilt to diff against.

## What the port keeps from the native card

The layout in points (a 1-point border, 16 of padding, pills 22 high,
badges 20 and 4 apart, Geist Mono at 0.6 em with letter spacing after every
glyph), the sky through the intro's characters (cells 9×12 points, picked by
brightness, the grid starting where the card's `sky` says), the paper's
seven stops, the five foil finishes and the glare, the shadow (sigma 24, 24
down), the clip at the padding box with the corner radius less the border,
the springs (stiffness 220, damping 28 for the tilt; 160 and 22 for the
parallax; semi-implicit Euler capped at 1/30 s), the strength tween (140 ms,
outQuad), each layer's drift (±5 points a depth over 16000/depth ms plus the
number's share, alternating, inOutSine), and the arrival (36 points up over
820 ms, the foil swept once).

What it cannot keep: the text rasterizer. The browser sets the type.
