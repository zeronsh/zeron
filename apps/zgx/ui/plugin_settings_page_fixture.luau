-- Settings with plugins' pages in it, the host's part left to whoever
-- mounts this: the shell (ui/shell_fixture.luau, as it is) with
-- screens/plugins loaded and nothing played. src/plugins/settings_page_tests
-- mounts it with real plugins loaded (one that draws a settings page of its
-- own, `zeron.settings_page`) and plays the bridge itself, with the real
-- settings book (src/plugins/settings.rs).

local node = require("./shell_fixture")
require("@app/screens/plugins")

return node
