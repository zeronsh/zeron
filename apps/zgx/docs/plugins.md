# Writing a Zeron plugin

A plugin is a folder: `zeron-plugin.json` (its manifest), `init.luau` (its code) and `zeron.luau` (Zeron's helpers, the SDK). It runs in Zeron's window in a sandbox of its own, with only what its manifest asks for and the person allowed. Agents make plugins through Zeron's MCP tools. This guide is the whole reference: read it once, then use the loop at the end.

## Manifest

```json
{
  "id": "focus-timer",
  "name": "Focus timer",
  "version": "1.0.0",
  "description": "A timer in a chat's header.",
  "icon": "icon.svg",
  "entry": "init.luau",
  "permissions": ["ui.add", "sessions.read"],
  "ui": { "change": [], "replace": [] },
  "tools": []
}
```

- `id`: 2 to 40 lowercase letters, digits and dashes. `templates`, `zeron` and `app` are taken.
- `icon`: an SVG in your plugin's folder (`"icon.svg"`, 32 KB at most). Zeron draws it tinted in the text's colour on a neutral tile where your plugin is listed (Settings → Plugins, and your plugin's settings page), so draw it in one colour (`currentColor`) on a square viewBox. Without one, your plugin gets its initial on a coloured tile. GitHub's is the GitHub mark.
- `permissions`: only what you use. The person allows them once, in Zeron. Asking for more later asks them again.
- `ui.change`: the selectors you replace, each written exactly as in `zeron.place { at = … }`. Needs `"ui.change"`.
- `ui.replace`: whole screens you replace (`"@app/screens/…"`; never `@app/screens/plugins` or `@app/screens/shell/settings`). Needs `"ui.replace"`. A screen replacement runs as Zeron's own code; prefer `ui.change`.
- `tools`: tools agents can call, each `{ "name", "description", "input" }`. `name` is `^[a-z][a-z0-9_]{0,40}$` without `__`, and `input` is a JSON Schema. Needs `"tools"`.
- `settings`: what the person can set for your plugin (see [Settings](#settings)). Needs no permission.
- `commands`: the programs it runs as you (`["gh", "git"]`), each by its name as it's found on the PATH, never a path. Needs `"commands"`, and the person sees each name when allowing it (see [Programs and a database](#programs-and-a-database)).
- `service`: its headless half (`"service.luau"`), which Zeron's engine runs on each device that allowed the plugin. Needs `"service"` (see [Services](#services)).
- `network`: `{ "hosts": ["api.example.com", "*.example.org", "localhost:8080"] }`, the hosts its requests may reach, from its window and its service alike (see [Reaching the network](#reaching-the-network)). Without it, `network` reaches any public address and never this device, your network or a cloud's metadata address. With it, only those hosts; a private one (`localhost`, `10.…`, `nas.local`) is reachable only when it's named exactly, and the consent sheet says so. `Authorization` and `Cookie` headers go only to hosts named here. Needs `"network"`.
- `mcp`: `{ "command": "./bin/server", "args": ["--quiet"], "env": { "LOG": "off" } }`, a program of its own that Zeron starts as an MCP server beside your agents on this device, named `plugin-<id>`. `command` is required: `./…` is a file in your plugin's folder, anything else runs as named. `args` is a list of text, `env` names and text values. Needs `"native"`.
- `settings_description`: a line under your settings page's title, in place of Zeron's own. Plain text, 200 characters at most; line breaks and runs of spaces become one space.
- `author`: your name, as text. Optional. It's kept in the manifest (and signed with it), but Zeron never shows it as who made the plugin: that comes from the signature (see [Who made it](#who-made-it)).
- `signature`: Zeron writes it when it signs the plugin; never write it yourself. `{ "key", "files", "sig" }`: `key` is the signer's public key (ed25519, base64url), `files` each file's path and its SHA-256 (hex), and `sig` the signature over the manifest with `key` and `files` in it. `files` covers every file in the folder except the manifest and anything whose name starts with a dot. Changing any file, or the manifest, breaks the signature until it's signed again.
- `engines`: `{ "zeron": ">=0.4.0" }`, the versions of Zeron your plugin works with. Zeron won't install it, or update to it, anywhere else (`incompatible`). Without it, every version. The range is comparators separated by spaces (`>=0.4.0 <0.6.0`), sets joined by `||`, `^0.4.0` and `~0.4.0` as npm reads them, a bare version for exactly that one, or `*`. Each version is a full `MAJOR.MINOR.PATCH`.
- `categories`, `keywords`, `homepage`, `repository`, `license`, `screenshots`: how the store lists it (see [Publishing to the store](#publishing-to-the-store)). Only the store reads them. A plugin you share any other way needs none of them, and one that's wrong never stops it running.

Your own plugins are signed as yours, and re-signed and reloaded each time you save.

### Permissions

| permission | lets the plugin |
|---|---|
| `ui.add` | place nodes before or after Zeron's elements (`zeron.place`), add menu items |
| `ui.change` | replace the elements listed in `ui.change` |
| `ui.replace` | replace the screens listed in `ui.replace` |
| `sessions.read` | read sessions' title, status, project, device, times, branch, worktree |
| `messages.read` | read the open chat's messages (role, text) |
| `profile.read` | read your id, name, username, avatar |
| `account.email` | read your email (unless you hide email addresses) |
| `files.read` | read file paths, and folders: a session's (`cwd`) and a project's (`path`) (without it a file is an id of the plugin's own) |
| `panel.tabs` | open tabs of its own in a chat's panel |
| `pages.add` | add pages of its own to the sidebar |
| `navigate` | open chats and pages (`zeron.open`) |
| `links.open` | open http(s) links (`zeron.open_url`), where you open links (Settings → Links) |
| `notifications` | send you notifications (`zeron.notify`), as Zeron's own: Settings → Notifications decides whether they show, a few a minute at most |
| `compose` | put text in the message box, after what's there or in place of its words (`zeron.compose`), or a review comment on it (`zeron.compose_comment`); you still send it |
| `draft` | read the message box's draft as you type it (`zeron.draft`), or any chat's (`zeron.draft_of`), and take it whole, put it back, swap it or let it go (`zeron.take_draft`, `put_draft`, `swap_draft`, `drop_draft`). See [The composer's draft](#the-composers-draft) |
| `github`, `github.write` | read (and write) GitHub through Zeron's engine (older calls; prefer `commands` with `gh`) |
| `network` | the internet: `ui.fetch`, and remote images (`ui.image { src = "https://…" }`). See [Reaching the network](#reaching-the-network) |
| `clipboard`, `files`, `web`, `windows` | zgx's clipboard, file dialogs, web views, windows |
| `tools` | tools for agents (the manifest's `tools`) |
| `devices` | send data to itself on your other devices |
| `native` | run a program of its own (the manifest's `mcp`) |
| `commands` | run the programs its manifest's `commands` names, as you, on this device (`zeron.run`) |
| `service` | run its service (the manifest's `service`) on this device, with no window open, for its windows and agents here and on your other devices |
| `usage.read` | read each agent account's usage: its windows (session, week, …), how much is used, when each resets, its plan. Not who the account is. See [Your agents](#your-agents-usage-accounts-and-models) |
| `accounts.read` | read the saved agent accounts: which is active, which can be switched to, each one's email and name |
| `accounts.switch` | make another saved account the active one for a provider, on this device or one it names. Always asked, even for your own plugins |
| `accounts.pin` | run one chat on a saved account of its own, beside the active one (which stays as it is), and decide the account a new chat starts on. Always asked, even for your own plugins. See [Pinning chats](#pinning-chats-to-accounts) |
| `models.read` | read each harness's models, efforts and options, and a chat's model |
| `models.set` | set a chat's model, effort and options, and what new chats start with. Always asked, even for your own plugins |
| `messages.send` | send a chat a message, start a chat, answer a chat's question, as you would from the composer. Always asked, even for your own plugins. See [Sending messages](#sending-messages) |

## Reaching the network

With `network`, `ui.fetch`, remote pictures and the service's `zeron.fetch` all keep to one rule. It's checked for the address you ask, each redirect, and every address a name resolves to, before anything connects.

| Where | Reached |
|---|---|
| A public address | Yes, unless the manifest names hosts, and then only those |
| This device, your network (`localhost`, `127.…`, `::1`, `10.…`, `192.168.…`, `172.16–31.…`, `100.64–127.…`, `fc00::/7`) | Only if the manifest names it exactly (no `*.`) |
| A cloud's metadata address and link-local space (`169.254.…`, `fe80::`), `0.0.0.0`, `::`, multicast | Never, even named |

**Naming a local host.** Write what the URL says, in `network.hosts`:

```json
"permissions": ["network"],
"network": { "hosts": ["localhost:8080", "127.0.0.1:8080", "[::1]:8080", "nas.local"] }
```

- With a port, only that port. Without one (`"localhost"`, `"nas.local"`), any port on that host, which suits a dev server that changes ports. There's no port wildcard or range.
- `localhost` and `127.0.0.1` are different hosts: name the one you use.
- Naming any host narrows the plugin to the hosts you name, public ones included: a plugin that reaches `localhost:8080` and the internet lists its public hosts too (`*.example.org` covers names under it, never a private address).
- A host named exactly asks the person again when an update adds it, whatever it reads as, because it reaches where it resolves. The sheet says a named host can be on this computer or their network. A `*.` name never reaches a private address, so it needs no asking. A name like `box.tailnet.ts.net` that resolves to your network works once it's named and allowed.

A refused request fails with an error that names what to add, such as `add "localhost:3000" to "network": { "hosts": [] } in its manifest`. `Host`, `Connection` and the like can't be set, and `Authorization` and `Cookie` go only to hosts named here. A program it runs (`commands`) isn't under this rule: what it can reach is what you can.

## Placing by selector

```lua
local zeron = require("./zeron")("focus-timer")   -- your plugin's id

zeron.place { at = "#session-header", after = function(): Node
	return ui.text { "25:00" }
end }
```

`at` is a selector. `before`, `after` or `replace` is a function that builds a node. It runs as a component of its own, so `ui.use_state` works in it.

- `#id`: one element. `.class`: every element with that class. `.a.b`: both classes.
- `#a >> .b`: a `.b` inside `#a`. `role:button "Send"`: by role and name.
- **One node per match.** `.session-row` matches every row, so your node is built once per row. Each copy reads its own row's context.
- What fails in your node draws nothing there. It's reported once, as your plugin's, and the rest goes on.
- **A replacement sees what's provided above the replaced element, not the element's own `provide`.** Zeron puts each element's context on its parent for this reason (see the table). To place inside an element that provides something, place by a part inside it (`.session-title`, not `.session-row`).

`describe_elements` (MCP) lists every named element in the window running now: its selector, where it is, what it provides, whether it's present, and whether it's **stable**. Stable names are kept across Zeron's updates and by every template. Place by stable names where you can.

| stable selector | where | provides |
|---|---|---|
| `#sidebar` | the sidebar's list | `sessions` |
| `.session-row` | a sidebar row (place inside: `.session-title`) | `session` |
| `.project-icon` | a project's icon, wherever it shows: the sidebar's group headers and rows, tabs, the composer's project, project pickers, the panel, the palette, archived chats. Replace it (`ui.change`) to draw each project's own | `project` |
| `#session-header` | a chat's header (title: `#page-title`) | |
| `.message` | a message (`.message-user`, `.message-assistant`) | `message` |
| `#chat-composer` | the message box | |
| `#new-chat-background` | behind a new chat, under Zeron's effect: replace it (`ui.change`) to offer a wallpaper (see below) | `pointer` |
| `.file-row`, `.file-tab` | a file's row in the tree, its tab | `file` |
| `.panel-tab` | a tab of the side panel | `panel` |
| `.md-code` (+ `.lang-<language>`) | a reply's fenced code block | `code` = `{ lang, source }` |
| `.md-math` | a reply's display formula | `tex` |
| `.md-table`, `.md-image`, `.md-quote`, `.md-heading` | a reply's table, picture, quote, heading | |

Everything in the window also has `profile`, `appearance` and `route`. A chat's page and all in it has `session`.

### Slots: places a plugin offers other plugins

A selector reaches Zeron's elements, never another plugin's. A plugin that wants others to add to what it draws offers a **slot**: `zeron.slot(name, props?)` is a column (styled by `props`) holding what each plugin adds there, named under the offering plugin's id. A raw `ui.slot(name)` must be `<your id>.…` too: any other name, or one starting with `zeron`, fails the plugin as it loads, and its row says why. Put a `provide` above it for what they read there: plain data, or a state of it (never a function).

```lua
-- Git's Changes head: its branch, for whoever adds a button beside it.
ui.row { provide = { git_branch = branch }, zeron.slot("branch", { direction = "row", gap = "$space.1" }) }
```

```lua
-- GitHub: Create pull request there ("ui.add").
zeron.place { at = "slot:git.branch", after = function(): Node
	local branch = zeron.live("git_branch")   -- follows it as it changes
	…
end }
```

`before` and `after` both add to a slot, in plugin order; a slot can't be replaced. Zeron offers slots of its own under `zeron.`, a name no plugin can take: `slot:zeron.panel.branch`, the end of the panel home's branch row (`panel_branch`: `{ session, device, cwd, branch }`), where the Git plugin says what the branch adds over its base. Zeron's Git plugin offers `slot:git.branch` (in its Changes head, `git_branch`: `{ session, device, cwd, branch, upstream, remote, remote_url, pushed, ahead, behind, base, base_ahead, head }`), and in History, each giving `git_commit` (`{ sha, subject, author, email, at, parents, refs, remote_url, session }`): `slot:git.commit` (after a row's refs), `slot:git.commit.face` (over the author's face; `git_face` is its size) and `slot:git.commit.head` (an opened commit's head). The GitHub plugin puts its pull request in the first, and in History the author's picture, the pull request a commit came in by, and View on GitHub.

### Menus a plugin offers

Selectors and slots place nodes; a menu's items are data with a function to run, and a function never crosses from one plugin to another. So a plugin offers a menu by name, and other plugins add items to it the way they add to Zeron's own menus:

```lua
-- Git: a History row's menu, its own items then what others added.
ContextMenu { items = function()
	local entries = { …its own… }
	for _, e in zeron.menu_entries("commit", { sha = c.sha, remote_url = url }) do table.insert(entries, e) end
	return entries
end, row }
```

```lua
-- GitHub ("ui.add"): an item in Git's menu; `run` gets the context Git gave.
zeron.menu { kind = "git.commit", id = "view", label = zeron.s("view"), run = function(c) zeron.open_url(…c.sha…) end }
```

The menu's name is the offering plugin's id and a name (`git.commit`), so only Git offers it. Zeron keeps every plugin's items, tells each plugin only those for its own menus, and runs a chosen one in the plugin that added it, with the context the offering plugin gave (plain data). A plugin runs items only in its own menus.

## Context

Inside a build function:

- `zeron.context(name)`: what the place provides, or nil.
- `zeron.need(name)`: the same, or draw nothing here. Settings → Plugins then says why ("Focus timer needs a session, but it's in the title bar").
- `zeron.live(name)`: a function that reads `name` as it changes (a list that moves: `sessions`).
- `zeron.context_of("session", id)`: a session's record by id, read live in a reactive function.

Read at the top of the build function and close over the value: a function run later (a reactive prop) is outside the build.

**What a place provides is read-only.** Don't pass it to a component as its props (`Board(page)` fails: "attempt to modify a readonly table"). Give the component props of its own: `Board { page = page }`.

Records hold only what you were allowed. A field you weren't allowed is absent, never an error.

| name | value |
|---|---|
| `session` | `{ id }` + `title, status, project, device, updated, created, branch, worktree` with `sessions.read` |
| `message` | `{ id }` + `role, text` with `messages.read` |
| `project` | `{ id }` + `name, device, icon` (`{ emoji?, image?, hue? }`, as its people chose it) with `sessions.read` |
| `profile` | `{}` + `id, name, username, avatar` with `profile.read`, `email` with `account.email` |
| `file` | `{ id }` + `path, name` with `files.read` (else an id of your plugin's own) |
| `panel` | `{ id, session, open_tab }` |
| `code`, `tex` | plain data from the block |
| `appearance` | `"light"` or `"dark"` |
| `route` | `{ name, session?, page? }` |
| `pointer` | `{ x, y }`: where the pointer is over the page, 0 to 1 (0.5 at rest). Read it with `zeron.live("pointer")` |

### A new chat's background

A plugin can offer a new chat's wallpaper. Replace `#new-chat-background` (list it in your manifest's `ui.change`) and declare a `wallpaper` setting:

```json
"ui": { "change": ["#new-chat-background"] },
"settings": [ { "kind": "wallpaper", "section": "Wallpaper" }, … ]
```

Zeron draws that row on your settings page: "Use as wallpaper", and while it's on, Zeron's effect (dither, ASCII, halftone, scanlines) to draw over your wallpaper. Only the person turns it on, there. While it's on, a new chat opens on what you build at `#new-chat-background` and Settings → Appearance says the wallpaper is from your plugin, with a way to your page; choosing None or Image there takes it back. One plugin's wallpaper at a time: turning yours on replaces another's. Turned off or removed, Zeron goes back to Appearance's choice. Zeron fades what you draw from the top and lays the effect over it, so draw it plain: fill the box (`absolute`, every edge 0) and keep your pictures in your plugin's folder. The row has no value of yours. Zeron's own Parallax scenes plugin (`plugins-builtin/parallax-scenes`) is this whole.

## Doing things

| call | what | needs |
|---|---|---|
| `zeron.open("home" \| { session = id } \| { settings = page })` | open a page | `navigate` |
| `zeron.compose(text, { replace?, session? })` | text into a chat's draft: after what's there (on a line of its own), or in place of its words (`replace = true`, its chips kept); the chat shown's draft, or `session`'s ([below](#the-composers-draft)) | `compose` |
| `zeron.compose_comment { file, line, note, to?, side?, snippet?, key?, session? }` | a review comment on a file or a diff, as a chip on a chat's draft (below) | `compose` |
| `zeron.composed()` | in a build: your comments still on a draft, by `key`: `{ session, hovered }` | `compose` |
| `zeron.draft()` | in a build: the message box's draft, `{ session?, text, attachments = { { kind, name } } }` (below) | `draft` |
| `zeron.take_draft(key)`, `zeron.put_draft(key)`, `zeron.swap_draft(key)`, `zeron.drop_draft(key)` | the draft taken whole and kept by Zeron under your key; put back; traded with what the key keeps; let go (below) | `draft` |
| `zeron.drafts()` | in a build: what Zeron keeps for you, by key: `{ session?, text, attachments, at }` | `draft` |
| `zeron.draft_of(session)` | a promise of chat `session`'s draft as it is now, as `zeron.draft()` says it (`""`: a new chat's) | `draft` |
| `zeron.toast(text)` | a short note at the window's foot | (always) |
| `zeron.need("panel").open_tab { kind, title, icon?, data? }` | a tab of yours in the chat's panel; asked again, the one there is brought forward (found by `data.key` when you give one, else by its title) | `panel.tabs` |
| `zeron.tab(kind, function(tab) return node end)` | how a kind of your tabs draws (`tab` = `{ kind, title, data }`) | `panel.tabs` |
| `zeron.page { id, title, icon?, build = function(page) … end }` | a page of yours, in the sidebar (`title` may be `zeron.t("…")`, following the language) | `pages.add` |
| `zeron.menu { kind = "session" \| "file" \| "<plugin>.<menu>", id, label, run }` | an item in a row's menu (`run` gets `{ id }` or `{ path, session }`), or in a menu another plugin offers (its context) | `ui.add` |
| `zeron.menu_entries(name, context)` | the items others added to a menu of yours (`<id>.<name>`), as ContextMenu entries | |
| `zeron.store(key, default)` | kept between runs (1 MB): `:get()`, `:peek()`, `:set(v)` | (always) |
| `zeron.every(seconds, fn)` | now and then, 30 s at least; returns a stop function | (always) |
| `zeron.window(peek?)` | `{ shown, active }`: whether the window shows (not hidden, minimised or covered) and is in front, read as states (an effect reading them runs again). Leave work no one would see for when it shows | (always) |
| `zeron.db.exec(sql, params)`, `zeron.db.query(sql, params)` | its own SQLite database: promises of `{ changes, last_id }` and of rows | (always) |
| `zeron.run(program, args, { cwd, stdin, timeout, env })` | a program its manifest names, as you: a promise of `{ status, stdout, stderr }` | `commands` |
| `zeron.send(to, data)`, `zeron.on_message(fn)`, `zeron.devices()` | the same plugin on your other devices | `devices` |
| `zeron.github.watch_pr(session_id)` | in a build: a function reading that session's PR `{ number, title, url, state, base, head }` | `github` |
| `zeron.github.pr(url)`, `zeron.github.diff(url)` | promises of a PR's details or diff | `github` |
| `zeron.github.at(session?, project?)` | the same calls, asked on the device that session's or project's checkout is on; also `.line_comment(url, { path, line, side, commit }, body)`, `.reply(url, thread, body)`, `.resolve(url, thread, resolved)`, `.comment(url, body)` | `github`, writes `github.write` |
| `zeron.github.repository(project)`, `zeron.github.list(project, repository, { filter, after, refresh, cached })` | a project's repository (`owner/name`; its folder stays Zeron's) and its open PRs a page at a time | `github` |
| `zeron.notify { title, body?, session? }` | a notification, as Zeron's own: under Settings → Notifications, a few a minute; a click opens `session`. A promise of `"posted"`, `"off"`, `"held"`, `"paced"` or `"unavailable"` | `notifications` |
| `zeron.open_url(url)` | an http(s) link, where you open links (Settings → Links) | `links.open` |
| `zeron.sessions()`, `zeron.projects()` | your sessions and projects by id, as they change | `sessions.read` |

**Cost.** Zeron times each build of your nodes. Past 1 ms a build (a row's badge) or 8 ms of builds in one frame, your row in Settings → Plugins says so ("Slow: 4.2 ms per row"), and `plugin_status` gives the numbers. Do the work outside the build (in `zeron.every`, or once per id), and read what changes in reactive functions.

**Commands and shortcuts.** zgx's own `ui.command { id, title, menu, key, run }` works in a plugin. It shows in Zeron's menu bar as `<plugin>/<id>`, with `key` as its shortcut (`"cmd+shift+t"`). A shortcut Zeron (or an earlier plugin) already has stays theirs: yours shows without it, and your row in Settings → Plugins says so.

A shortcut needs a modifier: `"k"`, `"enter"`, `"f5"` or Shift alone on a character (`"shift+k"`) would take that key from every field, page and terminal. Written so, the plugin fails as it loads, and its row says why. A key alone that a state or function gives is dropped: the command stays in the menu bar without it, and the row says so. For letters while your own view has focus, use `on_key` on your element, or `panel_key` in the panel. A command's `id` can't start with `zeron`, those names being Zeron's own.

**Tools for agents.** List each in the manifest's `tools` (and ask `"tools"`), then:

```lua
zeron.tool { name = "count_words", run = function(args, call)
	local words = 0
	for _ in string.gmatch(args.text or "", "%S+") do
		words += 1
	end
	return { words = words }
end }
```

`run` returns the answer, or `error()` fails the call. It runs as a task, so it can await (`ui.fetch`). `call.chat` is the chat that called. Tools run while Zeron is open on this device; a plugin with a [service](#services) can answer them there instead, with no window open.

### Review comments on the composer

`zeron.compose_comment` leaves a review comment on a file or a diff on a chat's draft (`session`; without one, the draft shown), as the files pane leaves its own: a chip in the composer naming the file and lines (`graph.rs:10–12`), its note under it. Nothing goes until the person sends. Then, after their words, every comment goes in Zeron's review comments block, as production's composer writes it (crates/ui `comments.rs`), so the agent reads each as its file, its lines, its side and its note, with the code under it:

~~~
Comments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):
- src/graph.rs:10-12 (R): Keep the old name.
  ```
   fn lanes() {}
  -fn rows() {}
  +fn rows(n: usize) {}
  ```
~~~

`file` is the project's path; `line` (and a range's last, `to`) the line numbers; `side` `"L"` for the original file's lines (a line taken out), `"R"` for the changed file's; `snippet` the code (at most 40 lines go); `note` what the person said. Without a `side`, it's a comment on a file (`- src/app.rs:12: …`, under "Review comments").

Give each a `key` of your own: `zeron.composed()` lists yours still on a draft by it (taken off or sent, gone), and says which one's chip is under the pointer (`hovered`), so the lines it was left on can keep a mark and light up as its chip is pointed at. Zeron's Git plugin does all of this in its Changes (`plugins-builtin/git/changes.luau`): the diff's + (or a drag from it over lines) opens `ds/comment`'s composer under them, using `ds/git_changes`'s `on_comment`, `ranges` and `marks`.

### The composer's draft

With `draft`, a plugin reads what you're typing and can set it aside and bring it back: a stash for a prompt written in the wrong chat, a counter of its length, a bar that says what it carries.

- **`zeron.draft()`**, read in a build or an effect, is the draft shown as it changes: `session` (the chat it's for; nil on a new chat), `text` (the words as typed) and `attachments`, each chip's `kind` (`"file"`, `"image"`, `"chat"`, `"comment"`, …) and `name`. A file's path is never in it.
- **`zeron.take_draft(key)`** takes the draft shown whole (its words, its chips, the passages quoted in it) and clears the composer. Zeron keeps it, under your `key` (1 to 64 characters), so your plugin never holds a file's path. An empty draft isn't taken; taking under a key that's kept replaces what it kept. Up to 16 at once.
- **`zeron.put_draft(key)`** puts what `key` kept back on the draft shown, in whichever chat is open, and lets it go. Onto an empty composer it's as it was taken; onto words already there, its words follow after a blank line and its chips after theirs.
- **`zeron.swap_draft(key)`** trades the draft shown with what `key` keeps. Either may be empty, so one call sets the draft aside (nothing kept), brings it back (an empty composer), or swaps the two.
- **`zeron.drop_draft(key)`** lets what `key` kept go.
- **`zeron.drafts()`**, read in a build, is what Zeron keeps for you, by key: `{ session?, text, attachments, at }` (`at` as `os.time()`). Drafts are kept while Zeron runs, as chats' own drafts are.
- **`zeron.draft_of(session)`** reads another chat's draft once, shown or not: a promise of `{ session?, text, attachments }`, as `zeron.draft()` says it (`""` for a new chat's, with no session).

With `compose` too, a plugin can change the words in place: **`zeron.compose(text, { replace = true })`** puts `text` in place of the draft's words, leaving its chips where they are, and `session` names another chat's draft. A rewrite or a snippet that expands is a read and a replace:

```lua
-- ("ui.add", "draft", "compose"): /bug alone in the box becomes the template.
ui.effect(function()
	local d = zeron.draft()
	if d and d.text == "/bug" then
		zeron.compose("## What happened\n\n## What I expected\n\n## Steps\n", { replace = true, session = d.session })
	end
end)
```

Nothing is sent: the person sees the new words in the composer, and sends or edits them. `zeron.compose` takes 20,000 characters at most.

Each is done in the order you ask. A prompt stash is one shortcut and one chip:

```lua
-- ⌘⇧S (Ctrl+Shift+S): set the draft aside, bring it back, or swap the two.
ui.command { id = "stash", title = "Stash prompt", key = "cmd+shift+s", run = function()
	zeron.swap_draft("stash")
end }

-- While one is kept, a chip over the composer: click to bring it back.
zeron.place { at = "#chat-composer", before = function(): Node
	return ui.show(function(): boolean
		return zeron.drafts().stash ~= nil
	end, ui.button { "Stashed", on_click = function() zeron.swap_draft("stash") end })
end }
```

## Programs and a database

A plugin can do real work of its own: run the command-line tools you already use, and keep what it learns.

**Programs** (`commands`). Name the programs in the manifest and ask for the permission. The person sees each one, and a shell or an interpreter (`sh`, `python3`, `node`) says plainly that it gives you full control of their computer, so name one only if you must:

```json
"permissions": ["commands"],
"commands": ["gh"]
```

```lua
zeron.run("gh", { "api", "user", "--jq", ".login" }):after(function(r)
	if r.status == 0 then zeron.toast(`Signed in to GitHub as {r.stdout}`) end
end)
```

- Each runs as you, on this device, with your login shell's PATH and your sign-ins (gh's GitHub login, git's credentials). The consent sheet names the programs and says so. Most programs can run others or write any file, so the sheet says a plugin that names one (git, gh, a shell, an interpreter, or anything not known to be harmless) can do anything you can, and asks every time.
- No shell: `args` reach the program exactly as given, so nothing in them is expanded or split. `stdin` is written to it; `cwd` (an absolute folder) is where it runs.
- `env` sets harmless variables over yours: `LANG`, `LC_…`, `TZ`, `TERM`, `NO_COLOR`, `FORCE_COLOR`, `COLUMNS`, `CI`, git's identity (`GIT_AUTHOR_…`, `GIT_COMMITTER_…`), `GIT_TERMINAL_PROMPT`, `GIT_OPTIONAL_LOCKS`, `GIT_INDEX_FILE` (a `.index` file in a repository's git folder), gh's quiet switches (`GH_PROMPT_DISABLED`, …, `GH_REPO`), and your own `ZERON_PLUGIN_…` names. Nothing that changes what runs or what loads into it (`PATH`, `GIT_SSH_COMMAND`, `BASH_ENV`, `NODE_OPTIONS`, `PYTHON…`, `DYLD_…`, …).
- git runs as git's own commands only (no aliases, no `git-<x>` from the PATH), with `-c` only for how it prints (`core.quotepath`, `color.…`, `core.pager=cat`, `user.name`, …), never `--upload-pack`, `--exec`, `--output`, `--template` or `--config`, `-C` inside the run's `cwd`, and `git config` read only. Every run has a repository's own fsmonitor turned off, no `ext::` transport, no pager, editor or prompt. Hooks run only where git would run yours and Zeron knows the checkout: a commit, merge, pull, push, rebase or branch switch from a service, in one of this device's chats' or projects' folders. Reading, anything elsewhere, and runs from a window run none. gh runs its own commands only: never `alias`, `extension`, `codespace`, `auth` except `auth status` (without the token), or `config` except reading.
- A run waits `timeout` seconds at most (60 unless you say, 300 at most), then is stopped (`status` -1). What it prints is kept up to 16 MB.
- Only the programs you named run; anything else is refused.

**A database** (`zeron.db`, no permission). Your plugin's own SQLite file: removed with the plugin, 100 MB at most, and no other file reachable through it. Use it for a cache, an index or a history, anything past what `zeron.store`'s small JSON file holds.

```lua
zeron.db.exec("CREATE TABLE IF NOT EXISTS seen (url TEXT PRIMARY KEY, at INTEGER)")
zeron.db.exec("INSERT OR REPLACE INTO seen VALUES (?, ?)", { url, os.time() })
zeron.db.query("SELECT at FROM seen WHERE url = ?", { url }):after(function(rows)
	local first = rows[1] and rows[1].at
end)
```

Parameters are always bound (`?`), never put into the SQL. `exec` with no parameters runs a script of several statements (a schema). Each call is a promise.

Zeron's GitHub plugin is built this way: it asks GitHub's own API through `gh` (GraphQL and REST), keeps what it read in its database so a pull request opens at once, and polls each repository once a minute for the sidebar's badges.

## Services

A service is your plugin's headless half: `service.luau`, run by Zeron's engine on each device, with no window open. Your windows call it on this device or on another of yours, and agents' tools can be answered by it. It's how a plugin does real work where the work is: a Git plugin runs `git` on the device a chat's checkout is on, whether that's your laptop or a devbox.

```json
"service": "service.luau",
"permissions": ["service", "commands"],
"commands": ["git"]
```

```lua
-- service.luau
local zeron = require("./zeron")("my-git")

zeron.serve("status", function(args, call)
	local r = zeron.run("git", { "-C", args.cwd, "status", "--porcelain" }):await()
	if r.status ~= 0 then error(r.stderr, 0) end
	zeron.emit("checked", { cwd = args.cwd })
	return { clean = r.stdout == "" }
end, { safe = true })
```

```lua
-- init.luau: on the device the session's checkout is on (nil: this one)
zeron.service(session.device):call("status", { cwd = session.cwd }):after(function(s)
	zeron.toast(if s.clean then "Clean" else "Changes")
end, function(err)
	zeron.toast(zeron.reason(err))       -- "offline", "not_approved", …
end)
```

**Each device decides for itself.** A device runs your service only if the plugin is installed there, on, and allowed (with `service`), in the same version as the caller's, signed by the same author. Its `commands` run as the person on that device, with that device's sign-ins. The person allows it once per device, as with every plugin.

**In service.luau**, `require("./zeron")(id)` gives the service's helpers (the window's `ui.*` isn't there):

| call | what | needs |
|---|---|---|
| `zeron.serve(method, fn, { safe? })` | answer `zeron.service(…):call(method, args)`: `fn(args, call)` returns the answer, `error()` fails it. `call.from` is the calling device (`{ id, name }`). `safe = true`: running it twice does no harm (see below) | |
| `zeron.tool { name, run }` | answer an agent's tool (one the manifest's `tools` lists), with no window open | `tools` |
| `zeron.every(seconds, fn)` | now and then (30 s at least), even while nothing calls it: Zeron starts the service to run it. Register timers at the top level, in the same order each time | |
| `zeron.on_stop(fn)` | told before it stops (idle, an update, Zeron quitting) | |
| `zeron.emit(event, data)` | to your windows: those on this device, and those watching from another (`:on`). 64 KB at most | |
| `zeron.watch(folders, fn, { quiet? })` | changes under whole paths on this device, each all the way down: `fn(changed)` once a burst has been quiet `quiet` ms (250), `changed` by folder: its changed paths (relative, `/`-separated), or `true` when anything may have (as it starts, too many, events lost). Reads aren't changes. Returns a stop function; it lasts while the service runs. Pair it with `emit` to tell windows, instead of their polling | `files.read` |
| `zeron.run(program, args, { cwd, stdin, timeout, env })` | a promise of `{ status, stdout, stderr }`, as the window's | `commands` |
| `zeron.fetch(url, { method, headers, body, timeout })` | a promise of `{ status, headers, body }` (http and https; public addresses, or the manifest's `network.hosts`) | `network` |
| `zeron.folders()` | a promise of this device's chats' and projects' folders (whole paths) | `sessions.read` |
| `zeron.notify { title, body?, session? }` | as the window's | `notifications` |
| `zeron.store(key, default)` | `:get()`, `:set(v)`: kept on this device between runs (1 MB), the service's own (not the window's store) | |
| `zeron.sleep(seconds)`, `zeron.task(fn)` | a promise done later; run `fn` as a task of its own | |
| `zeron.json.encode(v)`, `zeron.json.decode(text)`, `zeron.device` | JSON; this device's `{ id, name }` | |

Handlers, tools, timers and `:after` callbacks each run as a task, so `:await()` works in them. A service shares nothing with your windows but what it answers and emits.

**In your windows:**

- `zeron.service(device?)`: the service on `device` (an id: a session's or project's `device`), or on this device without one.
- `:call(method, args, { timeout })`: a promise of what the method returned. 30 seconds unless you say, 300 at most.
- `:on(event, fn)`: `fn(data, event)` for each `zeron.emit` it makes. For another device, its events reach this one while a window here is open. Returns a function that stops listening.
- `zeron.reason(err)`: why a call failed, as a word.

**Failed calls are yours to handle.** Zeron never retries a call for you, and says why it failed:

| reason | the device that was called… |
|---|---|
| `offline`, `timeout` | couldn't be reached, or didn't answer in time |
| `not_installed`, `off`, `not_approved` | doesn't have the plugin, has it turned off, or hasn't allowed it (or its service) yet |
| `version_mismatch`, `other_author` | has another version of it, or someone else's plugin by the same id |
| `no_service`, `not_loaded` | has a version with no service, or one whose service.luau failed to load (the message says why) |
| `unknown_method`, `failed` | doesn't serve that method, or your handler failed (`error()`, or a refused program); the message is yours |
| `interrupted` | stopped during the call, and the method isn't `safe` |
| `old_engine` | runs a Zeron from before services |

The one exception: when the service stops during a call (it ran out of memory, or crashed), a method marked `safe = true` runs once more on a fresh start. Mark only what can run twice: reading, not posting.

**When it runs.** A service starts on its first call, tool call or timer, and stops after 5 minutes with nothing to do. An update loads the new version beside the old one, which answers until the new one has loaded and finishes the calls it has. Each call or resumed task has 2 seconds of work before it fails (await between parts: waiting costs nothing), and the service has 64 MB. `plugin_status` shows the service on this device: running or not, what it serves, its last log lines (`print`) and why it didn't load.

A service has `zeron.on(event, fn, opts)` and the agent calls of [Your agents](#your-agents-usage-accounts-and-models) too: register listeners at the top level, in the same order each time (they're known by their place, as timers are), and Zeron starts the service to tell it, with no window open.

Zeron's Git plugin (`plugins-builtin/git`) is built this way: its service does every piece of git work on the device a chat's checkout is on (status, what changed through an index of its own via `GIT_INDEX_FILE`, diffs, staging, commits, pushes, branches, history), and its window says each refusal in its own words. `plugins-dev/checkout-status` is the small version, whole: a session row's menu asks the service on that session's device for the checkout's branch and changes, and agents get the same through a tool.

## Your agents: usage, accounts and models

A plugin can watch how much of your agents' plans you've used, switch which saved account a provider uses (as Settings → Providers' switcher does), and choose a chat's model, and hear each of those change. The same calls work in a window and in `service.luau`; each is a promise, and a refusal fails it with `"<reason>: <words>"` (`zeron.reason(err)`).

| call | what | needs |
|---|---|---|
| `zeron.usage { provider?, device? }` | each account's usage, as Zeron last read it: `{ { provider, account, active, plan?, fetched_at?, error?, windows = { { label, used, remaining, resets_at? } } } }` (percent; `resets_at` in ms) | `usage.read` |
| `zeron.usage_refresh { device? }` | read usage again now: once every 5 minutes per plugin (`paced` otherwise) | `usage.read` |
| `zeron.accounts { provider?, device? }` | the saved accounts: `{ { provider, account, active, switchable, email?, name?, organization?, plan? } }` | `accounts.read` |
| `zeron.switch_account { provider, account, when_idle?, device? }` | make a saved account the active one: a promise of `{ status = "switched" }`, or `"waiting"` with `when_idle` | `accounts.switch` |
| `zeron.pin_account { provider, chat, account?, device? }` | run a chat on a saved account of its own, from its next run, while the active account stays as it is; no `account` takes the pin back: a promise of `{ provider, chat, account? }`, or `already` when nothing changes | `accounts.pin` |
| `zeron.pins { provider?, device? }` | the chats pinned to an account: `{ { provider, account, since, chat? } }` (`chat` with `sessions.read`) | `accounts.read` |
| `zeron.models(harness, { device? })` | a harness's models: `{ { id, label, efforts, options = { { id, label, default, choices } } } }` | `models.read` |
| `zeron.chat_model(chat)` | a chat's `{ provider, model, effort, options }` | `models.read` |
| `zeron.set_model { chat, model?, effort?, options? }` | a chat's model, effort or options, for its next turn, on the provider it has | `models.set` |
| `zeron.set_default_model { harness, model, effort? }` | what a new chat starts with in this window (a window's call only) | `models.set` |
| `zeron.on(event, fn, opts?)` | `fn(data, event)` for each event below; returns a function that stops it | the event's |

`provider` and `harness` are a harness's id (`"claude-code"`, `"codex"`, …); `account` is an account's id from `zeron.accounts` or `zeron.usage`.

**What it never gets.** No sign-in, token or password; it can't add or remove an account, or read another plugin's grants. A read never makes Zeron ask a provider again (that can spend a saved login's one-time refresh); only `usage_refresh` does, paced.

**Switching.** A switch happens at once, chats running or not; pass `when_idle = true` to switch once that provider's chats on that device have finished (within 30 minutes). Switching to the account that's active already is refused (`already`). Each change says which plugin made it: in its event (`by = { plugin, name }`), and in `plugin_status` (`agents.actions`).

**Another device.** Name it per call (`device = id`, a session's or project's `device`): that device decides with what it allowed your plugin there (installed, on, the same version by the same author, and the permission), as a service call does. A service acts on its own device; to act on another, call your service there.

### Events

They're told as Zeron learns of a change, never by polling: when Settings, the composer's usage ring or a plugin reads accounts, a provider reports usage, a chat's status or config changes, or a catalog is listed. A window hears them while it's open; a service's `zeron.on` hears them with no window open.

| event | data | needs |
|---|---|---|
| `usage.changed` | an account's usage, as `zeron.usage` gives it | `usage.read` |
| `usage.threshold` | `{ provider, account, window, remaining, below, resets_at }`, once as remaining crosses under `opts.below` (percent), again only after it's back over | `usage.read` |
| `usage.reset` | `{ provider, account, window, next_reset_at }`: a window reset | `usage.read` |
| `limit.hit` | `{ provider, account?, window? }`, or `{ provider, blocked_turn = true, chat? }` when a turn was stopped by a limit | `usage.read` |
| `account.switched` | `{ provider, account, from?, by? }` | `accounts.read` |
| `account.pinned` | `{ provider, account, from?, by?, chat? }`: a chat now runs on `account` | `accounts.read` |
| `account.unpinned` | `{ provider, account, by?, reason?, chat? }`: a pin was taken back, or (`reason = "removed"`) its account is gone | `accounts.read` |
| `run.preparing` | `{ provider, chat? }`, asked of a service right before a chat's first run: what its listener returns is the answer. See [Pinning chats](#pinning-chats-to-accounts) | `accounts.pin` |
| `account.signed_out`, `login.expired` | `{ provider, account?, chat? }` | `accounts.read` |
| `model.changed` | `{ chat, provider, model, effort, options, by? }`, or `{ default = true, provider, model, effort, by }` | `models.read` |
| `models.updated` | `{ provider, models }`: a catalog changed | `models.read` |
| `turn.started` | `{ chat, by? }` (`by`: the plugin whose message set it off) | `sessions.read` |
| `turn.finished`, `turn.errored` | `{ chat, outcome, by?, reply? }`: `completed`, `errored`, `interrupted` or `limited`; `reply`, its final words (the last 16 KB), with `messages.read` | `sessions.read` |
| `chat.awaiting_input`, `chat.input_answered` | `{ chat, by? }`: an agent asked a question or needs approval, and was answered | `sessions.read` |

- **Narrowing.** `opts.provider`, `opts.account`, `opts.window` and `opts.chat` let only matching events through. A threshold watches the active account unless `opts.account` names one.
- **A threshold starts from what it sees.** The first reading only learns which side of the line each window is on, so read `zeron.usage()` as you start to act on how things stand now.
- **Only what you were allowed.** A field your plugin wasn't allowed is left out, never blanked: `email` is there with `accounts.read`, `chat` with `sessions.read`, a turn's `provider` with `models.read`.
- **A handler can't hold Zeron up.** Events are told after the change, never before it. A listener that fails (or takes past its time) stays on and hears the next event; your row in Settings → Plugins and `plugin_status` (`agents.notices`) say the latest failure and how many in a row.
- **A loop is stopped.** A plugin told of its own change (`by` is it) that answers with another change, and so on 16 times in a row, has that next change refused (`loop`), and your row says so. A message counts as a change: a turn your message set off is told with your `by`, so sending again on its `turn.finished` is counted. What you heard last decides: answering anyone else's change starts the count over. How often you change things never counts, only your changes setting off your own.

```lua
-- service.luau ("service", "usage.read", "accounts.read", "accounts.switch")
zeron.on("usage.threshold", function(e)
	for _, a in zeron.accounts({ provider = e.provider }):await() do
		if not a.active and a.switchable then
			zeron.switch_account({ provider = e.provider, account = a.account, when_idle = true }):await()
			return
		end
	end
end, { below = 10, provider = "claude-code" })
```

`plugins-dev/account-rotate` is this whole: its service switches Claude Code to the next saved account with more left when the active one drops below 10%, once its chats finish, and its window says so in a toast.

### Pinning chats to accounts

Switching moves everything on the device to another account at once. Pinning moves one chat: `zeron.pin_account` makes a chat run on a saved account of its own while the active account stays as it is, so your other chats and your own terminals keep running on it. Zeron holds the pins and delivers the credential; **which account a chat gets is the plugin's to decide.**

- **A pin lasts the chat's life.** From its next run, until a plugin (or the same one) takes it back. Resume and prompt caching assume stable credentials, so a pin changes only when you or a plugin change it. A chat parked on the old account is replaced, and resumed, before its next turn.
- **What can be pinned.** Each row of `zeron.accounts` says `pinnable` and `mechanism`: Claude Code logins (`"env"`: the saved login's token goes to that chat's process alone) and Muse's kept accounts (`"root"`). Other providers say `pinnable = false` for now, and an API key never can (it bills per token). A pinned Claude Code chat can't use features that need the account's profile (Remote Control). Rows also say `signed_out`; `zeron.usage` rows say `limited_until` while a full window hasn't reset.
- **The first run.** A service listening for `run.preparing` is asked right before a chat that has no pin runs for the first time, one chat at a time. What its listener returns is the answer: `{ account = "…" }` pins the chat there, anything else leaves it on the active account. It has 3 seconds; a listener that's late, fails or names an account that can't be pinned is passed over (its row says so), and each chat is asked once.
- **Pins follow accounts.** An account that's removed takes its pins with it (`account.unpinned` with `reason = "removed"`), and its chats run on the active account.
- **Attributed and stopped like a switch.** Each pin is recorded as the plugin's (`by`, `plugin_status`), and the loop guard counts it.

```lua
-- service.luau ("service", "accounts.read", "accounts.pin", "sessions.read")
zeron.on("run.preparing", function(e)
	for _, a in zeron.accounts({ provider = e.provider }):await() do
		if a.pinnable and not a.signed_out then
			return { account = a.account }
		end
	end
end, { provider = "claude-code" })
```

`plugins-dev/account-pin` is the whole of it: it answers with the saved account that has the fewest chats (counted from `zeron.pins()`), so new chats take turns.


## Sending messages

With `messages.send`, a plugin talks to your agents as you do: it sends a chat a message, starts a chat, and answers a chat's question or approval. The same calls work in a window and in `service.luau`, and each goes the way the composer's would, so a chat behaves exactly as if you'd sent it: queued behind a running turn, steering it, or a turn of its own, wherever the chat runs (its host may be another of your devices).

| call | what | needs |
|---|---|---|
| `zeron.send_message { chat, text, attachments?, mode? }` | a message into `chat`: a turn of its own when nothing runs; while a turn runs, queued for its end (`mode = "queue"`, the default) or steering it (`"steer"`). A promise of `{ status = "sent", chat, delivery = "run" \| "queue" \| "steer", id }` | `messages.send`; `attachments` (files' paths on the chat's device) also `files.read` |
| `zeron.new_chat { prompt, device?, project?, harness?, model?, effort?, cwd? }` | a new chat, sent `prompt` as its first message: on `device` or in `project` (a project's id), with `harness` (else this device's default) and `model` and `effort` from its catalog. A promise of `{ status, chat, id }`: `chat` is the new chat's id | `messages.send` |
| `zeron.answer { chat, answers, request? }` | the question (or approval) `chat` waits on, answered as you would: `answers = { [question] = label }` (or `{ labels }`, or a list of `{ question, labels }`); `request` names one, else the latest. A promise of `{ status = "answered", chat, request }` | `messages.send` |

- **What it sends is yours.** Your agents read it as your message, and do what it says, so `messages.send` is always asked, even for your own plugins, and the consent sheet says so plainly.
- **Who sent it is kept.** Each message a plugin sends is recorded as its (`{ plugin, name }`) beside the chat's transcript, synced with the chat, and in `plugin_status` (`agents.actions`). The transcript shows it as any message.
- **Its turns say so.** The turn your message sets off (or, queued, the next one) is told with your `by`: `turn.started`, `chat.awaiting_input`, `turn.finished`, `turn.errored`.
- **A loop is stopped**, as with switches: sending on your own message's `turn.finished`, 16 times in a row, has the next send refused (`loop`), and your row in Settings → Plugins says so. A send on something else (your own turn ending, say) starts the count over. How often you send never counts.
- **Refusals.** `not_allowed` (the permission, or `files.read` for files), `not_found` (no such chat, model, effort or question waiting), `bad_call` (no text; `"steer"` while a chat waits for an answer: answer it, or queue), `loop`.

```lua
-- service.luau ("service", "sessions.read", "messages.read", "messages.send")
zeron.on("turn.finished", function(e)
	if e.outcome == "completed" and e.reply and string.find(e.reply, "%[continue%]%s*$") then
		zeron.send_message({ chat = e.chat, text = "continue" }):await()
	end
end)
```

`plugins-dev/keep-going` is this whole: its service sends "continue" to a chat whose turn ends with the line `[continue]`, with no window open, and its window says so in a toast.

## Settings

Declare settings in the manifest; Zeron draws their page. While your plugin is on, it has a page of its own in Settings, under "Plugins", titled with its name and built from Zeron's own rows, so it looks native in every theme and template. You never draw it, and no plugin can draw into it. Settings search finds each one.

```json
"settings": [
  { "id": "show", "kind": "switch", "label": "Show the timer", "description": "In each chat's header.", "default": true },
  { "id": "length", "kind": "number", "label": "Length", "min": 5, "max": 90, "unit": "minutes", "default": 25 },
  { "id": "sound", "kind": "choice", "style": "segmented", "label": "Sound",
    "options": [ { "value": "bell", "label": "Bell" }, { "value": "none", "label": "None" } ], "default": "bell" }
]
```

Each setting has an `id` (letters, digits, `- _ .`), a `kind`, a `label`, and optionally a `description`, a `default` and a `section` (consecutive settings with the same `section` share a titled card). By kind:

| kind | value | extra fields |
|---|---|---|
| `switch` | `true` / `false` (default off) | |
| `choice` | one option's `value` (default the first) | `options` `[{ value, label, description? }]` (an option's `description` stands in for the setting's while it's chosen), `style` `"select"` or `"segmented"` |
| `text` | text, 2000 characters at most | `placeholder` |
| `number` | a number, kept within `min` and `max` | `min`, `max`, `step`, `unit` |
| `shortcut` | `"ctrl+alt+space"` (with `ctrl`, `alt` or `cmd` held), or `""` | |
| `folder` | the absolute path of a folder the person chose, or `""` | |
| `wallpaper` | none of yours: Zeron's "Use as wallpaper" row, for a plugin that replaces `#new-chat-background` ([above](#a-new-chats-background)) | |

Read them in your code (no permission):

```lua
local length = zeron.setting("length")        -- :get() follows it, :peek() doesn't
local all = zeron.settings()                  -- every value by id
zeron.on_setting(function(id, value, was)     -- each change the person makes
	zeron.toast(`{id} is now {tostring(value)}`)
end)
```

**Status.** Your plugin can say how a setting stands, and Zeron draws it beside the control: a short badge in a fixed tone (`neutral`, `success`, `warning`, `danger`) and words in the description's place. Plain text only; it goes when your plugin is off.

```lua
zeron.set_status("shortcut", { badge = "Set up", tone = "warning", text = "Another app has these keys. Choose others." })
zeron.set_status("shortcut", nil)   -- cleared
```

**Permission rows** (`{ "kind": "permission", "permission": "screen-recording" | "accessibility" }`) are for Zeron's own plugins only; anyone else's is left out. Zeron supplies the row's words, whether it's granted, the button to grant it and Check again.

- Only the person sets them, on that page; your plugin reads them.
- A setting never set reads its `default`, so changing a default in an update reaches everyone who left it alone. A value that no longer fits (an option you dropped) reads the default again.
- What the person sets is kept for your plugin alone, across restarts, and removed with the plugin.

## Translations

Zeron speaks the language chosen in Settings → General → Language (English or 简体中文 today, or System: your computer's own). A plugin can too: its words go in `locales/`, beside `zeron-plugin.json`, a file per language, with `en.json` the source:

```
my-plugin/
  zeron-plugin.json
  init.luau
  locales/
    en.json        { "timer": { "start": "Start", "left": "{n, plural, one {# minute} other {# minutes}} left" } }
    zh-Hans.json   { "timer": { "start": "开始", "left": "还剩 {n} 分钟" } }
```

```lua
ui.button { zeron.t("timer.start"), on_click = start }   -- follows the language as it changes
ui.text { zeron.t("timer.left", { n = minutes }) }       -- a state or function argument follows too
tooltip = zeron.s("timer.start")                         -- the text now, where only a string goes
zeron.toast(zeron.s("timer.done"))
```

- **Keys** are nested in the file and dotted in code (`timer.start`). Messages are ICU MessageFormat: `{name}`, `{n, plural, one {# item} other {# items}}` (and `=0 {…}`), `{x, select, a {…} other {…}}`. Plurals follow English's rules (`one` for 1, else `other`); a language with one form (Chinese) writes `other` alone.
- **`zeron.t(key, args)`** is for text (a child, `ui.text`, a label): it follows the language, and the arguments if they're states or functions. **`zeron.s(key, args)`** is the text now, for what takes only a string (a tooltip, a toast, a menu item's label). Read in a function, it follows as well.
- **Falling back:** a language the plugin has no file for, or a key that file lacks, shows the plugin's English (`zh-Hans`, then `zh`, then `en`), and a key English lacks shows the key itself. A plugin without `locales/` loses nothing: plain strings work as they always have.
- **Zeron's own words** in what the plugin uses (the design system's buttons, menus and states) follow the language by themselves.
- Language files are part of the plugin: signed and shipped with it like any other file, up to 1 MB together. Zeron reads them as the plugin loads, so after editing one, save `init.luau` (or turn the plugin off and on) to see it.
- `ui.t` is Zeron's own catalog, not the plugin's: use `zeron.t` for the plugin's words.

`plugins-dev/session-words` is a working example: session-status's word in a chat's header, in English and Chinese.

## The loop

1. **`create_plugin { id, name, description, permissions }`** makes the folder and signs it as yours. Its reply gives the folder, its files and a link to this guide.
2. **Edit `init.luau`** (and the manifest, for permissions). Zeron re-signs and reloads it on every save. If the manifest asks for something new, the person allows it in Zeron.
3. **`plugin_status { id, snapshot = true }`** answers without a person:
   - its status (on, off, pending review, tampered, broken, and why) and what it was allowed;
   - each place it made: its selector, how many elements it matched and their boxes;
   - what its Settings row says (a need its place doesn't give, places this template lacks, an error);
   - with `snapshot`, a PNG of the window around its nodes. Read the file at `snapshot`.
4. **Fix and save**, then ask again. Matched 0? Check the selector against `describe_elements`. A need? Place it inside something that provides it.

`list_plugins` lists them all. Selectors that aren't stable may move; templates may rename elements. `plugin_status` and Settings show "Not shown in this template" when yours aren't there.

## Sharing and installing

A plugin travels as its folder, signed. The person installs it from the store (Settings → Plugins → Browse) or with Settings → Plugins → Install…; an agent uses these tools:

- **`search_plugins { query?, category?, keyword?, author?, verified?, sort? }`** searches the store, and **`get_store_plugin { id }`** reads a plugin's page there: its README, what it asks for, its versions (each `verified` or not), `verified_version` (the newest Zeron has verified) and whether it works with this Zeron (`compatible`). Say what it asks for, and whether it's verified, before you suggest it.
- **`install_plugin { store = id }`** installs it from the store. An agent installs only versions Zeron has verified: without a `version`, the newest verified one; a `version` it names must be verified too, else it's the person's to install from the plugin's page (`needs_person`). Zeron checks it's exactly what the store's record says (its files, its version, who signed it) before anything is written (`store_mismatch` when it isn't).
- **`install_plugin { path }`** installs a `.zeron-plugin` file. **`install_plugin { url, ref?, folder? }`** installs from a link:
  - a link to a `.zeron-plugin` file;
  - a GitHub release (`…/releases/latest`, `…/releases/tag/<tag>`), from its `.zeron-plugin` asset;
  - a git repository: `owner/repo`, its GitHub page, `…/tree/<branch>/<folder>`, or any https git URL.
- **`update_plugin { id? }`** only checks. It looks where a plugin installed from someone else came from (the store, its file, link, release or repository) and reports any newer version: its version and the permissions it adds or removes (`added`, `removed`). Without `id`, it checks every one. It never installs anything: the person installs an update in Zeron, with Update on the plugin in Settings → Plugins. Tell them what you found. (`check = true` is still accepted and changes nothing.)

Zeron checks the signature over every file before anything is written, so an unsigned or changed plugin is refused.

When installing or updating fails, the error reads `"<reason>: <words>"`, for example `rate_limited: GitHub is turning away requests…`. The reason is a stable word (`git_missing`, `repo_unreadable`, `rate_limited`, `release_not_found`, `no_package`, `download_failed`, `tampered`, `other_signer`, `not_newer`, `incompatible`, `store_unreachable`, `not_in_store`, `store_mismatch`, …); Zeron's window says each one in the person's language. `rate_limited` means GitHub answered 403 or 429: wait, or sign in with `gh auth login` (Zeron looks up releases as you when `gh` is signed in, or `GITHUB_TOKEN`/`GH_TOKEN` is set). `git_missing` means installing from a repository needs git on that device; a release or a `.zeron-plugin` file doesn't.
- **Signed with this device's key, installed from Zeron's window:** what it does inside its sandbox is allowed at once. Anything outside it (`commands`, `native`, `service`, `devices`, `ui.replace`, `accounts.switch`, `models.set`, `messages.send`) waits for the person. Installed by an agent, it waits.
- **Signed by anyone else:** it waits until the person allows it in Zeron. The reply says `waiting`; you can't allow it for them.
- **From the store:** who made it is who owns its id in the store, so an update signed by another of their keys (a deploy key, a new machine) keeps what the person allowed. An update from a new owner waits for the person again. The store checks for updates for every plugin installed from it at once. A version the store took down as harmful is turned off at the next check, with the reason shown; the person can turn it back on.
- **An update:** it must be signed by the same key as the installed version, or by another live key of the same account (the author on a new machine; an old key revoked still counts as theirs). One signed by a new key installs but waits for the person to allow it again. One that asks for a new permission still installs, but waits for the person to allow just that.
- **Removing a plugin** (Settings) takes away its folder, what it stored, its pages and its placements.

### Making a package

To share a plugin, save it as one signed `.zeron-plugin` file:

- **`export_plugin { id }`** (MCP), or **`zeron plugin export <id> [--to <path>]`** on the command line. Your own plugin is signed again first, so the file holds what's in its folder now. The reply gives the file's `path` and its size (`bytes`).
- Without `--to`, it's saved in Zeron's data folder, as `plugin-exports/<id>-<version>.zeron-plugin` (an earlier one of the same name is replaced). `--to` is a folder to save it in, or a file whose name ends in `.zeron-plugin`. The tool takes `to` too, but an agent's export always goes to `plugin-exports`: leave `to` out and read `path`.

Then attach that file to a GitHub release, or send it. People install it from the file, or from the release's link. You can also commit the plugin's folder, with its signed `zeron-plugin.json`, to a repository: Zeron re-signs your own plugins as you save them, so what you commit is already signed. The package format (`zeron-plugin:v1`) is the same signed package templates use: the manifest, plus every file as base64.

### Who made it

Zeron says who made a plugin from its signature, never from its manifest. Each device makes a signing key of its own and registers it to the Zeron account signed in there (a new key needs a fresh sign-in). To check a plugin, Zeron asks Zeron's servers whose key signed it. The answer is kept for an hour, so a key revoked elsewhere stops counting within that.

| what people see | it means |
|---|---|
| by Zeron | one of Zeron's own, shipped in the app |
| by you, Made by you | signed with this device's key |
| Made on one of your devices | signed with your account's key from another of your devices. It's still asked, like anyone's |
| by @username, Made by @username | signed with a key registered to the account that holds that username, and not revoked; or installed from the store, where @username owns it, exactly as the store's record says it (verified or not, as its record says) |
| Unverified, Unknown author | Zeron can't confirm who made it: it's unsigned, its key was never registered or was revoked, its author has no username yet, or Zeron couldn't check (offline, signed out). The sheet says which, and where it came from |
| Changed since you allowed it | its files changed after you allowed it, in a way its author's signature doesn't vouch for (edited on disk, or an unverified plugin replaced). It doesn't run, and it can't be allowed as it is: its sheet offers Remove, or Reinstall (fetched again from where it came from, then asked again) |

**Claiming a username.** Your plugins read as "by @username" to others only once your account has a username. You claim it once, in Zeron's setup right after you sign in: 3 to 20 letters, digits, `_` or `-`, starting with a letter or digit, kept lowercase. It can't be changed afterwards, and names that look like Zeron's, its staff's or a brand's are reserved. Until you have one, your plugins show as Unverified to everyone else.

A plugin you wrote that shows as Unverified here (one you copied in unsigned, say) can be made yours: the consent sheet's "It's mine…" signs it with this device's key. A plugin someone else signed can't be taken over; copy it under a new id to make your own version.

## Publishing to the store

The store lists plugins people made, on zeron.sh/plugins and in Settings → Plugins → Browse. A version is live as soon as it's published: anyone can install it, and devices with the plugin are offered it as an update at once. Zeron reads versions after they're published and marks the ones it has read **verified**; a reviewer can also decline to verify one (you see why in `zeron plugin status`; others only see that it isn't verified). Every listed version can be installed, not only the newest.

On a device, a version that isn't verified says so: on its page, on the consent sheet and on the update sheet, which also offers the newest verified version when there is one. Installing or updating to an unverified version always asks the person, even when it asks for nothing new. By default a device offers the newest version; it can be set to take the newest verified one instead, for every plugin or one (Settings → Plugins). An "update all" never takes an unverified update: each is its own question. An agent never installs an unverified version. If Zeron takes a version down as harmful, devices that have it turn it off within about 15 minutes.

### What the store needs

- **`README.md`** in the plugin's folder, at most 512 KB: its page. CommonMark and GitHub's tables, task lists and strikethrough. Raw HTML (`<p align="center">`, `<details>`, `<img>`, `<br>`) and footnotes (`[^1]`) aren't drawn: both stores show them as the characters typed, and the publish check says where (`raw_html`, `footnote`, in README.md or CHANGELOG.md). Use Markdown for pictures and headings. Display math (a `$$` block, or a fence marked `math`) is drawn in Zeron but shown as typed on the store's website, and the check says so too (`math`). Pictures are files in the plugin, by a relative path (`![Board](shots/board.png)`); a picture from elsewhere shows as a link, because what's shown is what was reviewed. Links open in the browser.
- **`description`**: one line, at most 300 characters. It's the plugin's summary on its card. The `name` is at most 64.
- **`icon`**: an SVG file in the plugin, at most 32 KB.
- **`categories`**: one or two of `agents`, `appearance`, `chat`, `developer-tools`, `git`, `integrations`, `productivity`, `utilities`.
- **`version`**: strict semver: `MAJOR.MINOR.PATCH`, optionally `-prerelease` and `+build` (`1.2.0`, `2.0.0-beta.1`). No `v`, no leading zeros.
- **`id`**: not one of Zeron's own plugins' (`git`, `github`, `parallax-scenes`), not starting with `zeron`, and not a name that looks like a brand's. The first account to publish an id owns it.
- A username on your Zeron account, so the store can say "by @username".

The rest is optional:

```json
{
  "keywords": ["focus", "timer"],
  "homepage": "https://example.com/focus-timer",
  "repository": "https://github.com/you/focus-timer",
  "license": "MIT",
  "screenshots": ["shots/board.png", "shots/settings.png"],
  "engines": { "zeron": ">=0.4.0" }
}
```

- `keywords`: up to 8, each lowercase letters, digits, dashes and single spaces, at most 32 characters.
- `homepage`, `repository`: `https://` links, at most 2048 characters, written exactly as a browser reads them (`https://example.com/`, with its slash; a lowercase host).
- `license`: a short name, like `MIT`, at most 64 characters.
- `screenshots`: up to 8 PNG or JPEG pictures in the plugin, shown on its page.
- **`CHANGELOG.md`** gives each version's notes: the section under the heading that starts with that version (`## 1.2.0`, `## [1.2.0] - 2026-11-01`, `## v1.2.0`), up to the next heading as high, at most 64 KB. People see the notes when Zeron offers them the update.

Files that aren't text or pictures (a compiled program, bytecode) are allowed, but reviewers look closely at them, and the check says which they are.

What the store shows is what's there:
- The `name`, `description` and `keywords` can't hold control characters or invisible formatting characters (direction overrides, zero-width spaces).
- Two files whose paths a Mac or a Windows machine would see as one (`README.md` and `readme.md`, or the same name in two Unicode forms) are refused.
- The manifest is at most 256 KB, and `CHANGELOG.md` at most 512 KB.
- The package, as sent, is at most 20 MB (a plugin you share any other way can be up to 50 MB), and the manifest in it, its signature included, at most 1 MB and 100,000 values.

### Checking it

**`check_plugin { id }`** (MCP), or **`zeron plugin check <folder or id>`** on the command line, lists every problem before anything is sent. Each problem has a stable `rule`: `missing_readme`, `missing_listing` (with the `field` that's missing), `bad_listing` (with the `field` that's wrong), `not_semver`, `id_reserved`, `bad_package` (the manifest is one Zeron won't run) and `too_large`. Fix them and check again. `zeron plugin check` needs no Zeron running, so a pull request's CI can run it.

### Versions

- Each version is published once and keeps its content for good. To change one, publish a new version.
- A version must be later than every version the plugin has had, in any state (yanked or taken down, pre-releases included).
- A pre-release (`2.0.0-beta.1`) is offered only to people who chose to take that plugin's pre-releases.
- **Yank** a version (`zeron plugin yank <id> <version>`) to stop new installs of it; copies already installed keep working. `zeron plugin unyank` undoes it.
- `zeron plugin status [id]` shows each of your versions, whether it's verified, and the reviewer's reason when they declined to verify it.

### Submitting

Publishing is the person's act, in Zeron (Settings → Plugins → Yours → Publish…) or on the command line. An agent can check a plugin: no tool publishes one, and Zeron never gives an agent the check's ticket that does. (An agent with a shell could still run `zeron plugin publish` as you, so tell it not to unless you mean it.)

- **From your machine:** `zeron plugin publish <id>`, with Zeron running and signed in. Your plugin is signed with this device's key and sent with your session; it's live at once, and the command prints its page. `--dry-run` checks it without sending it.
- **From CI:** use a **deploy key**. It's your account's key, for the plugins you name or for all of yours (those you first publish later included), fixed when it's made: another set of plugins is another key. A key publishes only plugins you already have in the store (a plugin's first version comes from your machine) and never claims a new id. Making one asks you to sign in again. Its secret (`zpk_…`) is shown once, and Zeron keeps no copy. A deploy key publishes new versions of its plugins and reads their state (`zeron plugin status`), nothing else: yank, unyank and keys are yours, from your machine. A version it publishes is live for everyone at once, so treat a key like a password: if one leaks, whoever has it can publish versions of its plugins that people can install (unverified, until Zeron reads them). Revoke a leaked key at once, and yank what it sent. Each version a key publishes shows on your devices as a notice. Put the secret in your CI's secrets as `ZERON_DEPLOY_KEY`, then run `zeron plugin publish <folder>`. No Zeron needs to be running and nobody needs to be signed in: the package is signed with the deploy key, and so is the request that sends it.

  ```sh
  zeron plugin key create --name "GitHub Actions" --plugins focus-timer,focus-board   # once, on your machine
  zeron plugin key create --name "All my plugins" --all
  zeron plugin key list
  zeron plugin key revoke <key>
  ```

  `zeron plugin publish` reads the secret from the environment only, never from an argument. It exits 0 when the plugin is published, 1 when it doesn't meet the store's rules, 2 for a set-up problem (no plugin there, a damaged secret, the runner's clock out by more than five minutes), 3 when the store refuses the key or the account, and 4 when the store can't be reached (try again). A revoked or expired key publishes nothing; versions it published before stay in the store. An account has at most 20 live keys (`too_many_keys`: revoke one first) and makes at most 10 a day (`rate_limited`).

### The GitHub Action

`zeronsh/zeron/actions/publish-plugin` installs Zeron's Linux command line and publishes the plugin with your deploy key. Add the secret to the repository (Settings → Secrets and variables → Actions, as `ZERON_DEPLOY_KEY`), then add these steps to a workflow running on `ubuntu-latest`:

```yaml
- uses: actions/checkout@v4
- uses: zeronsh/zeron/actions/publish-plugin@v0.4.0
  with:
    key: ${{ secrets.ZERON_DEPLOY_KEY }}
```

Pin it to a Zeron release tag or a commit SHA, never a branch: the step holds your key. It runs on Linux runners (`ubuntu-*`, x86_64 or arm64). `path` names the plugin's folder when it isn't the repository's top, and `dry-run: true` checks it without submitting. Without `key` (a pull request from a fork gets no secrets), it only checks. Run it when you push a version's tag, after bumping `version` and adding its `CHANGELOG.md` section.

### Reason words

Every refusal from the store, or from Zeron about the store, reads `"<word>: <words>"` (or, from a method that answers rather than fails, `{ ok: false, error: "<word>", message }`). The word is stable; the words are English for whoever reads them raw.

| word | when |
|---|---|
| `not_a_package`, `unsigned`, `tampered`, `bad_signature`, `too_big`, `bad_file` | the package itself is refused, as from a file |
| `incompatible` | its `engines.zeron` leaves this Zeron out |
| `paths_collide` | two of its paths are one file on macOS and Windows (case, Unicode spelling, trailing dots), a file sits where another needs a folder of that name, or a path is the manifest's name there: refused as it's read, on every disk |
| `needs_person` | an agent tried to install over a plugin installed from the store: updating it (and taking its pre-releases) is the person's, in Zeron's window |
| `other_signer`, `not_newer`, `different_plugin`, `up_to_date`, `no_source` | installing or updating, as from any source |
| `store_unreachable` | the store didn't answer (offline, or busy): what's installed keeps running |
| `not_in_store` | no such plugin or version, or not to you |
| `store_mismatch` | what the store sent isn't what its record says: never installed |
| `store_error` | the store's answer couldn't be read |
| `rate_limited` | too many requests for now |
| `signed_out` | it needs you signed in |
| `review` | publishing needs the check Zeron's window or the CLI made, for the plugin as it was checked |
| `changed` | the plugin changed since it was checked: check it again |
| `not_owner`, `invalid_scope` | a deploy key's scope names plugins that aren't yours in the store (`ids` says which), or isn't a scope |
| `not_verified` | an "update all" (`verified_only`) left an unverified update for its own question |
| `step_up_required` | making a deploy key needs a fresh sign-in begun for it (`StepUp {for: "deploy_key"}`): each key its own, and no other sign-in's |
| `bad_package`, `id_reserved`, `not_semver`, `missing_readme`, `missing_listing`, `bad_listing`, `too_large` | the store's rules (`check_plugin`); `binary_file`, `no_notes`, `raw_html`, `footnote` and `math` are warnings |
| `needs_username`, `id_taken`, `version_exists`, `suspended` | the store refused the submission |
| `key_not_allowed`, `key_revoked`, `key_expired`, `invalid_signature`, `unauthenticated` | the store refused the deploy key |
| `unauthenticated`, `forbidden` | the store turned away who's asking (a 401 or 403 that said no more): for a signed-in person, sign in again |
| `stale_request` | the clock is more than five minutes out |
| `replayed_request`, `body_mismatch` | the request was already made, or changed on its way: send it again |
| `too_many_keys`, `key_taken`, `invalid_name`, `invalid_expiry`, `invalid_proof` | making a deploy key |

## Trust model

- A plugin you install runs with the access you grant it, and nothing more. It runs in its own sandbox, and each call it makes is checked against what you allowed.
- What you grant, it has. A plugin with `commands`, `native` or `service` runs programs as you, so it can do anything you can. Grant those only to a plugin you trust.
- Only you allow a plugin, in Zeron's window. An agent can make and install plugins, but never allow one for you.
- Code already running as you on your computer (for example an agent with a shell) is trusted the same as you. Zeron doesn't try to confine it.

## Three examples

### A badge on each sidebar row

```json
{ "id": "row-branch", "name": "Row branch", "version": "1.0.0", "entry": "init.luau",
  "permissions": ["ui.add", "sessions.read"] }
```

```lua
local zeron = require("./zeron")("row-branch")

zeron.place {
	at = ".session-title",          -- inside each row, so the row's session is above it
	after = function(): Node
		local id = zeron.need("session").id
		return ui.text {
			function(): string
				return zeron.context_of("session", id).branch or ""
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}
	end,
}
```

### A widget in a chat's header

```json
{ "id": "focus-timer", "name": "Focus timer", "version": "1.0.0", "entry": "init.luau",
  "permissions": ["ui.add"] }
```

```lua
local zeron = require("./zeron")("focus-timer")

zeron.place {
	at = "#page-title",              -- in #session-header, which has the session
	after = function(): Node
		zeron.need("session")        -- only on a chat's page
		local left = ui.use_state(25 * 60)
		-- (A timer belongs to the node it's made in: it stops with it.)
		ui.every(1000, function()
			left:set(math.max(0, left:peek() - 1))
		end)
		return ui.text {
			function(): string
				local s = left:get()
				return string.format("%d:%02d", s // 60, s % 60)
			end,
			tabular = true,
			text_size = "$font.size.sm",
			color = "$text.secondary",
		}
	end,
}
```

### A renderer for a kind of code block

Replies' fenced blocks are `.md-code.lang-<language>`, and each provides `code`. Replace one kind, and the rest stay code blocks. A block still streaming in is built again as it grows. With your plugin off, it's a code block again. (`plugins-dev/md-hello` is this, with tests.)

```json
{ "id": "mermaid-view", "name": "Mermaid diagrams", "version": "1.0.0", "entry": "init.luau",
  "permissions": ["ui.change"], "ui": { "change": [".md-code.lang-mermaid"] } }
```

```lua
local zeron = require("./zeron")("mermaid-view")

zeron.place {
	at = ".md-code.lang-mermaid",
	replace = function(): Node
		local code = zeron.need("code")      -- { lang = "mermaid", source = "graph TD; A-->B" }
		return ui.canvas {
			w_full = true,
			h = 240,
			draw = function(c)
				-- lay code.source out and draw it: boxes, arrows, labels
			end,
		}
	end,
}
```

## Zeron's components

Zeron lends your plugin its design system: the same components Zeron's own screens are built from, from whichever template is active, so your plugin looks native in every theme and template. It needs no permission. Require the modules you use from `@ds/…` (not `@app/…`, which plugins can't reach):

```lua
local Button = require("@ds/button")
local Badge = require("@ds/badge")
local Settings = require("@ds/settings")
local git = require("@ds/git")             -- a change's pieces: header, diff, checks, activity

zeron.tab("my-review", function(tab: any): Node
	return ui.column {
		w_full = true,
		gap = "$space.5",
		git.header({ state = "open", number = 12, title = tab.title, branch = "fix", base = "main", added = 3, removed = 1 }),
		Settings.Group {
			Settings.Row { label = "Checks", description = "2 of 3 passed", control = Button { "Re-run", tone = "secondary" } },
		},
	}
end)
```

- **Use them before drawing your own.** Text (`@ds/text`), buttons and icon buttons, fields, selects, tabs, badges, callouts, empty states, settings cards and rows, markdown, comments and threads (`@ds/comment`, `@ds/inline_block`), and the git pieces (`@ds/git`, `@ds/git_changes`). They hold the theme's spacing, corners, focus rings and motion, which is what makes a plugin look like part of Zeron rather than beside it. `ui/ds/*.luau` is their source, each with its usage at the top, and the gallery (`zeron-zgx gallery`) shows each one.
- **In your plugin's own VM.** The modules load in your sandbox, with state of their own; nothing is shared with Zeron's copy or another plugin's. They lend no authority: what they do is limited by your permissions, as your own code is. Set a markdown link's destination with `require("@ds/links").opener = function(url) zeron.open_url(url) return true end` (`links.open`).
- **Require a module, not `@ds` itself.** `@ds` loads every component Zeron has; `@ds/button` loads one.
- **A tab or a page fills its room.** What `zeron.tab` and `zeron.page` build is given the room the panel or page has: `h_full` on it, then scroll inside (`ui.scroll`) or keep to the height and scroll in parts.

- **Faces take a picture from anywhere you can reach.** A person (`{ id, name, src }`) shows `src` over their gradient, which shows until it arrives or if it never does; `src` may be a remote address with `network`.
- **The lightbox works in a plugin.** Put one `require("@ds/lightbox").Host()` in what you build (a tab's, a page's); a markdown picture then opens whole over the window on a click, as a chat's does. (Zeron tells your plugin the window's size for it, as `ui.shared("zeron.window")`, since `ui.window` isn't a plugin's.)

Zeron's GitHub plugin (`plugins-builtin/github`) is built this way throughout, with nothing of Zeron's made for it: its pull request viewer and board are the gallery's review design on `@ds/git`, its sidebar badge is `@ds/badge`'s, people's faces are their GitHub pictures (loaded with `network`), and everything it knows comes from GitHub through `gh` (`commands`), kept in its database. Its words are in `locales/en.json` (`zeron.t`, `zeron.s`), so it follows the language like the rest of Zeron.

## Style

Prefer Zeron's components (above). Where you draw your own, use Zeron's tokens, never raw values: colours (`$text.primary`, `$text.secondary`, `$accent`, `$border`), spacing (`$space.1`…`$space.8`), radii (`$radius.sm`…`$radius.xl`) and type (`$font.size.xs`…`$font.size.lg`). Your nodes then follow the person's theme and accent. Inside something rounded, a corner is the outer radius less the inset.
