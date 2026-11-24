// The names of the store's icons (Icons.astro).
export const ICONS = ["timer", "pr", "gauge", "check", "palette", "cal", "cloud", "zap", "book", "key", "db", "msg", "bug", "bell", "grid", "globe", "term", "search", "arrow", "chev", "out", "down", "flag", "plus", "copy", "seal", "x", "left", "menu", "clock", "file", "net", "dots", "laptop", "user"] as const;
export type IconName = (typeof ICONS)[number];
