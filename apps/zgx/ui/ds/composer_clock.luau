-- The composer's snap/settle deadlines use the same clock in its inner
-- geometry and its page placement. Production keeps ui.now; the headless
-- motion fixture replaces this function before either composer is built.
return { now = ui.now }
