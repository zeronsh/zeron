/**
 * A store item's kind (contract "Templates in the store"): a plugin or a
 * template, fixed when its id is claimed. One namespace of ids holds both,
 * and the routes of each are its own prefix.
 */
import { STORE_DATA } from "./data";

export type Kind = "plugin" | "template";
export const KINDS: readonly Kind[] = STORE_DATA.kinds as readonly Kind[];

/** The prefix of `kind`'s routes under `/store/v1/`, and its cache keys'. */
export const segmentOf = (kind: Kind): "plugins" | "templates" => (kind === "plugin" ? "plugins" : "templates");

/** The kind a route prefix is of, if it's one. */
export const kindOfSegment = (segment: string | undefined): Kind | undefined =>
  segment === "plugins" ? "plugin" : segment === "templates" ? "template" : undefined;

/** `?kind=` of a store-wide route: plugin unless it says template; undefined
 * when it says anything else. */
export const kindParam = (p: URLSearchParams): Kind | undefined => {
  const raw = p.get("kind");
  return raw === null ? "plugin" : KINDS.find((k) => k === raw);
};

