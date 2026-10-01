package sh.zeron.android.tools

import java.net.URLDecoder

/**
 * What a tapped transcript link or file badge means: a web page (the in-app
 * browser, localhost dev servers included), a file in the session's folder
 * (the viewer — PDFs, images, code, HTML), a path outside it, or something
 * for another app (mailto:, …). Paths may be absolute on the device, relative
 * to the chat's folder, `file://` URLs, or carry editor suffixes (`:12:4`,
 * `#L12`).
 */
object Links {
    sealed interface Target {
        data class Web(val url: String) : Target
        /** Workspace-relative. */
        data class File(val path: String) : Target
        data class Outside(val path: String) : Target
        data object Other : Target
    }

    private val scheme = Regex("^([a-zA-Z][a-zA-Z0-9+.-]*):")
    private val lineSuffix = Regex("(:\\d+){1,2}$")

    fun classify(link: String, ref: WorkspaceRef?): Target {
        val text = link.trim()
        val lower = text.lowercase()
        if (lower.startsWith("http://") || lower.startsWith("https://")) return Target.Web(text)
        val raw = when {
            lower.startsWith("file://") -> decode(text.substring(7).removePrefix("localhost"))
            scheme.containsMatchIn(text) && !Regex("^[a-zA-Z]:[\\\\/]").containsMatchIn(text) -> return Target.Other
            else -> decode(text)
        }
        val path = raw.substringBefore('#').substringBefore('?').replace(lineSuffix, "").trim()
        if (path.isEmpty() || ref == null) return Target.Other
        return resolve(path, ref.root)
    }

    /** A path against the workspace root → workspace-relative, or outside. */
    fun resolve(path: String, root: String?): Target {
        if (path.startsWith("/")) {
            val base = root?.trimEnd('/') ?: return Target.Outside(path)
            val normalized = normalize(path) ?: return Target.Outside(path)
            return when {
                normalized == base -> Target.Outside(path)
                normalized.startsWith("$base/") -> Target.File(normalized.removePrefix("$base/"))
                else -> Target.Outside(path)
            }
        }
        if (path.startsWith("~")) return Target.Outside(path)
        val relative = normalize("/" + path)?.removePrefix("/") ?: return Target.Outside(path)
        return if (relative.isEmpty()) Target.Outside(path) else Target.File(relative)
    }

    /** Collapse `.`/`..`; null when `..` climbs above the root. */
    private fun normalize(path: String): String? {
        val out = ArrayList<String>()
        for (part in path.split('/')) {
            when (part) {
                "", "." -> Unit
                ".." -> if (out.isEmpty()) return null else out.removeAt(out.size - 1)
                else -> out += part
            }
        }
        return "/" + out.joinToString("/")
    }

    private fun decode(s: String): String =
        if ('%' in s) runCatching { URLDecoder.decode(s.replace("+", "%2B"), "UTF-8") }.getOrDefault(s) else s
}
