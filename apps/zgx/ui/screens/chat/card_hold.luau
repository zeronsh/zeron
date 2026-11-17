-- A chat that's a thread of a paused card (account/orchestration's
-- `paused_card`): what's sent to it is held in its queue until the card is
-- resumed. Its composer's queue says so in one line, with Resume (into
-- Now). Read from the board the app already holds, never asked for.
--
--   local held, resume = CardHold.bind(function() return chat end, on_failed)
--   Composer { held = held, on_held_action = resume, … }

local orchestration = require("@account/orchestration")

local M = {}

function M.bind(chat: () -> string?, on_failed: (() -> ())?): (any, () -> ())
	local card = ui.computed(function(): any
		return orchestration.paused_card(chat())
	end)
	local held = ui.computed(function(): any
		if card:get() == nil then
			return nil
		end
		return { text = ui.t("chat.composer.held_paused"):get(), action = ui.t("chat.composer.resume"):get() }
	end)
	local function resume()
		local it = card:peek()
		if not it then
			return
		end
		orchestration.resume_item(it.id):after(function() end, function()
			if on_failed then
				on_failed()
			end
		end)
	end
	return held, resume
end

return M
