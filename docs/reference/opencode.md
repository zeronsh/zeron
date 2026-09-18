# OpenCode connection

Zeron can run OpenCode through its managed local process or an existing OpenCode v2 server. On desktop, open **Settings → Agents → OpenCode connection** and select the execution device in the page header. On iOS, open **OpenCode connection** from Home's menu and choose the execution device in the sheet. These settings live on that device's engine; desktop and iOS are views of the same connection.

For an existing server, choose **Existing server** and enter its full HTTP or HTTPS URL on the selected execution device, using `localhost` or a loopback IP such as `127.0.0.1` or `[::1]`. Include the server's port; a path prefix is supported for a local proxy. OpenCode v2's shared service commonly uses port `49374` and username `opencode`; use the values from your own server. The service setup guide describes `opencode pair` for obtaining its generated credentials. A foreground server may use a different port. Enter its password, then **Test connection** and **Save**. Test uses the draft values without saving. A blank password field keeps the saved password; **Clear saved password** marks it for removal when you save. Credentials are kept on the execution device and are never shown by the settings readback.

Choose **Managed local** and save to return to Zeron's default behavior. Zeron then starts its own OpenCode process when needed. An existing server connection requires no OpenCode CLI on the device running the desktop or iOS app.

The execution engine and OpenCode server must run on the same device so both can read the selected project's absolute directory and attachment paths. Remote OpenCode servers are not supported yet. The desktop or iOS client can still control that execution device remotely.

After connecting, open the model picker to see the models enabled for that project, including custom providers. The agent picker offers the server's visible primary agents. If discovery fails, use its Retry or Refresh action and check the connection settings. Connection changes are rejected while the engine retains an OpenCode runtime, even when it is idle. Stop active sessions first; if only an idle runtime remains, restart the execution engine to release it, then save again.

When an OpenCode command changes the session's agent, model, or thinking level, Zeron updates the chat's selectors and uses those settings for subsequent messages. OpenCode 2.0.9 reports these through `session.agent.selected` and `session.model.selected`; the model's `variant` carries the thinking level. Returning to the server's default variant clears the previous explicit thinking level.

The v2 API and example service values above come from [OpenCode's published documentation](https://opencode.ai/v2/docs/api) and [service setup guide](https://opencode.ai/v2/docs/cli/web). Use your server's actual address and credentials.

An isolated OpenCode **2.0.7** server API check verified `/api/info`, Basic authentication on HTTP and SSE, directory-scoped model and agent lists, agent selection on session creation and switch, and a catalog that grew after its first response. This was an API check; no provider-backed prompt or desktop/iOS client smoke session was completed. The macOS and iOS native build gates remain pending.
