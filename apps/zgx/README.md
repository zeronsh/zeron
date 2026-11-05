# Zeron in zgx

Zeron's interface rebuilt on [zgx](https://zgx.zeron.sh), starting with
onboarding. The interface is Luau (`ui/`): it hot-reloads as it is saved,
keeping its state, so most work never rebuilds Rust. The Rust host
(`src/main.rs`) makes windows, registers fonts and answers the scripts'
actions, and speaks to a Zeron engine (`src/engine/`), which owns the
account, this Mac's agents and GitHub for Cloud.

## Running it

**Dev builds talk to Zeron's staging servers, not production.** Signing in
uses staging's edge (`https://staging.zeron.sh`) and sign-in client, and
the engine zgx starts keeps its data in `~/.zeron-staging`: your production
account, chats and devices aren't there. `ZERON_ZGX_ENV=production` points a
build at production instead (`src/auth.rs` lists the other overrides).

You need Rust 1.85 or newer (edition 2024). We develop on macOS; Linux
should work but is untested. The app is this folder: `apps/zgx` in
zeronsh/zeron, or the root of zeronsh/zeron-zgx, which is a copy of it.

On macOS you also need Zig 0.16 (`brew install zig@0.16`): Ghostty draws the
panel's terminals there (zgx-desktop's `ghostty` feature), and zgx builds it
from source. The first build of it takes about six minutes, once; it's then
cached in `~/Library/Caches/zgx-ghostty` for every checkout. Without Zig 0.16
the build stops and says so (`ZIG` points it at another Zig executable).

```sh
git clone git@github.com:zeronsh/zeron-zgx.git && cd zeron-zgx
cargo run --release -- gallery      # the workbench: every component and demo
```

The first build fetches zgx from GitHub (its `main`, pinned in `Cargo.lock`)
and takes a few minutes. After that, edits to `ui/` hot-reload in the running
window without rebuilding. Pages worth a look, in the gallery's sidebar:

- **Shell → App shell:** the sidebar (grouped, sorted, your own sections,
  drag and drop, a virtualized list), its rail, the title bar, the composer
  travelling between a new chat and a session, routed with `ui.router`. Its
  "Many" switch gives the sidebar 1,000 sessions.
- **Conversation → Transcript:** a long conversation, virtualized, with replies
  streaming in as incremental markdown.
- **Code → Files, Code editor, Syntax highlighting:** a file tree and viewer,
  an editor on zgx's line-laid-out textarea, and the highlighter behind both.
- **Commands → Command palette, Context menu.**

zgx collects the scripts' garbage itself as the window goes idle
(`zgx::idle`). ⌘[ and ⌘] step back and forward through the pages visited.

All the ways in:

```sh
cargo run --release                 # the intro over the desktop, decoding into sign-in
cargo run --release -- intro        # the same
cargo run --release -- onboarding   # setup alone, in its window, from sign-in
cargo run --release -- main         # the main window: sessions, chats, files
cargo run --release -- gallery      # the design system, dark and light side by side
ZERON_ZGX_STEP=agents cargo run --release -- onboarding
cargo run --release -- gallery --inspect   # devtools for `zgx inspect` / its MCP
```

A launch signed in goes straight to setup, and once setup has been finished
on this Mac (its "Open Zeron"), to the main window; finishing setup opens it
too. Signed out, or new, it's the intro and setup again.

### The engine

Sign-in, Your agents and Connect GitHub are the engine's, over its local
IPC: the engine keeps the session (zgx keeps none of its own), finds the
agents and runs their sign-ins, and runs GitHub's device flow. zgx uses
the staging engine, port 27871 with its data in `~/.zeron-staging`
(`scripts/staging-app.sh`'s): one already running is used as it is,
otherwise zgx starts `target/debug/zeron-engine` from this checkout's root
(`cargo build -p zeron --bin zeron-engine` there first), detached, against
zgx's edge and sign-in client. Signing in restarts it once, to sync. Never
the production engine: port 27654 and `~/.comet-native` are refused.

```sh
ZERON_ZGX_ENGINE_PORT=27881 ZERON_ZGX_ENGINE_DATA=/tmp/zgx-engine \
ZERON_ZGX_ENGINE_HOME=/tmp/zgx-home \
  cargo run --release -- onboarding     # an engine of its own, throwaway
```

`ZERON_ZGX_ENGINE_HOME` gives the engine zgx starts a home of its own
(`HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so the agents' logins it reads
and writes are that folder's, not this Mac's. `ZERON_ZGX_ENGINE_BIN` picks
another engine; `ZERON_ZGX_ENGINE_SPAWN=0` only uses one that's running.
`ZERON_ZGX_NO_BROWSER=1` opens no browser for a sign-in (a path instead
writes the page there, for a test rig). See `src/engine/manager.rs`.
`ZERON_ZGX_MOCK=1` offers the engine's mock agent in the model picker
(src/chat.rs), to run a chat without a paid one; `ZERON_MOCK_DELAY_MS`
and `ZERON_MOCK_REPEAT` on the engine it starts pace and lengthen its replies.

This is a cargo workspace of its own (in zeronsh/zeron, not a member of the
root one). zgx
builds against patched crates (winit, wgpu-hal, accesskit), and a git
dependency does not carry a patch, so `Cargo.toml` repeats zgx's. Patching
those in the root workspace would also change gpui's wgpu renderer. It
tracks zgx's `main` branch: `cargo update -p zgx` moves `Cargo.lock` to the
latest commit.

## Layout

```
ui/
  .luaurc            strict mode; aliases @ds, @screens
  app.luau           the main window: lint on, ds.setup(), the screen
  gallery.luau       every ds component in both themes: work on it here first
  intro.luau         the onboarding intro (zgx's agents_intro): art, outside the ds;
                     its exit decodes setup's first screen into the real window
  ds/                the design system: the only place values live
    init.luau        what screens import: Text, Button, Surface, Mark, Steps, setup
    tokens.luau      app tokens (zeron.*), declared in standard tokens; loads themes
    themes/          zeron-dark.json, zeron-light.json (restyle in place as saved)
    text.luau        type styles × tones (ui.variants)
    button.luau      primary / secondary / ghost / accent × sm / md / lg
    surface.luau     plain / raised / sunken / overlay; interactive; selected
    mark.luau        a harness mark on its tile
    steps.luau       progress dots
    decode.luau      a screen decoding out of characters (layer shader): transitions
    onboarding_frame.luau  setup's window and its steps (ds.Onboarding: Frame,
                     Step and its footer pills, Form), the app's and the
                     gallery's examples' alike
    card_wall.luau   columns of member cards drifting beside a screen's words
  screens/
    onboarding/      init.luau (the steps in ds.Onboarding.Frame), state.luau,
                     actions.luau, one module per step: signin first
    shell/           the main window: init.luau (the frame), route.luau (where
                     it is: ui.router, the project, session and device open),
                     sidebar.luau (ds.Sidebar on @account/sessions), page.luau
                     (the routed page, the composer, the files pane),
                     title_bar.luau, settings.luau (its pages: `settings.add`),
                     appearance.luau; ui/shell_fixture.luau runs it headless
files/           the session page's files (Pane { project, session }):
                     workspace.luau (a folder read as it opens, the file open,
                     saves, what changes on disk), api.luau (src/files.rs's
                     calls; ui/files_fixture.luau stubs them)
    settings/        Settings' pages (init.luau adds them to the shell's list):
                     General, Appearance, Notifications, Files, Profile (your
                     account too: `settings/account` lands there), Providers,
                     Devices, Members, About & updates (the engine's
                     updater, src/about.rs); wallpaper.luau (a new
                     chat's own picture, kept by the host); search.luau (⌘F, each page indexes
                     its rows), store.luau (the account and organization),
                     prefs.luau (this Mac's preferences, for other screens),
                     chimes.luau; host.luau's calls, src/settings.rs's
                     (ui/settings_stub.luau answers them headless)
  assets/            art, icons (paths resolve from ui/)
fonts/               Instrument Serif (Geist comes from apps/ios/Zeron/Fonts)
```

## The design system's rules

zgx's design guide has three layers, and this follows them:

1. **Values live in the theme.** Colours, type, radii and spacing are
   tokens: the standard ones (`$text.secondary`, `$space.3`, `$radius.lg`) set
   by `themes/*.json`, and Zeron's own under `zeron.*`, declared in
   `tokens.luau` in terms of standard ones so any theme styles them. To try a
   palette, edit the JSON; every window restyles without reloading scripts.
2. **Components turn tokens into styles** with `ui.variants` (shadcn's
   `cva`): one recipe per component, its axes typed. Axes chosen by a state or
   function (`selected = function() … end`) restyle in Rust without calling
   the script.
3. **Spacing is a scale.** Every padding, margin and gap is a step of
   `space.1`–`space.8` (4, 8, 12, 16, 24, 32, 48, 64), written out in both
   theme files, or one of two half-steps for controls: `zeron.space.hair` (2)
   and `zeron.space.snug` (10). Fixed geometry, like a title bar's height or
   where a control sits beside the traffic lights, is a named constant, not a
   spacing token.
4. **The lint keeps screens honest.** Entries call `ui.theme.strict("warn")`
   first. A hard-coded colour, spacing, radius, type size or shadow in a
   screen warns and names the token to use. `ui.raw(value, "why")` is the
   greppable escape hatch. `ZGX_DESIGN_LINT=error` makes the lint fail.
5. **Corners are concentric. No exceptions.** Anything inside a rounded
   container, its hover wash and selection included, takes the container's
   radius less the inset between them, so the two curves run parallel: a 16
   corner with 4 of inset holds a 12 corner, and a pill's contents are pills.
   Never a fixed radius inside a rounded parent. This holds for what follows
   the container's edge (washes, rows, thumbs, controls in a bar), and there
   an inset at or past the radius means square. A button standing well inside
   a card or dialog keeps its own corner. Work it out from the tokens, so it
   holds when a radius changes, and check every state, not only the one at
   rest. `control.nest` and `control.contain` do this.
6. **A container's inset is even.** A bar, pill, group or card holding
   controls leaves the same room on all four sides of them, measured from the
   controls' boxes, so the top and bottom never read tighter than the sides.
   It's one value, not `px` and `py`. Words inside a button are not this:
   a button's own label keeps its wider side padding.
7. **Data changes in place; never rebuild for it.** The interface is built
   once and follows its states, so new data reaches only what shows it.
   A list is a keyed `ui.each` (`(item, i, row)` when its items change),
   its rows staying their items' as others come and go. A value that
   changes is read through a function or a state bound to the prop that
   shows it. `ui.presence` and `ui.switch` are for real swaps (loading,
   empty, failed, found; one page to another), keyed by which kind shows,
   never by the data: a key built from data (`table.concat` of ids, a
   count, a name) makes the whole part again for every change, losing its
   focus, its scroll and its place in the flow. Where zgx fixes a prop
   when a node is made (a textarea's `highlight`, a popover's `placement`),
   key by that alone and say why. `zgx lint` doesn't catch this; the
   tests in `ui/tests/*_in_place.test.luau` do, by node identity.

Screens import only from `@ds` plus zgx's layout primitives
(`ui.column`, `ui.row`, `ui.presence`…). Anything a second screen would want
moves into `ds/`, and into the gallery, before it is used.

The settled glass values from the shipping app carry over. There is one
wash for hover, pressed and selected; selection adds only a ring, never a
drop shadow. The dark window is a #080808 scrim at 90% over the under-window
blur. Light is provisional.

## Patterns

What every chat's code follows, scripts and host alike (from the Nov 1
audit; each line names the mistake it keeps out):

- **Props take functions.** A ds prop that can change accepts a value, a
  state or a function (`control.Live<T>`) and reads it through
  `control.value(p)` inside the binding that shows it, so it follows. Pages
  pass a function (`disabled = function() return not admin:get() end`),
  never a `:peek()` taken once, and never wrap a component in a
  `presence` just to refresh a prop. A ds prop that only takes a value is a
  bug to fix in ds, not to work around in the page.
- **Lists from Rust change per entry.** The host publishes what changed: one
  entry (`set_key` on its key or index), not the whole list again, and
  builds Luau values only for that. Streaming a reply touches its one part.
  A whole-list republish costs every row's bindings on every change, and a
  long list of large values fragments the heap.
- **No file, network or process work on the UI thread.** Reading or
  hashing files, spawning a process and waiting on it, and HTTP all go on a
  thread. The answer comes back through an async action's future or
  `zgx::task::latest`.
- **No polling.** Wait on the event: `poll()` on a socket and a wake pipe
  (`engine/rpc.rs`), kqueue (`plugins/file_events.rs`), an engine watch,
  `zgx::task::latest`. A timer is for a deadline someone sees (the next
  stale state, a toast), set for that moment, not a loop checking.
- **Timers belong to what makes them.** `ui.after`/`ui.every` made in an
  effect run are cancelled when it runs again (zgx's "What owns what").
  Start them in an effect or a handler, never in a computed, and never rely
  on one surviving a rerun to clear a flag.
- **A reload changes nothing.** Building must not call the engine or start
  motion except behind keyed state (`visits.once`, `ui.state` flags), since
  a hot reload builds everything again (`tests/reload.rs`).

## Experimenting

- **A component:** run the gallery, edit `ds/*.luau` or a theme file, and
  save. Both themes update side by side.
- **A step:** `ZERON_ZGX_STEP=<step> … onboarding`. The step is kept across
  saves, so edit its module and watch it change in place.
- **A variant of a component:** add an option to its recipe's axis, try it in
  the gallery, then keep it or delete it.
- **Agents:** `--inspect`, then `zgx inspect tree`, `zgx inspect snapshot`,
  `zgx inspect click '"Continue"'`, or `claude mcp add zgx -- zgx inspect mcp`.
  Install the CLI with `cargo install --git https://github.com/zeronsh/zgx
  zgx-script --bin zgx --features inspect`.
- **Checks:** `zgx lint ui` (performance rules) and `zgx check ui` (types, needs
  luau-lsp).

## The intro's hand-off

Taking "Get started" opens setup's window (680 by 500, fixed) under the
scene, at a rect the intro picks (`prepare`, staged while only the drift
moves). The scene is opaque there, so the window's first frame and macOS's
open animation stay hidden. The intro draws setup's first screen, sign-in,
over that rect: the same `@screens/onboarding` module in the same theme.
The moment is built round sign-in's member card:

- A still picture of the scene, taken at the click, closes onto the
  window's rounded rect through characters from the screen's edges inward
  (`decode.collapse`): the sunrise's light ends at the window's edge.
- Sign-in decodes out of the same characters from its card outward
  (`decode.shader`, opened off-centre, `decode.full`): the card's sky is
  drawn in them, so the sea's glyphs settle into the card first.
- "Your agents, anywhere.", "Get started" (ds.Onboarding.WayIn, the
  forward pill in glass) and the agents are in that picture too (it is the
  window as last shown, `snapshot`), so the live ones stop drawing as it
  stands in, and they go with it: with the closing front, or under the
  screen decoding over them. Kept live and moving, they left the picture's
  copies doubled behind them.
- The decoded screen fades onto the real window, which shows the same
  pixels: while the host's `onboarding.arrived` is false, setup holds its
  card wall at its start and its frame opaque, and the cards' art drifts by
  the clock, so both copies are on one frame. The host sets it at `done`:
  the card catches the light once, the wall starts drifting and a frosted
  window clears to its glass.

The collapse runs on a still picture, never on the live scene: a layer
shader over the live scene would push it through an offscreen layer every
frame, resampling its characters off their grid. While it waits, the scene
keeps drifting: the camera, each cloud bank, and the breathing edge of
characters. The intro window is capped at 60 fps. Tune the screen's decode
in the gallery's DECODE section (scrub or Play).

Devtools serve one window a process: the intro keeps them, so run
`onboarding` to inspect setup. The intro overlay only draws on the Space
it opened on. From a fullscreen app it is covered and waits; zgx's
`covers_display` window doesn't join the active Space yet.

## Plugins and templates

**A plugin is a folder in the engine's data dir:** `<data>/plugins/<id>/`, with its `zeron-plugin.json`, `init.luau` and files. Dropping one in installs it.

**The engine decides what loads** (`crates/engine/src/plugins`):
- It checks each plugin's signature against the account that owns the key. Every device has its own ed25519 key, registered to the account on the edge.
- It keeps what you allowed.
- It copies each plugin it may run into a snapshot that never changes.

**The app loads only those snapshots** (`src/plugins`):
- Each gets the zgx grants its permissions map to (`src/plugins/grants.rs`).
- Plugins place nodes by selector (`#id`, `.class`, a role and name). The elements are mapped in `src/plugins/elements.rs`, which agents read through `describe_elements`.
- What a plugin reads of Zeron comes as ids where it's placed, plus records the app keeps for it alone, holding only what its grants allow (`src/plugins/records.rs`): `sessions.read`, `messages.read`, `profile.read`, `account.email` and `files.read`.
- An edit never runs before the engine has checked it again. Your own plugins are re-signed as they're saved, so they reload at once.
- Allowing happens in a window of its own with no plugins (`ui/consent.luau`).

**Templates are whole interfaces**, like WordPress themes. Each is a folder, `<data>/templates/<id>/`, holding `zeron-template.json` and any part of Zeron's UI (`app.luau`, `screens/`, `ds/`, `assets/`…). A file it leaves out is Zeron's own.
- One template is in use at a time. The default, "Zeron v3" (id `zeron`), is always there. "Zeron v2" (id `zeron-v2`), the single-page layout from before v3, is Zeron's too: its files are `ui/templates/zeron-v2/`, installed into `<data>/templates/zeron-v2/` and kept current with each version, built in and trusted only while exactly as this build wrote it (`src/plugins/official.rs`). A test fails when a default file v2 replaces changes, until v2's copy is brought in step (`zeron_v2_is_in_step_with_the_default`).
- Settings → Templates lists them as Plugins lists plugins: Installed (the default and Zeron v2, built in, first) and Yours, a row each with Use and a ⋯ menu (its folder, Duplicate, Delete), and New template (a copy of Zeron's own).
- One is out of date when it was made for another version of Zeron: its manifest's `zeron_version` against this build's (the root workspace's version, which `build.rs` hands zgx as `ZERON_VERSION`). A save to Zeron's own while developing never makes one out of date; `template_status` still names the files whose Zeron original changed.
- Agents make and change them through Zeron's MCP server: `list_templates`, `create_template`, `use_template`, `template_status`, `delete_template` and `describe_app`.

**Themes are colours, apart from templates** (`src/themes.rs`, `ui/screens/themes/`, `ui/ds/palettes.luau`).
- Settings → Themes lists Zeron's built-in themes (crates/theme: each family's light and dark together, any other variant alone), each template's own `theme.luau` (read as data), and VS Code themes you import (a `.json` or an extension's folder, by Import theme or dropped on the page; rename and remove). Settings → Appearance has a quick pick for each scheme.
- You choose one theme for light and one for dark, kept in `<data>/theme-choice.json`. `ds/palettes` wears them as the window's two themes (`theme-light`, `theme-dark`) over whichever template runs. A scheme with nothing chosen wears the running template's own theme, else Zeron's.
- A theme is data, so it needs no consent and can't run code.
- The catalogue (what's listed, the library, the choice, palettes and what the window moves to keep them legible) is `zeron_theme::catalog`, shared with the engine. Agents read, make, change, delete and wear themes with Zeron's MCP tools (`list_themes`, `get_theme`, `create_theme`, `update_theme`, `delete_theme`, `use_theme`; guide: `docs/themes.md`). The engine tells the window after each change (`WatchThemes`), and the window reads the files again: a theme changed while it's worn restyles in place.
- Built-in themes were templates once (`theme-<key>`). Each launch moves anyone on one to the default template with that theme chosen, and deletes only the folders shaped exactly as Zeron wrote them (`src/plugins/retired_themes.rs`).

**How the window runs one** (`src/plugins/live.rs`):
- The main window always runs from `<data>/templates/.live/`: Zeron's own (`.zeron/`, copied from `ui/`) with the template in use laid over it.
- Switching, or saving a template's file, rewrites only the files that differ, and the window reloads in place. A module's state is kept wherever both have the file.
- These are never a template's: `screens/plugins/`, `screens/templates/`, `account/`, `.luaurc`, and the consent window, which always runs from `ui/`.
- The main window's errors and devtools sources name `.live/…` files. Other windows, and `zgx test ui`, still run `ui/`.

**When a template fails:**
- A file that fails (at load, at build or later) falls back to Zeron's own for that module alone, with one notice.
- If its `app.luau` fails, the whole window goes back to Zeron's own and the template shows as broken.
- Saving the file again retries it.
- **The way back from any template:** View → **Use Zeron v3** (the host's own menu item, which no template can remove), `zeron template use zeron`, or launching with `--zeron-template`.

**Plugins layer on whatever template is in use.** One that aims at an element the template dropped (its id or class) says "Not shown in this template" on the Plugins page. That check is static: only literal selectors are seen. A placed node that needs what its place doesn't give draws nothing, and its row says why.

**When a plugin fails** (at load, at build, or later), Zeron goes on without it, with the window's state kept (`src/plugins/recovery.rs`).

**Examples and how to write one:** `plugins-dev/README.md`. A staging rig of three engines is in `plugins-dev/rig/`.

```sh
zeron plugin new my-plugin          # yours, signed; edit init.luau and it reloads
zeron plugin sign <id-or-path>      # sign one you wrote as yours
zeron plugin verify <path>          # who made it, and whether it changed
ZERON_ZGX_REVIEW=<id> …  main       # (dev) that plugin's consent window at launch, with devtools
ZERON_ZGX_PLUGIN_LOG=1 …            # (dev) tool calls and answers
zeron template list|use <id>|create <id> [--from F]|status <id>|delete <id>
```

## Not yet

- Release bundles: Zeron's own copy that templates inherit from (`.zeron/`) is made from `ui/`'s source, so a bundled build must ship that source too.
- Templates from others: only templates made on this device (by you or your agents) can be used. A template runs with all of the app's powers, so importing someone else's waits for signing and consent like plugins have.
- The per-screen templates of Oct 31 (`<data>/templates/screens/…`) were development-only and aren't carried over.
- The engine's manager in full (the shipping app's `managed.rs`: the
  installed engine, updates), and Sync's one-time import of a local
  workspace on signing in.
- Release bundling of `ui/` (`zgx_script::build::bundle` + `ScriptHost::bundled`).
- Light theme neutrals against the shipping app.
- Settings → Profile's photo, title and Delete account work against an
  edge that has them (`PUT|DELETE /auth/me/photo`, `GET /auth/avatars/{key}`,
  `PATCH /auth/me {title}`, `DELETE /auth/me {confirm}`): staging does,
  production not until it's deployed there (until then the page says the
  change didn't go through).
- Settings → About & updates updates the engine only where the engine
  manages its own install (a service install on Linux or a Mac); the Zeron
  app's own engine is updated from the app. No download progress: the
  engine's `ApplyUpdate` answers once it's done. "Check for updates" asks
  the public release feed itself (the engine has no call to check now).
  zgx isn't packaged, so nothing updates it.
- Clicking a desktop banner opens the shipping app on its chat: the engine
  posts the banners, and opens whichever app is registered as Zeron. zgx
  needs registering as the app first.
