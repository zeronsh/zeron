// What each action asks before it's done, said once for its dialog and for
// the page that asks without a script, in the app's words (words.ts).
import type { FlashWord } from "./flash.ts";
import type { VersionStatus } from "./types.ts";
import { store } from "./words.ts";

export interface Ask {
  title: string;
  words: string[];
  confirm: string;
  tone: "primary" | "danger";
  flash: FlashWord;
}

/** What an author can do to a version (amendment 45: withdraw goes; nothing waits for review). */
export type AuthorAction = "yank" | "unyank";

/** An author's action on one version: the app's words for it, its question, and the state it starts from. */
const authorAsk = (action: AuthorAction, tone: Ask["tone"], flash: FlashWord) => (name: string, version: string): Ask => ({
  title: store(`plugins.store.confirm.${action}.title`, { name, version }),
  words: [store(`plugins.store.confirm.${action}.body`)],
  confirm: store(`plugins.store.action.${action}`),
  tone,
  flash,
});

export const AUTHOR_ACTIONS: Record<AuthorAction, { from: VersionStatus; ask: (name: string, version: string) => Ask }> = {
  yank: { from: "listed", ask: authorAsk("yank", "danger", "yanked") },
  unyank: { from: "yanked", ask: authorAsk("unyank", "primary", "unyanked") },
};

export const isAuthorAction = (value: unknown): value is AuthorAction => typeof value === "string" && Object.hasOwn(AUTHOR_ACTIONS, value);

export const revokeAsk = (name: string): Ask => ({
  title: store("plugins.store.key.revoke_title", { name }),
  words: [store("plugins.store.key.revoke_body")],
  confirm: store("plugins.store.key.revoke"),
  tone: "danger",
  flash: "key_revoked",
});
