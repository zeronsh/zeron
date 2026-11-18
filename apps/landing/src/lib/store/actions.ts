// What each action asks before it's done, said once for its dialog and for
// the page that asks without a script.
import type { FlashWord } from "./flash.ts";
import type { VersionStatus } from "./types.ts";

export interface Ask {
  title: string;
  words: string[];
  confirm: string;
  tone: "primary" | "danger";
  flash: FlashWord;
}

export type AuthorAction = "withdraw" | "yank" | "unyank";

export const AUTHOR_ACTIONS: Record<AuthorAction, { from: VersionStatus; ask: (version: string) => Ask }> = {
  withdraw: {
    from: "pending",
    ask: (v) => ({
      title: `Withdraw ${v} from review?`,
      words: ["It leaves the review queue and won't be listed. A version's number stays with what you submitted, so to try again, publish a new version."],
      confirm: "Withdraw",
      tone: "danger",
      flash: "withdrawn",
    }),
  },
  yank: {
    from: "approved",
    ask: (v) => ({
      title: `Stop offering ${v}?`,
      words: ["Nobody new will get this version, and devices won't update to it. Anyone who has it keeps it. You can offer it again later."],
      confirm: "Stop offering",
      tone: "danger",
      flash: "yanked",
    }),
  },
  unyank: {
    from: "yanked",
    ask: (v) => ({
      title: `Offer ${v} again?`,
      words: ["People can install it again, and devices can update to it."],
      confirm: "Offer again",
      tone: "primary",
      flash: "unyanked",
    }),
  },
};

export const isAuthorAction = (value: unknown): value is AuthorAction => typeof value === "string" && Object.hasOwn(AUTHOR_ACTIONS, value);

export const revokeAsk = (name: string): Ask => ({
  title: `Revoke “${name}”?`,
  words: ["Anything using this key can't publish again. Versions it already published stay as they are. This can't be undone: make a new key if you need one."],
  confirm: "Revoke key",
  tone: "danger",
  flash: "key_revoked",
});

/** A deploy key's public half, as the edge names it (base64url). */
export const isKey = (value: unknown): value is string => typeof value === "string" && /^[A-Za-z0-9_-]{16,128}$/.test(value);
