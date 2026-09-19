Native GPUI screenshots from the isolated sidebar fixture with a synthetic account.

```sh
ZERON_SIDEBAR_COMPACT=1 ZERON_SIDEBAR_ACCOUNT=1 cargo run -p zeron-ui --example sidebar-fixture --features project-palette-fixture
```

The sidebar footer uses a circular 21px avatar button with a 13px circle matching the remote icon and a centered
monospace initial. It shows no account name or Alpha label. The account menu
opens to the right, clamped within the window, and retains its email and actions.

Native X11 checks verified that clicking footer whitespace does not open the
menu and clicking the avatar opens and closes it. Screenshots show the resting
button, hover state, and open menu. All 12 account-related UI tests passed,
as did the native fixture build, formatting, and diff checks.
