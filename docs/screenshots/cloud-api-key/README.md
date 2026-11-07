# Cloud API key with an LLM gateway — runtime evidence

Captured on 2026-10-27 from `zeron/custom-api-url-configuration` rebased on `46d4e42b`.
Synthetic accounts and temporary settings; no engine, vault or real key.

## Desktop

Build and run: `cargo run -p zeron-ui --features cloud-api-key-fixture --example cloud-api-key-fixture -- <output>`.
Executable: `target/debug/examples/cloud-api-key-fixture`; SHA256
`356cb0083e9e7843218b653140a0052b96aec619eb8584b84d569fdea6f73db2`. Exited successfully with
the fixture PASS marker; all nine scenes are kept here. The five scenes also captured before
the rebase rendered byte-identical.

- `card-dark.png`: Settings → Providers on Cloud, Claude Code open — "Add Anthropic API key"
  sits beside "Add account".
- `dialog-anthropic-dark.png`: the "Add an Anthropic API key" dialog as it opens — the key
  masked and echoed by the four characters the vault will list it by, Anthropic's API chosen.
- `dialog-gateway-dark.png`, `dialog-gateway-light.png`: LLM gateway chosen; the choice names
  the gateway's host once the base URL holds up, the field is in the key's monospace, and the
  line under it previews the URL Claude Code will call (`/v1/messages` muted).
- `dialog-refused-dark.png`: an `http://` base URL, refused in the engine's own words; Save dims.
- `dialog-saving-dark.png`: Save pressed — a spinner and "Saving…" while the vault seals the key.
- `dialog-openai-dark.png`: Codex's OpenAI key — no Endpoint section; only an Anthropic key
  takes a base URL.
- `saved-dark.png`, `saved-light.png`: the saved key's row — "Anthropic API key", `…81e5`,
  In use — with the gateway's host standing where a login's usage meters would.

## iPhone

Not produced by a committed fixture: there is no Rust iOS target on the capture machine, so a
local, uncommitted harness compiled the app's own `APIKeySheetViewController.swift`,
`CloudUI.swift`, `Palette.swift`, `BrandMarks.swift`, `EdgeFade.swift` and `AccountCell` for
the iOS 27.0 simulator (iPhone 18 Pro) against stand-ins for the three core functions the
sheet calls (`checkApiBaseUrl`, `apiKeySuffix`, `apiKeyTakesBaseUrl`). The verdicts shown are
the stand-ins' for these inputs; the core's own are covered by the `client_ffi::cloud` tests.
None of those Swift files changed upstream between the capture base and `46d4e42b`.
Captured with `xcrun simctl io … screenshot`, status bar overridden to 9:41.

- `ios-list-dark.png`, `ios-list-light.png`: Claude Code's Cloud accounts, through the app's
  `AccountCell`: a login with its meters, then the gateway key with its host by a globe.
- `ios-empty-dark.png`: the sheet as it opens — the key field focused, Save disabled. Paste is
  `UIPasteControl`, dimmed while the simulator's pasteboard holds no text.
- `ios-gateway-dark.png`, `ios-gateway-light.png`: a pasted key (`…81e5`), LLM gateway chosen
  and the route previewed.
- `ios-typing-dark.png`: the base URL being typed — the URL keyboard is up, Save rides above it,
  and the field and its route are scrolled into view; the content above fades under the bar.
- `ios-refused-dark.png`: an `http://` base URL refused under the field; Save disabled.
- `ios-saving-dark.png`: saving — spinner on Save, the fields, choices and close button dimmed.
- `ios-failed-dark.png`: a save the core refused; the reason stands above Save, where the eye
  is after the tap.
- `ios-openai-dark.png`: Codex's OpenAI key — no Endpoint section.

## Validation

After the rebase on `46d4e42b`: `cargo test -p zeron-proto --lib -- cloud::` (7 passed),
`-p zeron-ui --lib -- settings::accounts settings::harnesses composer` (294),
`-p zeron-mobile --lib -- client_ffi::cloud` (2),
`-p zeron-engine --lib -- cloud_accounts:: credentials:: cloud_client::` (20),
`-p zeron-engine --test cloud_runner --test cloud_rpc` (25, 7); `edge/vault` typecheck,
unit (52) and workerd (74) tests; regenerated Swift bindings identical to the committed ones;
the iOS app typechecks against them (`swiftc -typecheck`, iOS 26 simulator target).
`cargo test -p zeron-client --test demo` passed before the rebase only. Static screenshots don't
establish animation, VoiceOver behaviour or a real gateway's compatibility.

- `card-dark.png`: SHA256 `86eca9d72774268e072460a3fa921b71b797b05c696864aedf609abcbfa3696d`
- `dialog-anthropic-dark.png`: SHA256 `29fe81531f243444200b9964fd82bb4b4a1548b6a8f120a53fca7b53601cc22f`
- `dialog-gateway-dark.png`: SHA256 `7a77838469aa3497ef703d75b4a46f585efe5d12ce49bee3c6d0aedff1a9bcfc`
- `dialog-gateway-light.png`: SHA256 `73f9dae001eadf3879c1ff1c5a84952fbf0313e74c03d8967ec728f7cc91c77a`
- `dialog-refused-dark.png`: SHA256 `329d8543f208a86a8ffd59b5339996e562206eb1ec6d81a5e5e3707807f5ee4f`
- `dialog-saving-dark.png`: SHA256 `1b39cc178f533b893d1f418ba0bd20b4bf00f97111511b9144358c5bbf7acc47`
- `dialog-openai-dark.png`: SHA256 `f26be69019e9d82e9180c3aaf59f3af72b32d62f6f064297acc979514517ef0d`
- `saved-dark.png`: SHA256 `cba105589c4fb58d6195352f93c7e8dd21bae4ee979c1371014b9f759d77b39d`
- `saved-light.png`: SHA256 `39e97dceabef185cc9b5b27f358d7757562c951a09b39a17a81c81d8961565c5`
- `ios-list-dark.png`: SHA256 `0a2ea10749638815ef763f98fc22aea01928bec5c819ff8c1efcecf1c5240ee0`
- `ios-list-light.png`: SHA256 `20dd25aac3f024af26670cb559a32104f3b5c0efc091378000987af4cb933e04`
- `ios-empty-dark.png`: SHA256 `4f2b0a99286f8337c1489216bdaf7045c4250d88a7c22d90f809629c3ac3c583`
- `ios-gateway-dark.png`: SHA256 `19f794d219bbc5d6768f61b7d08ba350fd25accfd7573d7c1d1acc11342488aa`
- `ios-gateway-light.png`: SHA256 `4d96f43199ffad7519ec6268398f36a6405cf0e9257d6c57be144c7ac584cf9f`
- `ios-typing-dark.png`: SHA256 `d20dd9131bb3200e9a4e551e1598813d85a2f6e24a90effcaaf16768786d5806`
- `ios-refused-dark.png`: SHA256 `1186052b6d70bea42f917caaa5f78806217cef09052de0e5dccdf7f5faf6bd0a`
- `ios-saving-dark.png`: SHA256 `f32f3803d94d39af04dc9078fc91dbc397e71f7d06433188d6734a58996647c5`
- `ios-failed-dark.png`: SHA256 `817fc8958ecf372bf3d58224eea14e77768992143f9cd403b5a2e64df993ca74`
- `ios-openai-dark.png`: SHA256 `6ba8a316ec612c30425962b58f01879247539bf2d03d0221530e956e4689484b`
