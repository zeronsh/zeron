-- Load as plugin `flaky` beside any app (ScriptHost::set_plugins, or a
-- plugins/ folder). It declares a command, then fails: it's set aside,
-- yet `commands()` still has `flaky/flake`, and `run_command` on it is
-- false (its runner's VM is gone).
ui.command {
	id = "flake",
	title = "Flake",
	menu = "Test",
	key = "cmd+alt+f",
	run = function()
		print("ran")
	end,
}
error("broken on purpose")
