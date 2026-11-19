// The app's list down the side of every store, account and admin page: what
// is in it, in the app's words, each with the app's icon. (layouts/App.astro
// draws it; the staff's group is filled in on public pages by
// scripts/app.ts, which reads the session after the page has loaded.)
import type { GlyphName } from "../../components/ui/Glyph.astro";
import { store } from "./words.ts";

export interface NavItem {
  id: string;
  label: string;
  href: string;
  icon: GlyphName;
}

/** The store's eight categories (amendment 21: the fixed list), as the app names and draws them. */
export const CATEGORIES: readonly NavItem[] = [
  { id: "agents", label: store("plugins.store.category.agents"), href: "/plugins/all?category=agents", icon: "bot" },
  { id: "appearance", label: store("plugins.store.category.appearance"), href: "/plugins/all?category=appearance", icon: "palette" },
  { id: "chat", label: store("plugins.store.category.chat"), href: "/plugins/all?category=chat", icon: "chat-round-line" },
  { id: "developer-tools", label: store("plugins.store.category.developer_tools"), href: "/plugins/all?category=developer-tools", icon: "terminal" },
  { id: "git", label: store("plugins.store.category.git"), href: "/plugins/all?category=git", icon: "git-branch" },
  { id: "integrations", label: store("plugins.store.category.integrations"), href: "/plugins/all?category=integrations", icon: "plug-circle" },
  { id: "productivity", label: store("plugins.store.category.productivity"), href: "/plugins/all?category=productivity", icon: "checklist" },
  { id: "utilities", label: store("plugins.store.category.utilities"), href: "/plugins/all?category=utilities", icon: "tuning" },
];

export const BROWSE: readonly NavItem[] = [
  { id: "discover", label: "Discover", href: "/plugins", icon: "star" },
  { id: "all", label: "All plugins", href: "/plugins/all", icon: "widget" },
];

export const PUBLISH: readonly NavItem[] = [
  { id: "mine", label: "Your plugins", href: "/account", icon: "folder-with-files" },
  { id: "tokens", label: "Deploy tokens", href: "/account/tokens", icon: "lock-keyhole-unlocked" },
  { id: "guide", label: "Publishing guide", href: "/plugins/publishing", icon: "document" },
];

export const REVIEW: readonly NavItem[] = [
  { id: "queue", label: "Queue", href: "/admin/plugins", icon: "inbox" },
  { id: "reports", label: "Reports", href: "/admin/reports", icon: "flag" },
  { id: "reserved", label: "Reserved ids", href: "/admin/reservations", icon: "tag" },
  { id: "audit", label: "Audit log", href: "/admin/audit", icon: "clock-circle" },
];

