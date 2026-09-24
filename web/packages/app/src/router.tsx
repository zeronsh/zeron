import { createRootRoute, createRoute, createRouter, redirect } from "@tanstack/react-router";
import { RootLayout } from "./routes/root-layout";
import { AppShell } from "./components/app-shell";
import { ConversationPage } from "./routes/chat-page";
import { PairPage } from "./routes/pair-page";
import { SettingsLayout } from "./components/settings-layout";
import { AccountsSettingsPage } from "./routes/settings-accounts";
import { AppearanceSettingsPage } from "./routes/settings-appearance";
import { DevicesSettingsPage } from "./routes/settings-devices";
import { AgentsSettingsPage } from "./routes/settings-agents";
import { FilesSettingsPage } from "./routes/settings-files";
import { NotificationsSettingsPage } from "./routes/settings-notifications";
import { ShortcutsSettingsPage } from "./routes/settings-shortcuts";
import { ArchivedSettingsPage } from "./routes/settings-archived";

const rootRoute = createRootRoute({ component: RootLayout });
const shellRoute = createRoute({ getParentRoute: () => rootRoute, id: "shell", component: AppShell });
/*
 * BOTH conversation routes render the SAME component reference
 * (`ConversationPage`, ticket 15): TanStack's `Match` memoizes the route
 * element on `route.options.component`, so the shared reference keeps ONE
 * fiber alive across the `/` ↔ `/chat/$chatId` boundary — the composer is
 * one persistent entity, re-anchored by the dock, never remounted. The
 * page re-renders on navigation through its router-state subscription.
 */
const indexRoute = createRoute({ getParentRoute: () => shellRoute, path: "/", component: ConversationPage });
const chatRoute = createRoute({ getParentRoute: () => shellRoute, path: "/chat/$chatId", component: ConversationPage });
// Changes and Files have no routes: they are right-pane surfaces on the
// desktop, and a route for either took the chat off `/chat/$chatId`, which is
// the only path that owns a pane — the column, its tabs and its toggle all
// disappeared. `rightPaneStore.show(chatId, "changes" | "files")` opens them.
const pairRoute = createRoute({ getParentRoute: () => rootRoute, path: "/pair", component: PairPage });

const settingsRoute = createRoute({ getParentRoute: () => shellRoute, path: "/settings", component: SettingsLayout });
const settingsIndexRoute = createRoute({
  getParentRoute: () => settingsRoute,
  path: "/",
  beforeLoad: () => {
    // The desktop's `OpenSettings`/user-menu always lands on Devices
    // (SettingsSection::ALL's first row, shell.rs:3268/5294/7792).
    throw redirect({ to: "/settings/devices" });
  },
});
const accountsRoute = createRoute({
  getParentRoute: () => settingsRoute,
  path: "/accounts",
  component: AccountsSettingsPage,
});
const appearanceRoute = createRoute({
  getParentRoute: () => settingsRoute,
  path: "/appearance",
  component: AppearanceSettingsPage,
});
// Ticket 29's sections — the harnesses path keeps its route segment (the
// desktop enum variant) while the nav and page carry the "Agents" label.
const devicesRoute = createRoute({ getParentRoute: () => settingsRoute, path: "/devices", component: DevicesSettingsPage });
const harnessesRoute = createRoute({ getParentRoute: () => settingsRoute, path: "/harnesses", component: AgentsSettingsPage });
const filesRoute = createRoute({ getParentRoute: () => settingsRoute, path: "/files", component: FilesSettingsPage });
const notificationsRoute = createRoute({
  getParentRoute: () => settingsRoute,
  path: "/notifications",
  component: NotificationsSettingsPage,
});
const shortcutsRoute = createRoute({ getParentRoute: () => settingsRoute, path: "/shortcuts", component: ShortcutsSettingsPage });
const archivedRoute = createRoute({ getParentRoute: () => settingsRoute, path: "/archived", component: ArchivedSettingsPage });

const routeTree = rootRoute.addChildren([
  pairRoute,
  shellRoute.addChildren([
    indexRoute,
    chatRoute,
    settingsRoute.addChildren([
      settingsIndexRoute,
      accountsRoute,
      appearanceRoute,
      devicesRoute,
      harnessesRoute,
      filesRoute,
      notificationsRoute,
      shortcutsRoute,
      archivedRoute,
    ]),
  ]),
]);

export {
  accountsRoute,
  appearanceRoute,
  archivedRoute,
  chatRoute,
  devicesRoute,
  filesRoute,
  harnessesRoute,
  indexRoute,
  notificationsRoute,
  pairRoute,
  rootRoute,
  settingsRoute,
  shellRoute,
  shortcutsRoute,
};

export const router = createRouter({ routeTree, defaultPreload: "intent" });

declare module "@tanstack/react-router" {
  interface Register {
    readonly router: typeof router;
  }
}
