local on = zeron.store.get("on", false)

zeron.on("before_run", function(run)
    if on then
        run.prompt = run.prompt .. "\n\nKeep the answer under five sentences."
    end
end)

zeron.command("Toggle brief answers", function()
    on = not on
    zeron.store.set("on", on)
    zeron.notify(on and "Brief answers on" or "Brief answers off")
end)
