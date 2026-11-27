local id = "prompt-jump"
local messages = require("./long_chat_fixture").chat(150)
local prompt = 0
for _, message in messages do
	if message.role == "user" then
		prompt += 1
		message.text = `#{prompt} · {message.text}`
		message.copy = message.text
	end
end

local now = os.time()
ui.shared("fixture.chats", {} :: { [string]: any }):set({
	[id] = {
		session = {
			id = id,
			state = "ready",
			title = "Prompt Jump",
			project = "atlas",
			device = "mac",
			agent = "claude",
			harness = "claude-code",
			model = "opus",
			created = now,
			updated = now,
		},
		messages = messages,
	},
})
ui.shared("fixture.initial_chat", ""):set(id)
return require("./shell_fixture")
