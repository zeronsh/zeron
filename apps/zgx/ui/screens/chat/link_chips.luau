-- What a chat's reply draws a link as (ds/markdown `link_chip`), rather
-- than as a link: one of its agent's skills (a link to its SKILL.md,
-- wherever the agent keeps it: `…/skills/<name>/SKILL.md`, the skill in the
-- chat's list), or a file of its folder. Nil: a link.
--
--   transcript { link_chip = LinkChips(function() return store.session:peek() end) }

local FileLinks = require("@app/screens/chat/file_links")
local Invocations = require("@app/screens/chat/invocations")

return function(session: () -> any?): (target: string) -> { file: boolean?, skill: string? }?
	return function(target: string): { file: boolean?, skill: string? }?
		local s = session()
		local place: Invocations.Target? = if s and s.id then { chat = s.id, device = s.device } else nil
		local skill = Invocations.skill_of(target, if s then s.harness else nil, place)
		if skill then
			return { skill = skill }
		end
		if FileLinks.chip(target, if s then s.cwd else nil) then
			return { file = true }
		end
		return nil
	end
end
