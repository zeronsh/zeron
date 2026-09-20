import { SESSION_HASH_HEADER, SESSION_ROOM_HEADER, SESSION_STORE_HEADER } from "./browser-sessions";
import { AUTH_USER_HEADER, ROOM_KIND_HEADER } from "./env";

/** Copy caller headers while replacing every Worker-controlled trust signal. */
export const forwardedHeaders = (
  request: Request,
  userId: string,
  roomKind?: "workspace"
): Headers => {
  const headers = new Headers(request.headers);
  headers.delete(ROOM_KIND_HEADER);
  headers.delete(SESSION_HASH_HEADER);
  headers.delete(SESSION_ROOM_HEADER);
  headers.delete(SESSION_STORE_HEADER);
  headers.set(AUTH_USER_HEADER, userId);
  if (roomKind) headers.set(ROOM_KIND_HEADER, roomKind);
  return headers;
};
