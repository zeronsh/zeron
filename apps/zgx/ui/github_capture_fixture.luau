-- The shell with GitHub's sidebar badges (plugins-builtin/github), for
-- comparing them with Zeron's: ui/shell_fixture.luau as it is, and the host's
-- part played here, its sessions' pull requests open, merged and closed
-- (the numbers and states the shipping app's sidebar fixture shows).
-- Run: ZERON_ZGX_DEV_PLUGINS=plugins-builtin/github zeron-zgx github_capture_fixture.luau --inspect

local node = require("./shell_fixture")

ui.after(0, function()
	ui.shared("plugin.github.github", {} :: any):set({
		a1 = { number = 413, title = "Fix the build", url = "https://github.com/zeronsh/zeron/pull/412", state = "open" },
		a2 = { number = 414, title = "Write the docs", url = "https://github.com/zeronsh/zeron/pull/412", state = "merged" },
		b1 = { number = 416, title = "Ship the release", url = "https://github.com/zeronsh/zeron/pull/412", state = "closed" },
	})
end)

return node
