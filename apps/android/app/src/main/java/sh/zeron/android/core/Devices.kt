package sh.zeron.android.core

import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.ProjectView

/** Pure device logic shared by the pickers and Settings (JVM-tested). */
object DeviceIdentity {
    /**
     * What a client is built for: the engine's device id, edge and identity.
     * Any change (signing in or out, a custom server) replaces the client.
     */
    fun key(deviceId: String, edgeUrl: String, userId: String, orgId: String): String =
        listOf(deviceId, edgeUrl, userId, orgId).joinToString("|")

    /** A device's glyph: phones and tablets by platform, servers, else a laptop. */
    fun icon(platform: String): Int = when (platform) {
        "android", "ios", "ipados" -> ZIcons.Phone
        "linux" -> ZIcons.Server
        else -> ZIcons.Laptop
    }
}

/** One machine in the new-session picker: its projects, and whether it can run a session without one. */
data class MachineGroup(
    val deviceId: String,
    val name: String,
    val platform: String,
    val online: Boolean,
    val isSelf: Boolean,
    val projects: List<ProjectView>,
)

object Machines {
    /**
     * Projects grouped by the machine they live on, plus every execution host
     * without projects (a session can still run "on the device"), as the
     * desktop's picker lists them: this device first, then online ones, then
     * by name. Viewer-only devices never appear.
     */
    fun groups(projects: List<ProjectView>, hosts: List<DeviceView>): List<MachineGroup> {
        val byDevice = projects.groupBy { it.deviceId }
        val known = hosts.filter { it.isExecutionHost }.map { d ->
            MachineGroup(d.id, d.name, d.platform, d.online || d.isSelf, d.isSelf, byDevice[d.id].orEmpty())
        }
        // Projects on a device the registry hasn't described yet still show.
        val orphans = byDevice.filterKeys { id -> known.none { it.deviceId == id } }.map { (id, list) ->
            val first = list.first()
            MachineGroup(id, first.deviceName ?: "Device", "", first.deviceOnline, false, list)
        }
        return (known + orphans).sortedWith(
            compareByDescending<MachineGroup> { it.isSelf }
                .thenByDescending { it.online }
                .thenBy { it.name.lowercase() },
        )
    }
}
