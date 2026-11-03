/**
 * What of a user's goes from BLOBS with their account: their own photo and
 * every image they put (`PUT /auth/me/images`). Both live at `avatars/{key}`
 * (auth-routes.ts). Images put since the directory began tracking them are in
 * `user_images`. Older ones are found by their `owner` metadata in a bounded
 * scan of `avatars/`.
 */
import type { Env } from "./env";

/** Listing pages read looking for untracked images (1000 objects a page). */
const LEGACY_SCAN_PAGES = 20;

/** Keys of `avatars/` objects marked as `userId`'s images, found by scanning. */
const untrackedImages = async (env: Pick<Env, "BLOBS">, userId: string): Promise<string[]> => {
  const found: string[] = [];
  let cursor: string | undefined;
  for (let page = 0; page < LEGACY_SCAN_PAGES; page++) {
    const listed = await env.BLOBS.list({ prefix: "avatars/", cursor, include: ["customMetadata"] });
    for (const object of listed.objects) {
      if (object.customMetadata?.owner === userId && object.customMetadata?.kind === "image") found.push(object.key);
    }
    if (!listed.truncated) break;
    cursor = listed.cursor;
  }
  return found;
};

/** Delete a departing user's photo and images. Failures are logged without
 * the user's id and left: an orphaned image is unreachable, not exposed. */
export const deleteUserBlobs = async (
  env: Pick<Env, "BLOBS">,
  userId: string,
  photoKey: string | null,
  imageKeys: readonly string[]
): Promise<void> => {
  try {
    const keys = new Set([...(photoKey ? [photoKey] : []), ...imageKeys].map((key) => `avatars/${key}`));
    for (const key of await untrackedImages(env, userId)) keys.add(key);
    const all = [...keys];
    for (let i = 0; i < all.length; i += 1000) await env.BLOBS.delete(all.slice(i, i + 1000));
  } catch (e) {
    console.warn("account deletion: removing images failed", e instanceof Error ? e.name : "error");
  }
};
