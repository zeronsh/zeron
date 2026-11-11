-- The composers screens/chat/shared counts for the shell's ⌘/ (tests/
-- chat_actions.test.luau): two overlapping as a page that swaps its composer
-- has them, the new one mounted before the old one's cleanup, and a
-- template's composer from before the count, which sets true or false.
local shared = require("@app/screens/chat/shared")

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

ui.command {
	id = "overlap",
	title = "A composer swapped in before the old one's cleanup",
	run = function()
		shared.composer_shown:set(0)
		note(`none:{tostring(shared.open_model_picker())}`)
		shared.composer_mounted() -- the first composer
		shared.composer_mounted() -- its replacement
		shared.composer_gone() -- the first one's cleanup, last
		note(`swapped:{tostring(shared.open_model_picker())}`)
		shared.composer_gone()
		note(`gone:{tostring(shared.open_model_picker())}`)
		shared.composer_gone()
		note(`floor:{tostring(shared.composer_shown:peek())}`)
	end,
}

ui.command {
	id = "legacy",
	title = "A template's composer that sets true or false",
	run = function()
		shared.composer_shown:set(true)
		note(`true:{tostring(shared.open_model_picker())}`)
		shared.composer_mounted()
		shared.composer_gone()
		note(`counted:{tostring(shared.open_model_picker())}`)
		shared.composer_shown:set(false)
		note(`false:{tostring(shared.open_model_picker())}`)
	end,
}

return ui.text { "composers", literal = true }
