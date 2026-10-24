local risky = { "rm %-rf", "git push %-%-force", "git reset %-%-hard", "DROP TABLE" }
local stats = zeron.store.get("stats", { turns = 0, commands = 0, flagged = 0 })
local guard = zeron.store.get("guard", true)
local commands_this_turn = {}

local function save()
    zeron.store.set("stats", stats)
end

local function plural(n, word)
    return n == 1 and ("1 " .. word) or (n .. " " .. word .. "s")
end

zeron.on("before_run", function(run)
    if run.prompt:lower():find("refactor") then
        run.reasoning = "high"
    end
    if run.cwd:find("zeron") then
        run.prompt = run.prompt .. "\n\nEnd with one line naming the files you touched."
    end
    commands_this_turn[run.chat] = 0
end)

zeron.on("tool_call", function(call)
    if call.kind ~= "exec" then
        return
    end
    commands_this_turn[call.chat] = (commands_this_turn[call.chat] or 0) + 1
    stats.commands += 1
    if not guard then
        return
    end
    for _, pattern in risky do
        if call.command:find(pattern) then
            stats.flagged += 1
            zeron.notify("Risky command: " .. call.command:sub(1, 60), "error")
            break
        end
    end
end)

zeron.on("turn_done", function(turn)
    stats.turns += 1
    save()
    local ran = commands_this_turn[turn.chat] or 0
    commands_this_turn[turn.chat] = nil
    local took = string.format("%.0fs", turn.seconds)
    if turn.status == "completed" then
        zeron.notify("Done in " .. took .. ", " .. plural(ran, "command"), "success")
    else
        zeron.notify("Turn " .. turn.status .. " after " .. took, "error")
    end
end)

zeron.command("Session stats", function()
    zeron.notify(plural(stats.turns, "turn") .. ", " .. plural(stats.commands, "command") .. ", " .. stats.flagged .. " flagged")
end)

zeron.command("Toggle command guard", function()
    guard = not guard
    zeron.store.set("guard", guard)
    zeron.notify(guard and "Command guard on" or "Command guard off")
end)

zeron.tool {
    name = "session_stats",
    description = "How many turns and shell commands this Zeron has run, and how many commands the guard flagged.",
    run = function()
        return stats
    end,
}
