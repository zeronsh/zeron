// Types for pages.mjs, for the edge worker's typecheck.
export interface PublicBadge { readonly id: string; readonly name: string; readonly icon: string | null }
export interface PublicCardLike {
  readonly username: string;
  readonly firstName: string | null;
  readonly userNumber: number;
  readonly joinedOn: string;
  readonly badges: readonly PublicBadge[];
  readonly scene: number;
  readonly holo: string;
  readonly sky: number;
}
export interface PageUrls {
  page: string; player: string; image: string; json: string;
  script: string; page_script: string; style: string; manifest: string;
}
export const PUBLIC_KEYS: readonly string[];
export const PLAYER: { readonly width: number; readonly height: number };
export const IMAGE: { readonly width: number; readonly height: number };
export function escapeHtml(s: unknown): string;
export function publicCard(card: PublicCardLike): PublicCardLike;
export function urlsFor(card: { readonly username: string }, site: string, base?: string): PageUrls;
export function nameOf(card: PublicCardLike): string;
export function renderShare(card: PublicCardLike, urls: PageUrls, options?: { site?: string }): string;
export function renderPlayer(card: PublicCardLike, urls: PageUrls): string;
export function renderEmbedTest(members: readonly PublicCardLike[], urls: PageUrls): string;
export function renderIndex(members: readonly PublicCardLike[], urls: PageUrls): string;
