# Writing a Zeron template

A template is a whole interface for Zeron, like a WordPress theme. It's a folder holding any part of Zeron's own Luau app: `app.luau`, `screens/`, `ds/`, `assets/`. Zeron runs its window from its own app with your template laid over it, so any file you leave out is Zeron's own. One template is in use at a time. Colours are separate: the person picks a theme, and it's worn over whichever template runs (see Colours and themes). Agents make and change templates through Zeron's MCP tools; this guide is the whole reference.

## Zeron's templates

Two come with Zeron, listed first on the Templates page, by Zeron:

- **Zeron v3** (id `zeron`), the default. The chat and its panel are rounded cards set into the window under the title bar, and chats can sit side by side: drag one from the sidebar (or a pane's head) to a chat's edge, up to 6 (`screens/shell/split.luau`).
- **Zeron v2** (id `zeron-v2`), the single-page layout from before Zeron v3: one chat at a time, no cards. It's a template like yours, laying 8 files over the default (`ui/templates/zeron-v2/` in Zeron's source). Zeron installs it and updates it with each version, and it can't be deleted.

To make one from either, `create_template` with `from` set to its id.

## How it runs

- The window runs from `<data>/templates/.live/`: Zeron's own files (`.zeron/`) with the template in use on top.
- Saving a file rewrites only what changed, and the window reloads **in place**. State is kept wherever both versions have the module.
- **Fallback, per file:** a file that fails (to load, to build, or later) is replaced by Zeron's own copy of that file alone. You get one notice, and `template_status` names it with its error. Save it again to retry.
- **Fallback, whole:** if your `app.luau` fails, the whole window goes back to Zeron's own and the template shows as broken.
- **The way back from any template:** View → Use Zeron v3 (in the menu bar, or the bar Zeron draws at the window's top on Linux and Windows), Revert on a preview's toast, holding ⌥ as Zeron opens, or launching it with `--zeron-template`. Each starts **safe mode**: Zeron's own interface runs whatever `.active` says, the terminals templates opened are closed, and it stays so across launches until the person picks a template on Zeron's Templates page. A template that crashes Zeron at launch twice in a row starts it too.

## Preview until the person keeps it

A template is Zeron's own code, so it can do anything Zeron can, but only once the person has **kept** it in Zeron's own window. Until then it runs as a **preview**: it draws as it likes, under a toast Zeron draws ("Previewing <name>", with Keep and Revert), and Zeron refuses what would run a program or carry what it reads out of the window. It can't run a program, use a plugin's powers or your account, load a page, open a link, make a request or show a remote picture, change plugins, templates or files through Zeron, delete a chat or a project, sign an agent out or remove its account, disconnect GitHub, or send to agents or answer for them. Everything it draws and its own pictures work as they will once kept. What it saves with `ui.persist` is held: it reads what's saved, and what it writes is saved only if the person keeps it. A preview isn't a sandbox: its code still runs as the app's, and reads what the app reads. Keeping it brings all of this back, without a relaunch.

- Every template you make or switch to with these tools starts as a preview. So does one installed signed by this device, and one changed since it was kept: what the person keeps is the template's files as they were, so an edit after that is a preview again until they keep it again.
- Keep opens Zeron's own sheet in a window of its own. Only that window records the decision, in Zeron's app data, never the engine's. No tool, IPC call or plugin can keep a template for the person.
- Zeron says when the template in use changed without the person choosing it there.

The line this holds is agents that reach Zeron only through its MCP tools, and agents in their usual sandboxes, which can't write Zeron's app data (`~/Library/Application Support/…` on macOS). Code running unsandboxed as the person, with their shell, is already the person: it can write any of their files, this record included, and no template rule stands in its way.
- **Out of date:** a template is out of date when it was made from another version of Zeron. `template_status` lists the files whose Zeron original has changed since you copied them. Compare yours with `.zeron/`'s.

## The folder

`create_template` copies all of Zeron's app into the new folder. Keep what you change and delete the rest to inherit Zeron's (smaller templates stay current as Zeron updates).

```
zeron-template.json    id, name, description, author, version, zeron_version
theme.luau             your template's own theme, as data (see Colours and themes)
app.luau               the root: the window, built from the rest
screens/shell/         the window: init.luau (its frame), sidebar.luau, title_bar.luau,
                       page.luau (the routed page, its cards and split chats),
                       split.luau (which chats sit side by side), settings.luau
                       (Settings' list), route.luau (where it is), appearance.luau
screens/chat/          a chat: home.luau (a new chat), session.luau, composer.luau, store.luau
screens/files/         the files pane
screens/panel/         the side panel (terminals, tabs)
screens/settings/      Settings' pages
ds/                    the design system: tokens.luau (Zeron's tokens), themes/*.json
                       (colours, type, spacing, radii per theme), and every component
assets/                icons and art
```

`describe_app` lists every screen: its module (`@app/screens/shell/sidebar`), its file, what it draws, and the elements it names (`#id`, `.class`).

**Never a template's**, whatever it holds (Zeron's own is used): `screens/plugins/`, `screens/templates/`, `account/`, `.luaurc`, and the window where plugins are allowed. That way a template can never sign you in, allow a plugin, or hide how to switch back.

## The rules

Zeron's interface is written in zgx. A template that follows these stays fast, keeps people's place and survives updates.

1. **Values live in the theme.** Colours, type, spacing, radii and shadows are tokens (a raw colour doesn't follow the person's theme): `$text.primary`, `$space.3`, `$radius.lg`, and Zeron's own under `$zeron.…` (declared in `ds/tokens.luau`). Change the look in `ds/themes/*.json` and `ds/tokens.luau` first; every component follows. A deliberate raw value is `ui.raw(value, "why")`. `zgx lint` flags any other.
2. **Corners are concentric.** Anything inside a rounded box takes its radius less the inset between them, hover washes and selections included. `control.nest(outer, inset…)` in `ds/control.luau` works it out.
3. **Data changes in place.** A list is a keyed `ui.each` with its row as state (`function(item, i, row)`), so rows stay theirs as others come and go. A value that changes is read through a function or a state bound to its prop. `ui.presence`/`ui.switch` are for real swaps (loading, empty, one page to another), keyed by which kind shows, **never by data**: a key made from data rebuilds the part on every change, losing focus and scroll.
4. **Reloads are cheap and quiet.** A reload rebuilds nodes but keeps keyed state. Nothing should animate or call the engine just because a reload happened. An effect owns what it makes: a timer started in an effect stops when the effect runs again.
5. **Keep Zeron's element names.** Plugins place by `#id` and `.class` (`describe_elements`). The stable ones are `#sidebar`, `.session-row`, `#session-header`, `.message`, `#chat-composer`, `#new-chat-background`, `.file-row`, `.file-tab`, `.panel-tab` and the `.md-*` blocks. Keep them, with what they `provide`, wherever you redraw those parts, and plugins keep working on your template. A plugin aimed at one you dropped says "Not shown in this template".
6. **Every save must parse.** A file with a syntax error falls back to Zeron's. A broken `ds/` file can take whole screens with it. Write a file whole, then save.

## Colours and themes

The theme is the person's choice, not the template's. They pick one for light and one for dark on Zeron's Themes page (Settings → Themes: Zeron's built-in themes, VS Code themes they import, and each template's own), and it's worn over whichever template runs. So your template works with every theme, and no theme needs a template.

- **Draw with tokens** (rule 1). A theme sets every colour token (`$text.primary`, `$accent`, `$border`, `$zeron.window`, `$zeron.syntax.*` and the rest) through `ds/palettes`. A raw colour, `ui.raw` included, stays as written under every theme.
- **A theme of your own** goes in `theme.luau` at the template's root: `return { name = "Ink", dark = { … }, light = { … } }`, a palette for each scheme you have, in `ds/palettes`' shape: `background`, `raised`, `text`, `muted`, `faint`, `accent`, `danger`, `warning`, `success`, `syntax` (12 colours: comment, keyword, string, number, type, function, property, variable, punctuation, tag, attribute, invalid) and, if you like, `shell`, `card`, `terminal` and `ansi` (16). It must be data: a table of strings, numbers and tables, no code. Zeron reads it as data to list it on the Themes page as your template's theme.
- **Themes of the person's own** are made with the theme tools (`create_theme`, docs/themes.md), not with a template.
- **It's a suggestion.** Your theme is worn for a scheme only while the person hasn't chosen one for that scheme. A theme with only `dark` fills only dark. Anyone can pick it on the Themes page and wear it with any template.
- Your `ds/themes/zeron-{light,dark}.json` still sets the rest of the look (type, spacing, radii, glass), and its colours show wherever no theme is worn.

Zeron's built-in themes used to be templates (`theme-<key>`, colour-only). They aren't any more: a launch moves anyone on one to the default template with that theme chosen, and removes the folders Zeron wrote for them. A folder of yours under such an id is never touched.

## The loop

1. **`create_template { id, name, from? }`**: a copy of Zeron's app (or of another template). Its reply gives the folder, its files, a short map, and a link to this guide.
2. **`use_template { id }`**: the window switches to it, live, as a preview (see above): ask the person to keep it when it's ready, or before it needs what a preview can't do.
3. **Edit and save.** Each save shows at once in the window.
4. **`template_status { id }`**:
   - whether it's in use;
   - `root_error`: what broke `app.luau`, if anything;
   - `file_errors`: each file that fell back, with its error;
   - `out_of_date`: the files whose Zeron original changed;
   - how many files it has, and how many it inherits.
5. **Fix and save again.** A fallen-back file is retried as you save it.

`list_templates` shows them all with their status: ok, errors, out_of_date or broken. `delete_template` sets one aside. Zeron's own (`zeron`) can't be deleted.

## Two examples

### Restyle the sidebar's rows through tokens

A selected row in the sidebar is a flat surface (`ds/surface.luau`, `depth = "flat"`). Its fill is the token `zeron.selected.flat.bg`, and the gap between rows is `zeron.list.gap`. Change both in your template's `ds/tokens.luau` and every flat selection and list follows, in both themes:

```lua
-- ds/tokens.luau, inside ui.theme.tokens { zeron = { … } }
list = { gap = 4 },                                  -- was 2: rows breathe
selected = {
	bg = "{selection.item}",
	ring = "{border}",
	highlight = "0 0 0 0 #00000000",
	lift = "0 0 0 0 #00000000",
	-- Flat (sidebar rows): an accent wash, not the neutral one.
	flat = { bg = "alpha({accent}, 0.18)" },
},
```

Keep only `ds/tokens.luau` (with this change) in the template, plus `zeron-template.json`. Everything else inherits Zeron's, so it stays current. For more, copy `ds/sidebar.luau` too: its constants (row heights, paddings) are at the top. Keep `class = "session-row"` and its `provide = { session = … }` so plugins still find each row.

### Replace the new-chat page

`screens/chat/home.luau` is what shows under the composer on a new chat. Keep its `id`, its `hook` and its slot (plugins put greetings there) and its file drop (dropping a file on it attaches it):

```lua
-- screens/chat/home.luau
local shared = require("@app/screens/chat/shared")

local function Home(): Node
	local zone = shared.drop_zone("home")
	return ui.column {
		id = "chat-home",
		hook = "chat.home",
		w_full = true,
		h_full = true,
		min_h = 0,
		items = "center",
		justify = "center",
		gap = "$space.3",
		on_file_hover = zone.on_file_hover,
		on_file_drop = zone.on_file_drop,
		drop_when = zone.drop_when,
		on_drag_over = zone.on_drag_over,
		on_drag_leave = zone.on_drag_leave,
		on_drop = zone.on_drop,
		ui.text { "What are we building?", text_size = "$font.size.xl", weight = "$font.weight.semibold", color = "$text.primary" },
		ui.text { "Ask anything. Drop files to attach them.", color = "$text.tertiary" },
		ui.slot("chat.home", { w_full = true, items = "center", gap = "$space.3", mb = "$space.8" }),
	}
end

return Home
```

Save it with the template in use, and the next new chat shows it. If it fails, Zeron's own page shows in its place and `template_status` says why.

## Sharing

`export_template { id, path }` saves a template as one `.zeron-template` file, signed with this device's key (the person's Zeron account vouches for it, as for their plugins). It holds the template's own files and its `preview.png`; the files no template may replace (`screens/plugins/`, `screens/templates/`, `account/`, `.luaurc`) are left out.

`import_template { path }` checks the signature over every file before anything is written; an unsigned or changed package is refused. One signed on this device installs at once, as a preview until the person keeps it. Anyone else's waits for the person: a template runs as Zeron's own code with all its powers, so they allow it in Zeron's own consent window, which shows who signed it. No tool can allow it for them; the reply says `needs_consent`. Protected files in a package are never installed (`ignored` names them). Once installed, `list_templates` shows it with `imported.author`.

`import_template { store }` brings one from Zeron's store, by its id there. An agent brings only a version Zeron has verified, and it always waits for the person, whoever made it; its update is theirs to take too. What a device takes of one (the newest, or the newest verified; pre-releases) is chosen on its page. If Zeron takes a version down as harmful, devices that have it, or a copy made of it, stop using it within about 15 minutes.

A template installed from a link is checked there for a newer version, signed by the same key. Only a later version is taken. One whose files differ from what's installed (anything but its `preview.png`) waits in Zeron's consent window like a new import, and replaces the installed one only once the person allows it there.

## Publishing to the store

The store lists templates people made, beside plugins and under the same rules: a version is live as soon as it's published, Zeron marks the ones it has read **verified**, a version that isn't verified stops taking new installs once a newer one is published, and each version is published once, later than every one before it. docs/plugins.md ("Publishing to the store") says all of that. What's a template's own:

- **It runs as Zeron itself.** A template asks for no permissions, because it can do anything Zeron can: its page, its card and Zeron's consent window say so.
- **Its id is one nobody has**, plugin or template: they're one list of ids, with a plugin's rules (none starting with `zeron`, none that looks like a brand's). Yours is its folder's name, and `zeron-template.json` must say the same.
- **`zeron-template.json`** has `id`, `name` (at most 64 characters), `description` (one line, at most 300), `version` and `zeron_version` (both strict semver; `zeron_version` is the Zeron it was made from, and one older than that can't install it). It may have `keywords`, `screenshots`, `homepage`, `repository` and `license`, as a plugin's may. Nothing else is sent: a template has no categories, permissions, commands, tools or services.
- **`README.md`** is its page, and must be yours: the one Zeron wrote with the template counts as missing.
- **`preview.png`** is its card's cover: a still PNG, at most 4 MB. Screenshots are PNG or JPEG files under `screenshots/`, up to 8. `CHANGELOG.md` gives each version's notes. None of these is part of the interface.
- **Files no template may replace aren't sent**, nor any whose name some disk takes for another's. Each is named by the check (`left_out`, with the manifest's members that aren't sent): changed or added ones one by one, Zeron's own untouched ones as a count. It's never in the way of publishing.

### Checking it

`zeron template check <folder or id>` lists every problem before anything is sent, with no Zeron running and no sign-in. Each has a stable `rule`, as a plugin's does.

### Publishing it

Publishing is the person's act: in Zeron, on its Templates pages, or on the command line. No tool publishes one.

- **From your machine:** `zeron template publish <id>`, with Zeron running and signed in. It's signed with this device's key and sent with your session. `--dry-run` checks it without sending it.
- **From CI:** with a deploy token in `ZERON_DEPLOY_TOKEN`, `zeron template publish <folder>` sends it with no Zeron running. The same tokens publish plugins and templates (`zeron plugin token create --plugins <id>`, an id of either; `--all --templates` for all of both); a template's first version comes from your machine. The GitHub Action (`zeronsh/zeron/actions/publish-plugin`) publishes a template from a folder with `zeron-template.json`.
- **Yank** a version with `zeron template yank <id> <version>` (`unyank` undoes it), and see each version's state with `zeron template store [id]`. (`zeron template status <id>` is how one is on this device.)

A copy of another template is yours to publish only if its author lets you; a copy of one that was taken down can't be published at all.
