package sh.zeron.android.core

import java.net.URI

/**
 * Where the updater looks for a release and its APK, in order.
 *
 * GitHub itself comes first, but from mainland China github.com,
 * api.github.com and the asset CDN it redirects to
 * (release-assets.githubusercontent.com) are often slow, reset, or reachable
 * only through a proxy the phone may not be using (Tailscale and a proxy app
 * can't both hold Android's single VPN slot). So every step can fall back to
 * public GitHub proxies that take the full GitHub URL as a path
 * (`https://ghfast.top/https://github.com/...`). They serve the same bytes;
 * the download is checked against the release's SHA-256 (when GitHub's API
 * gave one) and the APK's signing certificate before it is installed.
 */
object UpdateSources {
    /** Public "prefix" proxies that handle release downloads with Range (checked 2026-09). */
    val BUILT_IN_MIRRORS = listOf("https://ghfast.top/", "https://gh-proxy.com/", "https://gh.llkk.cc/")

    /** Key remembered for GitHub itself (see [downloadSources]'s `preferred`). */
    const val GITHUB = "github"

    data class Source(
        /** Short name shown to the user: "GitHub" or the mirror's host. */
        val label: String,
        val url: String,
        /** Mirror prefix, or [GITHUB]; remembered after a successful download. */
        val key: String,
        /** GitHub token for the API asset endpoint; never sent past the first hop. */
        val token: String? = null,
    )

    /** `ghfast.top` -> `https://ghfast.top/`; blank -> null. */
    fun normalizeMirror(raw: String?): String? {
        var m = raw?.trim().orEmpty()
        if (m.isEmpty()) return null
        if (!m.startsWith("http://") && !m.startsWith("https://")) m = "https://$m"
        if (!m.endsWith("/")) m += "/"
        return m
    }

    /** User mirror first, then the built-in ones, without duplicates. */
    fun mirrors(userMirror: String?, builtIns: List<String> = BUILT_IN_MIRRORS): List<String> =
        (listOfNotNull(normalizeMirror(userMirror)) + builtIns).distinct()

    fun label(mirror: String): String = runCatching { URI(mirror).host }.getOrNull() ?: mirror

    /**
     * Download order. A user-set mirror goes first (they set it for a
     * reason), then GitHub (through the token-authenticated API endpoint when
     * a token is saved), then the built-in mirrors. The source that worked
     * last time is moved to the front.
     */
    fun downloadSources(
        githubUrl: String,
        userMirror: String?,
        preferred: String?,
        token: String? = null,
        apiAssetUrl: String? = null,
        builtIns: List<String> = BUILT_IN_MIRRORS,
    ): List<Source> {
        val user = normalizeMirror(userMirror)
        val github = if (!token.isNullOrBlank() && !apiAssetUrl.isNullOrBlank()) {
            Source("GitHub", apiAssetUrl, GITHUB, token)
        } else {
            Source("GitHub", githubUrl, GITHUB)
        }
        val list = buildList {
            if (user != null) add(Source(label(user), user + githubUrl, user))
            add(github)
            builtIns.forEach { add(Source(label(it), it + githubUrl, it)) }
        }.distinctBy { it.key }
        val first = list.firstOrNull { it.key == preferred } ?: return list
        return listOf(first) + (list - first)
    }

    /**
     * Every source for the "switch mirror" picker, in a fixed order: GitHub,
     * the built-in mirrors, then the user's own mirror (when it isn't one of them).
     */
    fun choices(
        githubUrl: String,
        userMirror: String?,
        token: String? = null,
        apiAssetUrl: String? = null,
        builtIns: List<String> = BUILT_IN_MIRRORS,
    ): List<Source> {
        val all = downloadSources(githubUrl, userMirror, null, token, apiAssetUrl, builtIns)
        val user = normalizeMirror(userMirror)
        return all.filter { it.key != user } + all.filter { it.key == user }
    }

    /**
     * Tag out of a `releases/latest` redirect. Handles absolute Locations and
     * mirrors that rewrite them to a relative `/https://github.com/.../releases/tag/x`.
     */
    fun tagFromLocation(location: String?): String? {
        val tail = location?.substringAfter("/releases/tag/", "")?.substringBefore('?')?.substringBefore('#')?.trimEnd('/')
        if (tail.isNullOrBlank()) return null
        return java.net.URLDecoder.decode(tail, "UTF-8")
    }

    /** GitHub's asset `digest` ("sha256:<hex>") -> lowercase hex, or null. */
    fun sha256FromDigest(digest: String?): String? {
        val hex = digest?.trim()?.takeIf { it.startsWith("sha256:", ignoreCase = true) }?.substringAfter(':')?.lowercase()
        return hex?.takeIf { it.length == 64 && it.all { c -> c in '0'..'9' || c in 'a'..'f' } }
    }
}
