import { useEffect } from "react";
import { useNavigate } from "@tanstack/react-router";
import { onShortcut } from "../state/shortcuts";

/**
 * The new-chat flow (ticket 15): the blank new-thread canvas IS the route —
 * "New-chat mode mints the chat id on first send" (shell.rs:5897-5899), so
 * the titlebar `+` and Mod+N navigate to `/` (the desktop's
 * `open_new_session`) and the composer's send does the minting. The canvas
 * host (routes/chat-page.tsx) resolves the run target from the remembered
 * device/project picks.
 *
 * Headless. The desktop's titlebar is the single owner of the new-session
 * action in both sidebar states (`render_titlebar_cluster`), so this mounts
 * no control of its own — it subscribes to the `new-chat` shortcut event
 * that both the titlebar `+` and the app-shell keyboard layer (Cmd/Ctrl+N)
 * emit.
 */
export function NewChatListener() {
  const navigate = useNavigate();

  useEffect(
    () =>
      onShortcut("new-chat", () => {
        void navigate({ to: "/" });
      }),
    [navigate],
  );

  return null;
}
