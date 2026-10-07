package sh.zeron.android.ui

import android.content.Context
import org.json.JSONObject

/**
 * Where the last New Session went, per computer (iOS keeps the whole draft in
 * `AppModel.lastDraft`). The sheet opens on it instead of always on the first
 * project in the list.
 */
internal object NewSessionMemory {
    /** A project, or (projectless) the computer it ran on. */
    data class Picks(val projectId: String?, val hostId: String?)

    private const val PREFS = "zeron-new-session"

    fun load(context: Context, machine: String): Picks? {
        val raw = context.getSharedPreferences(PREFS, 0).getString("last:$machine", null) ?: return null
        return runCatching {
            val o = JSONObject(raw)
            Picks(o.optString("projectId").ifEmpty { null }, o.optString("hostId").ifEmpty { null })
        }.getOrNull()
    }

    fun save(context: Context, machine: String, picks: Picks) {
        if (picks.projectId == null && picks.hostId == null) return
        val o = JSONObject()
        picks.projectId?.let { o.put("projectId", it) }
        picks.hostId?.let { o.put("hostId", it) }
        context.getSharedPreferences(PREFS, 0).edit().putString("last:$machine", o.toString()).apply()
    }

    /**
     * What the sheet opens on: the project it was opened from, else the
     * last pick on this computer if it still exists (a projectless pick
     * reopens projectless on its host), else the most recently used project.
     */
    fun <P> initial(explicit: String?, remembered: Picks?, projects: List<P>, hosts: List<String>, id: (P) -> String, lastUsedMs: (P) -> Long): Picks {
        if (explicit != null) return Picks(explicit, null)
        remembered?.projectId?.takeIf { pid -> projects.any { id(it) == pid } }?.let { return Picks(it, null) }
        if (remembered != null && remembered.projectId == null && remembered.hostId in hosts) return Picks(null, remembered.hostId)
        return Picks(projects.maxByOrNull(lastUsedMs)?.let(id), null)
    }
}
