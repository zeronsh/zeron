package sh.zeron.android.core

import android.content.Context
import sh.zeron.android.BuildConfig
import uniffi.zeron_core.ClientEvent
import uniffi.zeron_core.ClientListener
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreConfig
import uniffi.zeron_core.Credentials
import uniffi.zeron_core.DemoFixture
import uniffi.zeron_core.DemoOptions
import uniffi.zeron_core.StreamSpeed
import uniffi.zeron_core.TranscriptScale
import uniffi.zeron_core.authProductionEdgeUrl
import java.io.File
import java.util.UUID

/**
 * How the app opens the Rust core for a workspace: `demo`, `cloud`, or a
 * saved machine's id (direct SSH). Shared by [ZeronModel] and the scheduled
 * send service, so both connect exactly the same way (same data directory,
 * device id and credentials).
 */
object CoreConnect {
    const val DEMO = "demo"
    const val CLOUD = "cloud"

    /** The client the app has open right now, so background work can reuse it. */
    class Live(val workspace: String, val client: CoreClient)

    @Volatile
    var live: Live? = null

    fun dataDirName(workspace: String): String = when (workspace) {
        DEMO -> "demo"
        CLOUD -> "core"
        else -> "direct-$workspace"
    }

    fun config(context: Context, dir: File): CoreConfig {
        val prefs = context.getSharedPreferences("zeron", 0)
        val id = prefs.getString("deviceId", null) ?: ("android-" + UUID.randomUUID().toString().take(8)).also {
            prefs.edit().putString("deviceId", it).apply()
        }
        return CoreConfig(
            edgeUrl = authProductionEdgeUrl(),
            dataDir = dir.absolutePath,
            deviceId = id,
            deviceName = android.os.Build.MODEL ?: "Android",
            platform = "android",
            appVersion = BuildConfig.VERSION_NAME ?: "0.2.94",
        )
    }

    fun demoCredentials() = Credentials.Demo(
        DemoOptions(
            fixture = DemoFixture.STANDARD,
            transcriptScale = TranscriptScale.Normal,
            streamSpeed = StreamSpeed.REALISTIC,
            longReply = false,
        ),
    )

    /** The signed-in Zeron Cloud account, if any. */
    fun storedCredentials(context: Context): Credentials? {
        val raw = context.getSharedPreferences("zeron", 0).getString("account", null) ?: return null
        val parts = raw.split('\n')
        if (parts.size < 4) return null
        return Credentials.WorkOs(parts[0], parts[1], uniffi.zeron_core.AuthTokens(parts[2], parts[3]))
    }

    class Unavailable(message: String) : Exception(message)

    /** Credentials for [workspace]; throws [Unavailable] when it no longer exists. */
    fun credentials(context: Context, workspace: String): Credentials = when (workspace) {
        DEMO -> demoCredentials()
        CLOUD -> storedCredentials(context) ?: throw Unavailable(AppLanguage.string(context, sh.zeron.android.R.string.not_signed_in_cloud))
        else -> {
            val store = MachineStore(context)
            val machine = store.list().firstOrNull { it.id == workspace } ?: throw Unavailable(AppLanguage.string(context, sh.zeron.android.R.string.computer_removed))
            Credentials.Direct(store.target(machine, route = store.plan(machine, NetworkWatcher.current(context))))
        }
    }

    /** A new client for [workspace] (blocking; call off the main thread). */
    fun open(context: Context, workspace: String, events: (ClientEvent) -> Unit = {}): CoreClient {
        val dir = File(context.filesDir, dataDirName(workspace)).apply { mkdirs() }
        val client = CoreClient(config(context, dir), credentials(context, workspace), object : ClientListener {
            override fun onEvent(event: ClientEvent) = events(event)
        })
        client.preloadSessions()
        return client
    }
}
