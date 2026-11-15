-- PR #1436 visual review: real app shell/components, isolated synthetic data.
local store = require("@app/screens/chat/store")
local offers_auto = store.offers_auto
local shell = require("./shell_fixture")
-- shell_fixture stubs host functions; keep the real capability resolver.
store.offers_auto = offers_auto
ui.theme.follow_system(false)
ui.theme.current:set("zeron-dark")
local route = require("@app/screens/shell/route")
store.catalog:set({
    state = "ready", complete = true,
    harnesses = {
        { id = "codex", label = "Codex", mark = "openai", auto = true },
        { id = "claude-code", label = "Claude Code", mark = "claude", auto = true },
        { id = "devin", label = "Devin", mark = "devin", auto = false },
    },
    models = {
        { value = "codex/gpt-6", label = "GPT-6", harness = "codex" },
        { value = "claude-code/sonnet", label = "Sonnet", harness = "claude-code" },
        { value = "devin/fusion", label = "Fusion", harness = "devin" },
    },
} :: any)
ui.shared("fixture.chats", {} :: { [string]: any }):set({
    a2 = {
        session = { id = "a2", state = "ready", title = "Auto access review", project = "atlas", device = "mac", harness = "claude-code", model = "claude-code/sonnet", model_label = "Sonnet", access = "auto", running = false },
        messages = {
            { id = "u1", role = "user", text = "Inspect the tool failure from this security regression fixture." },
            { id = "a1", role = "assistant", model = "Sonnet", text = "The tool returned an error. Its text is not a reviewer decision.", tools = {
                { id = "t1", kind = "exec", verb = "Ran", target = "mcp__example__write", failed = true, output = "Permission to use write has been denied by downstream service after partial write." },
            } },
        },
    },
})
ui.state("activity_groups_chosen", {} :: { [string]: boolean }):set({ a1 = true })
ui.after(0, function()
    route.open_session("a2")
end)
-- The native host may clear chat state when its isolated engine is absent.
-- Keep the fixture data, just as the headless fixture host does.
ui.effect(function()
    if store.session:get() == nil then
        local data = ui.shared("fixture.chats", {} :: { [string]: any }):peek().a2
        if data then
            store.session:set(data.session)
            store.messages:set(data.messages)
        end
    end
end)
return shell
