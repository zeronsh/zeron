// The upstream browser entry still imports node:zlib eagerly. Do not silently
// fake compression: these commands are excluded from the mobile command set.
export const constants = {};
export function gunzipSync() { throw new Error("gzip is unavailable in the mobile spike"); }
export function gzipSync() { throw new Error("gzip is unavailable in the mobile spike"); }
