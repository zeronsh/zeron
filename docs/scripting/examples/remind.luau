zeron.tool {
    name = "remind_me",
    description = "Show the user a reminder after a delay. Use it when the user asks to be reminded of something.",
    input = {
        text = { type = "string", description = "What the reminder says" },
        seconds = { type = "number", description = "How long to wait", optional = true },
    },
    run = function(args)
        zeron.after(args.seconds or 60, function()
            zeron.notify(args.text)
        end)
        return "Reminder set for " .. (args.seconds or 60) .. " seconds from now"
    end,
}
