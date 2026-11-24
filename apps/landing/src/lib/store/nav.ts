// The store's categories, in the app's words, each with the app's icon.
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
  { id: "agents", label: store("plugins.store.category.agents"), href: "/plugins?category=agents", icon: "bot" },
  { id: "appearance", label: store("plugins.store.category.appearance"), href: "/plugins?category=appearance", icon: "palette" },
  { id: "chat", label: store("plugins.store.category.chat"), href: "/plugins?category=chat", icon: "chat-round-line" },
  { id: "developer-tools", label: store("plugins.store.category.developer_tools"), href: "/plugins?category=developer-tools", icon: "terminal" },
  { id: "git", label: store("plugins.store.category.git"), href: "/plugins?category=git", icon: "git-branch" },
  { id: "integrations", label: store("plugins.store.category.integrations"), href: "/plugins?category=integrations", icon: "plug-circle" },
  { id: "productivity", label: store("plugins.store.category.productivity"), href: "/plugins?category=productivity", icon: "checklist" },
  { id: "utilities", label: store("plugins.store.category.utilities"), href: "/plugins?category=utilities", icon: "tuning" },
];
