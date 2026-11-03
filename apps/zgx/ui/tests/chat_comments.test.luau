-- A review comment left from the files pane (screens/files/comments calls
-- `shared.attach_comment { path, line, body, root }`): a chip naming its
-- file and line, following it when edits carry it, sent as Zeron's review
-- comments block citing the file within the project.

test("a comment from the files pane is a chip at its line, follows a move, and is sent as the review block", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("comment_from_files")
	app:frames(30)
	expect(app:has('~"app.rs:12"')).to_be(true)
	app:command("comment_moves")
	app:frames(30)
	expect(app:has('~"app.rs:14"')).to_be(true)
	expect(app:has('~"app.rs:12"')).to_be(false)
	app:type("role:textbox", "Please fix")
	app:key("Enter")
	app:frames(30)
	expect(app:wait('~"Please fix"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain("send:s2:Please fix\n\nReview comments (each cites the workspace file and line it belongs to):\n- src/app.rs:14: Rename this.")
end)

-- Comments a plugin left on a diff (Git's Changes: `zeron.compose_comment`,
-- handed on by screens/plugins): chips naming their file and lines, told
-- back to the plugin while they're carried, gone from it when taken off,
-- and sent as Zeron's diff comments block (production's
-- COMMENT_BLOCK_HEADER): each its file, its lines, its side, its note, the
-- code under it.
test("a plugin's comments on a diff are chips, told back while carried, and sent with their file, lines, side and code", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("plugin_diff_comments")
	app:frames(30)
	expect(app:has('#chat-composer >> ~"graph.rs:10–12"')).to_be(true)
	expect(app:has('#chat-composer >> ~"old.rs:4"')).to_be(true)
	expect(app:state("test_plugin_comments")).to_be("k1@s2,k2@s2")
	-- Taken off: no longer the plugin's to mark.
	app:hover('#chat-composer >> ~"old.rs:4"')
	app:frames(10)
	app:click('role:button ~"Remove old.rs:4"')
	app:frames(30)
	expect(app:has('#chat-composer >> ~"old.rs:4"')).to_be(false)
	expect(app:state("test_plugin_comments")).to_be("k1@s2")
	app:click('role:textbox "Message"')
	app:key("Enter")
	app:frames(30)
	app:wait("#never-shown", { timeout_ms = 300 })
	app:frames(30)
	expect(app:state("calls")).to_contain(
		"send:s2:Address the review comments below.\n\nComments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):\n- src/graph.rs:10-12 (R): Keep the old name.\n  ```\n   fn lanes() {}\n  -fn rows() {}\n  +fn rows(n: usize) {}\n  ```"
	)
	-- Sent: none left to mark.
	expect(app:state("test_plugin_comments")).to_be("")
	expect(app:error()).to_be(nil)
end)
