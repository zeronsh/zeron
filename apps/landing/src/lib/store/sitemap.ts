// Sitemaps (sitemaps.org): an index of the site's own pages and the store's.

const escape = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");

export function urlset(entries: { loc: string; lastmod?: number }[]): string {
  const urls = entries.map((e) => `<url><loc>${escape(e.loc)}</loc>${e.lastmod ? `<lastmod>${new Date(e.lastmod).toISOString()}</lastmod>` : ""}</url>`).join("");
  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${urls}</urlset>\n`;
}

export function sitemapIndex(locs: string[]): string {
  const maps = locs.map((loc) => `<sitemap><loc>${escape(loc)}</loc></sitemap>`).join("");
  return `<?xml version="1.0" encoding="UTF-8"?>\n<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${maps}</sitemapindex>\n`;
}

export const XML = { "content-type": "application/xml; charset=utf-8" };
