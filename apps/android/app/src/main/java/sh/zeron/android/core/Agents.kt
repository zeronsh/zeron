package sh.zeron.android.core

import org.json.JSONArray
import org.json.JSONObject

/**
 * The engine's harness-install and agent-account surface (Settings → Coding
 * agents), spoken over the untyped `host_call` passthrough. Reply shapes are
 * the engine's camelCase JSON (`HarnessDescriptor`, `AgentAccountsSnapshot`,
 * `AgentLoginStart`, `AgentLoginPoll`, `HarnessUpdateStatus`); parsing is
 * lenient — unknown fields are ignored, missing ones default the way the
 * engine's serde defaults do.
 */
object Agents {
    const val LIST_HARNESSES = "ListHarnesses"
    const val INSTALL_HARNESS = "InstallHarness"
    const val CANCEL_INSTALL = "CancelInstall"
    const val UNINSTALL_HARNESS = "UninstallHarness"
    const val CHECK_UPDATES = "CheckHarnessUpdates"
    const val LIST_UPDATES = "ListHarnessUpdates"
    const val APPLY_UPDATE = "ApplyHarnessUpdate"
    const val APPLY_ALL_UPDATES = "ApplyAllHarnessUpdates"
    const val LIST_ACCOUNTS = "ListAgentAccounts"
    const val START_LOGIN = "StartAgentLogin"
    const val POLL_LOGIN = "PollAgentLogin"
    const val COMPLETE_LOGIN = "CompleteAgentLogin"
    const val CANCEL_LOGIN = "CancelAgentLogin"
    const val FORGET_ACCOUNT = "ForgetAgentAccount"

    /** Harnesses that are test rigs, never shown to people. */
    private val hidden = setOf("mock")

    data class Harness(
        val id: String,
        val name: String,
        /** The CLI is present on the device. Old engines omit it: true. */
        val installed: Boolean,
        val canInstall: Boolean,
        val enabled: Boolean?,
    )

    data class Account(
        val id: String,
        val harness: String,
        val email: String?,
        val displayName: String?,
        val plan: String?,
        val active: Boolean,
        val provider: String? = null,
        val usageWindows: List<UsageWindow> = emptyList(),
        val usageError: String? = null,
    ) {
        val title: String get() = email ?: displayName ?: "Signed in"
    }

    data class UsageWindow(val label: String, val usedFraction: Float, val resetsAt: String?)

    data class Accounts(val accounts: List<Account>, val warnings: Map<String, String>) {
        fun forHarness(id: String) = accounts.filter { it.harness == id }
    }

    enum class LoginMode { PasteCode, Browser }

    data class LoginStart(val loginId: String, val url: String, val mode: LoginMode)

    sealed interface LoginPoll {
        data class Pending(val url: String?) : LoginPoll
        data object Done : LoginPoll
        data class Failed(val message: String) : LoginPoll
    }

    /**
     * One `HarnessUpdateStatus`. [phase] is the engine's kebab-case state
     * machine (`current`, `available`, `waiting-for-idle`, `installing`,
     * `updated`, `manual-action-required`, `failed`, …).
     */
    data class Version(
        val harness: String,
        val installed: String?,
        val latest: String?,
        val phase: String = "dormant",
        /** The engine can apply it; otherwise [manualCommand] says how. */
        val canApply: Boolean = false,
        val manualCommand: String? = null,
        val error: String? = null,
        val progress: String? = null,
    ) {
        /** A newer release the device can install itself. */
        val updatable: Boolean get() = phase == "available" && canApply
        val available: Boolean get() = phase == "available"
        /** The engine is mutating the CLI right now. */
        val busy: Boolean get() = phase in busyPhases
    }

    private val busyPhases = setOf("waiting-for-idle", "preparing", "downloading", "installing", "verifying")

    /** `UninstallHarness`: what went (or, for a dry run, would go), and the fresh catalog. */
    data class Uninstall(
        val removed: List<String>,
        /** Another copy outside Zeron that is still installed, with how to remove it. */
        val remaining: String?,
        val harnesses: List<Harness>?,
    )

    /** `ApplyAllHarnessUpdates`: harness ids updated/failed/manual, and the final statuses. */
    data class UpdateAll(
        val updated: List<String>,
        val failed: Map<String, String>,
        val manual: List<String>,
        val statuses: Map<String, Version>,
    )

    fun harnesses(json: Any?): List<Harness> {
        val arr = json as? JSONArray ?: return emptyList()
        return (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val id = o.optString("id").ifEmpty { return@mapNotNull null }
            if (id in hidden) return@mapNotNull null
            Harness(
                id = id,
                name = o.optString("name").ifEmpty { id },
                installed = if (o.has("installed")) o.optBoolean("installed") else true,
                canInstall = o.optBoolean("canInstall", false),
                enabled = if (o.has("enabled") && !o.isNull("enabled")) o.optBoolean("enabled") else null,
            )
        }
    }

    fun accounts(json: Any?): Accounts {
        val o = json as? JSONObject ?: return Accounts(emptyList(), emptyMap())
        val list = o.optJSONArray("accounts") ?: JSONArray()
        val accounts = (0 until list.length()).mapNotNull { i ->
            val a = list.optJSONObject(i) ?: return@mapNotNull null
            Account(
                id = a.optString("id").ifEmpty { return@mapNotNull null },
                harness = a.optString("harness"),
                email = a.str("email"),
                displayName = a.str("displayName"),
                plan = a.str("planLabel"),
                active = a.optBoolean("active", false),
                provider = a.str("provider"),
                usageWindows = usageWindows(a.optJSONArray("usageWindows")),
                usageError = a.str("usageError"),
            )
        }
        val warnings = o.optJSONArray("warnings") ?: JSONArray()
        val byHarness = (0 until warnings.length()).mapNotNull { i ->
            val w = warnings.optJSONObject(i) ?: return@mapNotNull null
            w.optString("harness") to w.optString("message")
        }.toMap()
        return Accounts(accounts, byHarness)
    }

    private fun usageWindows(windows: JSONArray?): List<UsageWindow> {
        if (windows == null) return emptyList()
        return (0 until windows.length()).mapNotNull { i ->
            val window = windows.optJSONObject(i) ?: return@mapNotNull null
            val fraction = window.optDouble("usedFraction", Double.NaN)
            if (!fraction.isFinite()) return@mapNotNull null
            UsageWindow(
                label = window.str("label") ?: "Usage",
                usedFraction = fraction.coerceIn(0.0, 1.0).toFloat(),
                resetsAt = window.str("resetsAt"),
            )
        }
    }

    fun loginStart(json: Any?): LoginStart? {
        val o = json as? JSONObject ?: return null
        val id = o.optString("loginId").ifEmpty { return null }
        val mode = if (o.optString("mode") == "paste-code") LoginMode.PasteCode else LoginMode.Browser
        return LoginStart(id, o.optString("url"), mode)
    }

    fun loginPoll(json: Any?): LoginPoll {
        val o = json as? JSONObject ?: return LoginPoll.Failed("No answer from the device.")
        return when (o.optString("status")) {
            "done" -> LoginPoll.Done
            "error" -> LoginPoll.Failed(o.str("message") ?: "Sign-in failed.")
            else -> LoginPoll.Pending(o.str("url"))
        }
    }

    fun versions(json: Any?): Map<String, Version> {
        val arr = json as? JSONArray ?: return emptyMap()
        return (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val h = o.optString("harness").ifEmpty { return@mapNotNull null }
            h to Version(
                harness = h,
                installed = o.str("installedVersion"),
                latest = o.str("latestVersion"),
                phase = o.optString("phase").ifEmpty { "dormant" },
                canApply = o.optBoolean("canApply", false),
                manualCommand = o.str("manualCommand"),
                error = o.optJSONObject("error")?.str("message"),
                progress = o.optJSONObject("progress")?.str("message"),
            )
        }.toMap()
    }

    fun uninstall(json: Any?): Uninstall {
        val o = json as? JSONObject ?: return Uninstall(emptyList(), null, null)
        val removed = o.optJSONArray("removed") ?: JSONArray()
        return Uninstall(
            removed = (0 until removed.length()).map { removed.optString(it) },
            remaining = o.str("remaining"),
            harnesses = o.optJSONArray("harnesses")?.let { harnesses(it) },
        )
    }

    fun updateAll(json: Any?): UpdateAll {
        val o = json as? JSONObject ?: return UpdateAll(emptyList(), emptyMap(), emptyList(), emptyMap())
        fun ids(key: String): List<JSONObject> {
            val arr = o.optJSONArray(key) ?: return emptyList()
            return (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }
        }
        val manual = o.optJSONArray("manual") ?: JSONArray()
        return UpdateAll(
            updated = ids("updated").map { it.optString("harness") },
            failed = ids("failed").associate { it.optString("harness") to (it.str("error") ?: "Update failed") },
            manual = (0 until manual.length()).map { manual.optString(it) },
            statuses = versions(o.optJSONArray("statuses")),
        )
    }

    /** A host_call reply as org.json (object, array, or the raw text). */
    fun parse(raw: String): Any = when (raw.trimStart().firstOrNull()) {
        '[' -> JSONArray(raw)
        '{' -> JSONObject(raw)
        else -> raw
    }

    /**
     * Relay calls can give up before a minutes-long install does; a timed-out
     * InstallHarness keeps going on the device, so the screen watches the
     * catalog instead of reporting a failure.
     */
    fun isTimeout(message: String?): Boolean = message?.contains("timed out", ignoreCase = true) == true

    private fun JSONObject.str(key: String): String? = if (isNull(key)) null else optString(key).ifEmpty { null }
}
