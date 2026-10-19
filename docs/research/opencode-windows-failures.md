# OpenCode failures on Windows

Investigated on October 2, 2026 against Zeron commit `69e64ef5` and the native Windows x64 OpenCode binaries published as `opencode-ai` 1.18.34 and `@opencode/cli` 2.0.22. The reported symptom is a new chat failing on its first message. No affected user's error, executable path, versions, provider, or logs were available, so this investigation establishes failure mechanisms rather than attributing a particular incident.

The investigation below describes the baseline before the accompanying reliability fix. The fix introduces a shared OpenCode path resolver, a generation-aware credential adapter, project-scoped run preflight, and native subprocess regression coverage.

## Reliability changes

- Windows credential paths, global discovery and cache invalidation use Node's `USERPROFILE` home and absolute XDG overrides. Global probes explicitly start in that home; runs validate models and v2 agents against the project's live catalog. An explicit empty connected-provider list yields no models.
- Account operations resolve the same CLI as chat startup. OpenCode owns database initialization and migration. Zeron uses legacy JSON for 1.x and a schema-checked SQLite adapter for 2.x; it rejects unknown credential generations. The v2 public API only renames, activates and removes credentials, so importing Zeron's OAuth token sets requires the isolated SQLite adapter. Transactions preserve other providers and native credential IDs. Switching back to a native ID preserves its latest refresh token, and removal checks the captured active ID inside the transaction. Native removal can activate another saved credential, matching OpenCode's behavior.
- Credential fingerprints read only credential rows through SQLite, including WAL updates, rather than hashing chat databases or invalidating discovery on every new chat. OpenCode 1.x inline auth overrides reject account writes that the CLI would ignore.
- Launch owns both password variables and the v1 username. Explicit executable overrides are validated and never silently fall back. Windows discovery includes the standalone OpenCodeCLI install location. Cold version probes share results across operations, retry negative results after a short cooldown, and include launch dependencies in PATH.
- Readiness races authenticated health checks against child exit and the overall deadline. Startup errors include the binary and working directory; stderr is drained and redacted before it reaches diagnostics. An unavailable SSE subscription prevents prompt submission and remains cancellable. Empty v2 provider-auth errors include a reconnect instruction.
- Attachments use encoded file URLs for drive paths, UNC shares, Unicode and reserved characters. Invalid paths produce an error before a prompt is submitted. Optional MCP injection failures preserve inherited configuration and let OpenCode validate it itself.

Regression coverage includes native 1.x/2.x process startup, a slow/unknown version probe, inherited credentials, disabled MCP, quick crashes, invalid overrides, Windows `.cmd` launch, project preflight, unavailable SSE, file URLs, WAL invalidation, account switching and concurrent account removal. Windows CI runs both the OpenCode wire tests and native startup fixtures.

The optional `opencode_v2_real_cli_reads_connected_account` engine test accepts `ZERON_OPENCODE_TEST_EXE` and `--ignored`; it creates private roots, initializes a fresh database through the real CLI, writes a placeholder OAuth account through Zeron and checks that the real CLI exposes its provider models. It passed against Windows OpenCode 2.0.22. This verifies storage compatibility and discovery, without making an authenticated model request. Existing OpenCode processes can retain cached credentials; Zeron's next run starts a fresh server. Expired/revoked credentials, missing model entitlement, quotas, outgoing TLS/proxy failures and future upstream protocol changes still require a useful error or reconnect rather than an automatic repair.

## Baseline findings

There were several concrete reasons a working model picker could precede an immediate failure. The strongest historical match was fixed in Zeron v0.2.102. The investigated baseline still had a v2 credential-store mismatch, a Windows home-directory mismatch, and a v1 catalog fallback that could advertise thousands of models with no connected provider. Server authentication can also fail independently of model-provider authentication.

## Findings to act on first

| Priority | Finding | Evidence and scope |
| --- | --- | --- |
| 1 | A cached failed version probe used to abort every chat while discovery worked. | Exact symptom documented in Zeron PR #686. Fixed in v0.2.102; verify the affected user's Zeron version, independently of OpenCode's version. |
| 1 | Zeron's OpenCode account controls update the v1 credential file, while v2 uses SQLite credentials. | Reproduced with OpenCode 2.0.22: changing `auth.json` after migration did not change the live account or model catalog, even after restarting the server. Applies across operating systems. |
| 1 | An explicitly empty v1 connected-provider list becomes the full picker catalog. | Reproduced through the actual Zeron harness on Windows: zero connected providers produced 8,363 selectable models. Applies across operating systems. |
| 2 | Windows `HOME` and `USERPROFILE` can point at different credential stores. | Reproduced with native v1: OpenCode read the `USERPROFILE` credential, while Zeron's code would read the `HOME` credential. v2 also created its default storage under `USERPROFILE`. |
| 2 | An inherited custom v1 server username prevents Zeron from authenticating to its own child. | Reproduced: `OPENCODE_SERVER_USERNAME=custom-user` caused HTTP 401 for Zeron's fixed `opencode` username. The tested v2.0.22 binary ignored that override. |

The v2 credential-store issue is particularly relevant to users who sign in or switch accounts inside Zeron. Their UI state can change while the OpenCode process continues using a different, potentially expired or revoked, credential. This is a source-backed consequence of the storage mismatch; an end-to-end real OAuth sign-in was not performed.

## Why detection does not establish readiness

The path is implemented in [the OpenCode harness](../../crates/harness/src/opencode/mod.rs), [the registry](../../crates/engine/src/registry.rs), [account storage](../../crates/engine/src/agent_accounts/stores.rs), and [the model picker](../../crates/ui/src/pickers.rs).

1. **Installation detection** resolves a file from the override, PATH, or known installation directories. It does not establish a usable provider login or complete a model request.
2. **Model discovery** starts a short-lived `opencode serve` without Zeron's MCP injection. v1 reads `/provider`; v2 reads `/api/model` and `/api/agent`. Existing disk and memory catalogs can also supply picker rows while a refresh is pending or unsuccessful.
3. **Sending a message** starts a separate server with the chat's working directory. When Zeron supplies its MCP server, startup additionally probes the executable version and merges `OPENCODE_CONFIG_CONTENT` using the generation-specific schema.
4. **Session setup** creates or resumes a session, reads the project-scoped provider catalog, and, on v2, sets the selected model and agent.
5. **Execution** subscribes to SSE and submits the prompt. Provider credential validation, model entitlement, SDK initialization, attachments, and plugin hooks can fail here even when steps 1 and 2 succeeded.

The picker therefore answers “what this server advertises,” with caching and filtering. It does not answer “has this selected model successfully authenticated and generated a response.” A dummy API key made Anthropic appear connected in the live v1 catalog and enabled its models after v2 migration; no request to Anthropic was needed to obtain those rows.

One implementation detail also matters: the comment on `server(None, None)` says discovery boots in the user's home, but `Server::spawn` only sets `current_dir` when a directory is supplied. Discovery actually inherits Zeron's process working directory. The model-list RPC does not carry the chat's directory. Project-specific config, agents, plugins, aliases, and disabled providers can therefore differ between discovery and the first chat request.

## The historical immediate failure

Commit `27480d99`, merged as [PR #686](https://github.com/zeronsh/zeron/pull/686) on October 1, fixes three related problems:

- Chat startup needed `opencode --version` to choose the MCP config format. The shared version probe had a two-second timeout and cached its failure for the executable's unchanged metadata. A slow first execution after installation or upgrade could poison later chat starts until Zeron restarted. Discovery did not require that successful MCP version probe.
- OpenCode v2 preferred `OPENCODE_PASSWORD` over `OPENCODE_SERVER_PASSWORD`. Zeron previously set only the latter, so an inherited password could lock it out of its own server.
- Failures before a stream started were only published to the live journal. The durable transcript could show a generic failed run without the underlying reason.

Current code retries the version probe with a 30-second budget, clears the failed cache on success, and starts without the injected MCP block if the version still cannot be determined. It owns both password variables and persists startup errors in the transcript.

`git tag --contains 27480d99` returned `v0.2.102`, the only containing release tag in this checkout. A user saying they updated OpenCode has not established that they received this Zeron fix. Conversely, the old cached-version failure should not be blamed for an incident established to be on v0.2.102 or a later build containing this commit.

The earlier reproduction against OpenCode 2.0.20 is documented in that commit. On this machine, ordinary version probes of the downloaded 1.18.34 and 2.0.22 binaries took roughly 0.4–0.5 seconds and 0.1 seconds respectively; this investigation did not reproduce a Defender-induced cold timeout.

## Current credential incompatibility with OpenCode v2

Zeron's `default_opencode_auth_file`, `keyed_file`, `write_keyed_entry`, account adoption, activation, and removal operate on `$XDG_DATA_HOME/opencode/auth.json`, defaulting to `~/.local/share/opencode/auth.json`. The supported account rows are ChatGPT (`openai`) and GitHub Copilot (`github-copilot`). The code does not branch on OpenCode generation for those writes.

OpenCode v2 reads credentials from a database service. Its [credential service](https://github.com/anomalyco/opencode/blob/beta/packages/core/src/credential.ts) manages stored credentials and activation, and its [legacy import migration](https://github.com/anomalyco/opencode/blob/beta/packages/core/src/database/migration/20260805200742_import_legacy_credentials.ts) imports the old file during database migration. That migration is not an ongoing synchronization mechanism.

The native Windows experiments used dummy keys and isolated data directories:

| Experiment | Result on 2.0.22 |
| --- | --- |
| Initialize an empty v2 database, then add an Anthropic entry to legacy `auth.json` and restart. | Only 10 anonymous OpenCode models appeared; `/api/integration` reported no Anthropic connections. |
| Place an Anthropic entry in `auth.json` before the first boot of an empty v2 database. | Again, no Anthropic connection. Fresh schema bootstrap marks migrations completed instead of executing the legacy import. |
| Initialize a v1 database with the same dummy Anthropic entry, then boot v2 over it. | Migration imported a credential. The catalog contained 10 OpenCode models and 19 Anthropic models; Anthropic had one stored connection. |
| Replace that legacy file with an OpenAI entry, removing Anthropic, then restart v2. | The same stored Anthropic connection and the same 29-model catalog remained. The file change did not change the live v2 credentials. |

The fresh-database result is consistent with [the database bootstrap implementation](https://github.com/anomalyco/opencode/blob/beta/packages/core/src/database/migration.ts). The existing-v1 upgrade experiment separately establishes that a successful initial import still does not make later legacy-file writes effective.

Consequences to verify in affected users:

- A new login added through Zeron may never reach a fresh v2 installation.
- Switching or re-authenticating in Zeron may leave v2 using an older credential.
- A login performed through OpenCode v2 may exist in its database without appearing in Zeron's legacy-file account list.
- Removing the legacy-file entry does not remove an already imported v2 credential.
- Zeron's model-context fingerprint hashes `auth.json`, but not v2's credential database. Native v2 account changes can therefore leave a cached picker catalog looking current.

A generation-aware native account integration is needed. Prefer OpenCode's credential/integration APIs and its own authentication flows over writing its database directly. Updating cache invalidation alone cannot make the existing account writes work.

## The v1 model picker can advertise disconnected models

`models_from_providers` converts `connected` into a set and filters providers only when that set is nonempty. This treats both a missing field and `"connected": []` as permission to return everything in `all`. The existing unit test `missing_connected_list_falls_back_to_the_full_catalog` explicitly asserts that behavior for both cases.

For a real native v1 reproduction, the isolated configuration contained `{"disabled_providers":["opencode"]}` and no credentials. Disabling the anonymous provider left `/provider` with 225 catalog providers and an empty connected list. Running the existing `opencode_models_probe` through Zeron returned **8,363 models** with exit code zero.

That is a complete reproduction of false readiness in the picker. The catalog contains providers for which the user has no key. The first request can fail immediately with a model or provider error. Compatibility with servers that omit `connected` should be considered separately from a server explicitly reporting that none are connected.

There is a second, more general limit even when `connected` is nonempty: provider connection can mean a credential or configured provider exists. It does not establish key validity, OAuth freshness, billing, subscription eligibility, or access to every advertised model. An authenticated provider may still reject a particular model.

## Windows credential path mismatch

[The harness home resolver](../../crates/harness/src/executable.rs) and [the engine home resolver](../../crates/engine/src/repos.rs) prefer `HOME`, falling back to `USERPROFILE`. On Windows, OpenCode's default XDG roots use the Windows home directory; the upstream global implementations use `os.homedir()` through their XDG helper.

The reproduction set `HOME` and `USERPROFILE` to different temporary directories and left XDG overrides unset. Each directory had a different dummy credential file: OpenAI under `HOME`, Anthropic under `USERPROFILE`. Native OpenCode 1.18.34 listed only Anthropic. Zeron's current source would select the OpenAI file. Native v2 also created its default config, cache, state, and log directories under the isolated `USERPROFILE` tree.

This condition can arise in shell, Git Bash, development, or redirected-profile environments; its prevalence among the affected users is unknown. When the two variables agree, this mechanism does not apply. An absolute `XDG_DATA_HOME` shared by both processes can align the data root, but existing credentials must be kept at that chosen location. Zeron should resolve OpenCode's home and default storage using the same Windows rules as the CLI.

## Server authentication is separate from provider authentication

Zeron sends HTTP Basic authentication as `opencode:<per-run-password>`. Current startup sets both password names, but leaves `OPENCODE_SERVER_USERNAME` inherited.

On the real v1.18.34 binary, setting the inherited username to `custom-user` caused `/global/health` to return **401** with Zeron's username. Sending the same password with `custom-user` returned **200**. This is a connection to Zeron's own localhost child; re-authenticating ChatGPT or Copilot does not repair it. [Upstream v1 server authentication](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/server/auth.ts) reads that username override.

The tested v2.0.22 binary continued to accept `opencode` with the same override. [Its current beta authentication service](https://github.com/anomalyco/opencode/blob/beta/packages/server/src/auth.ts) fixes the username to `opencode`. Treat the reproduced username problem as v1-specific unless another version demonstrates it. It would normally break live discovery as well as a chat; a cached or previously loaded picker can make the chat failure appear to be the first failure.

## Other mechanisms and their distinguishing evidence

These are supported by execution paths or vendor documentation, but were not established as the cause of an affected user's incident.

| Mechanism | Why the first message can fail | Distinguishing evidence |
| --- | --- | --- |
| Expired, revoked, or invalid provider credentials | A provider appears connected before a real request validates the credential. OAuth refresh may fail; a stale migrated v2 credential increases this risk. | Provider HTTP 401/403, `provider.auth`, refresh errors, or a different active native account from the Zeron account row. |
| Model access, billing, quota, or service changes | A catalog row does not establish plan eligibility, API billing, credits, regional access, or that the model is still served. | Provider response body and status; failure follows the provider/model across projects. Some transient failures retry instead of failing immediately. |
| Stale picker or saved model selection | Disk catalogs return quickly while refresh continues; recoverable refresh errors can retain old rows. v2 credential state is not in the current fingerprint. | Model absent from the live project catalog; a refresh or selecting a currently advertised model changes the result. |
| Project-specific configuration or agent | Discovery inherits the app directory; a chat boots in its project. The project can change providers, model aliases, plugins, agents, or endpoint settings. | One project fails while a clean directory works. Inspect both the global and project config and the selected Agent option. |
| Inline config and MCP merging | Chat startup, unlike ordinary discovery, merges `OPENCODE_CONFIG_CONTENT`. Invalid JSON/object shape or a non-object `mcp` block can fail inside Zeron before the child starts. | Errors such as `OPENCODE_CONFIG_CONTENT.mcp must be an object`. Native v2 accepted `{"mcp":false}` in a direct startup probe, while Zeron's merge explicitly rejects it when injecting MCP. |
| Plugin or provider package initialization | A plugin hook, incompatible provider package, stale cache, dependency download, or invalid custom provider can fail at project boot or prompt execution. | OpenCode logs, plugin name/stack, `ProviderInitError`, package/import errors, or success with the same model in an isolated configuration. [Vendor troubleshooting](https://opencode.ai/docs/troubleshooting/) describes these failure classes. |
| Wrong executable or multiple installations | Resolution considers multiple candidates and can choose the highest successfully probed version rather than the first PATH entry. v1 and v2 coexist. A GUI also retains its launch-time environment. | Compare Zeron's logged binary path/version with `Get-Command opencode -All` and `where.exe opencode`; check `OPENCODE_EXECUTABLE`. Updating the terminal's chosen binary may update a different installation. |
| Invalid override or missing launch dependencies | The OpenCode-specific resolver accepts any existing override file and does not actually call the native-override validator mentioned in its comment. A broken shim, missing package payload, invalid image, or blocked executable can still look installed. | Native launch error, shim output, or `--version` failing at the exact resolved path. Current npm packages advertise native `.exe` entry points; a missing Node runtime is chiefly relevant to older JavaScript wrappers. |
| Native installation directory absent from fallback search | Windows fallbacks cover npm, pnpm, Node, Volta, Scoop, Bun, and Unix-style per-user bins, but do not include `LOCALAPPDATA/Programs/OpenCodeCLI`. | A native CLI exists there while the GUI's PATH lacks it. The investigation machine had a running executable at that location. This is a detection gap, not proof that the reports came from this path. |
| Provider proxy or TLS setup | Zeron's loopback client disables proxies, but the child still needs its own provider network configuration. A successful Zeron usage probe uses a different HTTP client. | TLS/certificate errors, connection failures, proxy authentication, or success only from a terminal with the needed environment. [OpenCode's network documentation](https://opencode.ai/docs/network/) documents proxy variables and custom CAs. |
| Working-directory access | Deleted projects, offline shares, filesystem permissions, OneDrive placeholders, long paths, or a directory visible only in WSL can prevent startup or session work. | Native directory/access error; failure follows the folder. A WSL login and installation are separate from the Windows native CLI and its credentials. |
| Database migration, locking, or mixed generations | v1 and v2 can use the same data root. A newer schema, failed migration, corrupt database, lock contention, or unavailable storage can fail session creation after health succeeds. | `POST /session` or `/api/session` 500, SQLite errors, or migration logs. v1 session creation already retries one 5xx to recover the documented lazy migration failure; persistent failures still surface. |
| Localhost or process restrictions | Port reservation has a bind/release window. Security software, application policy, or process creation/job restrictions can block execution or communication. | Native OS error or startup exit rather than a provider response. Port/health timeouts are generally a weaker match for an immediate failure. |
| Upstream stream or API regression | A CLI version can change routes, strict schemas, agent/command bodies, or SSE events. Integration behavior can differ from the standalone TUI. | Version-specific failing HTTP route or event; same binary/provider works in its own CLI but fails through the server API. Current real 1.18.34 and 2.0.22 startup/session/error flows worked in this investigation. |
| Attachment URI conversion | Both wires construct `file://{path}` without proper file-URL conversion. Plain drive paths are tolerated, but `#` is treated as a fragment, `%23` is decoded, and an unnormalized UNC path can fail. | Failure only with an attachment or path containing these characters. Use proper platform-aware file-URL conversion; the current attachment tests use Unix paths. |
| Selected command or skill | Native command selection can become invalid after project discovery changes. v1 commands also reject attachments explicitly. | Error mentions a selected command no longer being available, a command route, or command attachments. A plain text first message excludes this branch. |
| No stream progress | The server may accept a prompt but fail to publish usable session events. Zeron fails after the configured stall bound or reconnect budget. | A roughly 60-second default stall, retries, or a disconnected event bus. This is a lower-priority explanation for genuinely immediate failures. |

The attachment behavior was checked with Node's Windows file-URL conversion. `file://C:\Users\test\image.png` round-tripped correctly, so a drive letter alone is not evidence of this bug. `shot #1.png` resolved to `shot `, `shot %23.png` resolved to `shot #.png`, and a raw `\\server\share\image.png` URI failed absolute-path validation. OpenCode v2's [attachment loader](https://github.com/anomalyco/opencode/blob/beta/packages/core/src/session/prompt.ts) uses `new URL` followed by `fileURLToPath`, matching this distinction.

An [upstream Windows report on 1.18.15](https://github.com/anomalyco/opencode/issues/41436) describes streams hanging as a normal user and working elevated. That supports investigating Windows runtime/security differences for stalls; it does not establish this as the cause here or show that elevation is the right fix. OpenCode Desktop's WebView, file-dialog, and built-in terminal bugs are also not direct explanations for Zeron's headless `serve` path.

## Validation performed

Downloaded the official Windows x64 platform packages from npm and verified each archive against its published SHA-512 integrity value. The binaries were unpacked into a temporary directory instead of installed globally. Credential experiments used explicitly isolated home/XDG roots and placeholder values. No real OAuth sign-in or paid model request was performed, and existing credential files were not modified.

| Check | Result |
| --- | --- |
| `cargo test --locked -p zeron-harness --test opencode` | 17 passed. |
| `cargo test --locked -p zeron-harness --lib opencode` | 63 matching unit tests passed, including v2 HTTP/SSE fixtures, discovery, model selection, errors, and loopback proxy behavior. |
| `cargo test --locked -p zeron-harness --features native-fixture --test windows_native` | 16 passed, including batch shims, Unicode/metacharacters, environment, working directory, cancellation, and process-tree cleanup. |
| `cargo test --locked -p zeron-harness --lib executable::tests` | 12 passed. |
| Native v1/v2 health and session creation | Passed with a directory containing spaces, an apostrophe, Japanese characters, `&`, and `!`. |
| Actual harness discovery against native v2.0.22 | Returned 10 anonymous models with Build/Plan agent choices after the catalog settled. |
| Actual harness first prompt with deliberately nonexistent provider/model | Both generations emitted a visible model error and settled as `Errored`, with no text. This exercised local error handling without calling a model provider. |
| v1 username override, split home directories, empty connected catalog, v2 legacy credential updates | Reproduced as described above. |

The initial v2 model endpoint was empty immediately after health became ready; a short settle poll returned its 10 anonymous models. The harness already handles this startup race. Basic Windows paths, native process creation, both current wire generations, and immediate model-error handling are consequently lower-priority suspects than the reproduced readiness and credential problems.

These tests do not establish successful authenticated generation for ChatGPT, Copilot, or every third-party provider. The three subprocess tests covering MCP injection, unknown versions, and cold version recovery in `mcp_injection_tests` are guarded with `cfg(unix)` and did not run on Windows. Windows CI runs the library and native process suites, but its current harness command does not include the `opencode` integration-test target or real OpenCode binaries. The suite can pass while a Windows-specific first-chat startup regression remains undetected.

## Diagnostics that will distinguish the next report

Request the following small set of facts from an affected user:

1. The exact error text, including whether it appears before any assistant/session activity and approximately how long after Send.
2. Zeron version, OpenCode version **at the path Zeron actually launched**, and that executable path.
3. Selected `provider/model`, Agent option, and whether the provider is ChatGPT OAuth, Copilot OAuth, an API key, or anonymous OpenCode.
4. Whether the same model works in native OpenCode in the same project, and whether Zeron works in a clean directory.
5. Whether `HOME` differs from `USERPROFILE`, whether the XDG roots are overridden, and whether `OPENCODE_AUTH_CONTENT`, `OPENCODE_CONFIG_CONTENT`, either password variable, or the username variable is present. Record presence and relevant paths, not secret values.
6. The Zeron startup error and relevant OpenCode log excerpt from the same timestamp. Default Windows logs are under `%USERPROFILE%\.local\share\opencode\log`; XDG overrides change that location. Redact secrets before sharing excerpts.

Zeron already logs the discovery binary path/version, but the chat's server-ready message is debug-level, spawn failures can be generic native I/O errors, stdout is discarded, and the stderr tail holds only six short lines. Add a concise startup record that connects chat ID, exact binary/version, protocol, working directory, credential backend, config source, and failure stage. Preserve a safe provider error code/status even when upstream supplies an empty message. A bare `provider.auth` should tell the user which provider to repair and which account store OpenCode is actually using.

## Recommended changes

1. Make v2 account listing, login adoption, activation, removal, and cache invalidation use the live v2 credential backend. Keep the file-based path for v1.
2. Separate an omitted v1 `connected` field from an explicitly empty list. Do not advertise the full catalog when the server says no providers are connected.
3. Align OpenCode credential/config roots with Windows home-directory behavior and normalize the owned server username on v1. Explain overridden auth/config sources in diagnostics.
4. Make discovery context explicit and project-aware, and validate the saved provider/model and selected agent against the live project catalog before submitting. Surface which state is cached without claiming it proves authentication.
5. Add Windows subprocess coverage for cold version/MCP startup and real native v1/v2 smoke tests using a local mock provider. Include sign-in-store changes and attachment paths with `#`, `%`, and UNC forms. Keep real provider compatibility checks separate from synthetic protocol tests.
6. Improve startup failure records and executable override validation, add the applicable native installation directory to Windows fallback search, and convert attachments with platform-aware file URLs.

No production behavior was changed during this investigation. The incident-specific root cause remains unknown until one affected user's versions, exact error, and failure stage are captured.
