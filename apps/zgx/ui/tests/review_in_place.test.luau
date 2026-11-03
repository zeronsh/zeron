-- A comment opened under a line of a pull request's diff opens in place:
-- the diff and its lines stay the same nodes, the comment box under the
-- line, focused (the pane scrolling only as far as brings it into view);
-- made, it stands there as a thread in the same diff. (The diff was
-- built again for every comment opened or made: it faded out and in, and
-- the box, opening in a diff being made again, scrolled it a third of a
-- pane past the box.) On the review example in ui/review_fixture.luau, at
-- the window's width.

-- A node's identity and top, by the text or label on its line: `@id`, y.
local function node(app: any, scope: string, label: string): (string?, number?)
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(label, 1, true) then
			return line:match("@(%d+)"), tonumber(line:match(" %-?%d+,(%-?%d+) %d+x%d+"))
		end
	end
	return nil, nil
end

local function settle(app: any)
	app:frames(30)
end

test("a comment opens and lands in place in the diff", function()
	local app = mount("review_fixture.luau")
	app:resize(1300, 900)
	settle(app)
	app:set_state("ex_gr_tab", "code")
	settle(app)
	local content = node(app, "#diff-content", "#diff-content")
	local line = node(app, "#diff-content", '"    local text = props[1]"')
	expect(content).never.to_be_nil()

	-- A comment opened on line 44.
	app:set_state("ex_gr_composing", "ui/ds/badge.luau|n44")
	settle(app)
	local now_content, top = node(app, "#diff-content", "#diff-content")
	expect(`diff @{now_content}`).to_be(`diff @{content}`)
	local now_line = node(app, "#diff-content", '"    local text = props[1]"')
	expect(`line @{now_line}`).to_be(`line @{line}`)
	expect(app:tree("#diff-content"):find('textbox "Comment" ="" [^\n]* focused') ~= nil).to_be(true)
	-- Just into view: its buttons inside the pane (225 to 645), not past
	-- the pane's middle.
	local _, cancel = node(app, "#diff-content", 'button "Cancel"')
	expect((cancel :: number) > 435 and (cancel :: number) + 28 <= 645).to_be(true)

	-- Written and sent: a thread in its place, the diff where it was.
	app:fill('role:textbox "Comment"', "Should this follow the theme?")
	app:click('role:button "Comment"')
	settle(app)
	expect(app:has('#diff-content >> ~"Should this follow the theme?"')).to_be(true)
	-- (Sending by its button scrolls the pane to its top, as it did before:
	-- something the press does, not the diff; made by state, it keeps it.)
	local after_content = node(app, "#diff-content", "#diff-content")
	expect(`diff @{after_content}`).to_be(`diff @{content}`)
	expect(`line @{node(app, "#diff-content", '"    local text = props[1]"')}`).to_be(`line @{line}`)
	expect(top).never.to_be_nil()
end)
