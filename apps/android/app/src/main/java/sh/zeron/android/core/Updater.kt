package sh.zeron.android.core

import sh.zeron.android.R
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import sh.zeron.android.BuildConfig
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/**
 * In-app updates from GitHub Releases (villatothesea/zeron-android-app).
 *
 * Release contract (see scripts/android/release.md): tag `roundN`, one APK
 * asset named `zeron-android-roundN.apk`, and a `versionCode: NNN` line in the
 * release notes. Every release is signed with the same key, so the system
 * installer accepts it as an update.
 */
class Updater(
    private val context: Context,
    /** Overridable for tests (a local server); production uses GitHub. */
    private val api: String = API,
    private val web: String = WEB,
    private val builtInMirrors: List<String> = UpdateSources.BUILT_IN_MIRRORS,
    private val signatureCheck: ((File) -> Signature)? = null,
) {
    data class Release(
        val tag: String,
        val name: String,
        val notes: String,
        val versionCode: Long,
        val assetName: String,
        val assetApiUrl: String,
        val downloadUrl: String,
        val size: Long,
        val htmlUrl: String,
        /** Asset SHA-256 from the API's `digest`; null via the web fallback. */
        val sha256: String?,
    ) {
        val newer: Boolean get() = versionCode > BuildConfig.VERSION_CODE.toLong()
    }

    class UpdateError(message: String) : IOException(message)

    private fun str(id: Int, vararg args: Any): String = AppLanguage.string(context, id, *args)

    private val prefs = context.getSharedPreferences("zeron-update", 0)
    private val secrets = SecretStore(context)

    var token: String?
        get() = secrets.get("github-token")
        set(value) = secrets.put("github-token", value?.trim())

    /** Optional prefix for mainland mirrors, e.g. `https://ghfast.top/`. */
    var mirror: String?
        get() = prefs.getString("mirror", null)?.ifBlank { null }
        set(value) = prefs.edit().putString("mirror", value?.trim()).apply()

    var lastCheckMs: Long
        get() = prefs.getLong("lastCheck", 0)
        set(value) = prefs.edit().putLong("lastCheck", value).apply()

    /**
     * When the last automatic check started, successful or not. Failed checks
     * don't move [lastCheckMs], so without this an offline phone would retry
     * on every return to the foreground.
     */
    var lastAttemptMs: Long
        get() = prefs.getLong("lastAttempt", 0)
        set(value) = prefs.edit().putLong("lastAttempt", value).apply()

    /**
     * Automatic checks run every [QUIET_MS]: on app start, on return to the
     * foreground and periodically while in the foreground (ZeronModel).
     */
    fun dueForQuietCheck(now: Long = System.currentTimeMillis()) = msUntilQuietCheck(now) == 0L

    /** How long until the next automatic check is due; 0 when it is. */
    fun msUntilQuietCheck(now: Long = System.currentTimeMillis()): Long =
        quietWaitMs(now, maxOf(lastCheckMs, lastAttemptMs))

    /**
     * Settings "Auto-check & download updates" (default on): download a newer
     * release in the background on an unmetered network, so the title-bar
     * badge turns straight into "ready to install". Off: no automatic checks
     * at all; "Check for updates" still works. Never installs by itself.
     */
    var autoUpdate: Boolean
        get() = prefs.getBoolean("autoUpdate", true)
        set(value) = prefs.edit().putBoolean("autoUpdate", value).apply()

    /** The last release a check found, so the badge survives an app restart. */
    fun lastKnown(): Release? = prefs.getString("lastRelease", null)?.let { raw ->
        runCatching {
            val o = JSONObject(raw)
            Release(
                tag = o.getString("tag"),
                name = o.getString("name"),
                notes = o.optString("notes"),
                versionCode = o.getLong("versionCode"),
                assetName = o.getString("assetName"),
                assetApiUrl = o.optString("assetApiUrl"),
                downloadUrl = o.getString("downloadUrl"),
                size = o.optLong("size", 0),
                htmlUrl = o.optString("htmlUrl"),
                sha256 = if (o.isNull("sha256")) null else o.optString("sha256").ifBlank { null },
            )
        }.getOrNull()
    }

    private fun remember(release: Release) {
        val o = JSONObject()
            .put("tag", release.tag).put("name", release.name).put("notes", release.notes)
            .put("versionCode", release.versionCode).put("assetName", release.assetName)
            .put("assetApiUrl", release.assetApiUrl).put("downloadUrl", release.downloadUrl)
            .put("size", release.size).put("htmlUrl", release.htmlUrl).put("sha256", release.sha256 ?: JSONObject.NULL)
        prefs.edit().putString("lastRelease", o.toString()).apply()
    }

    private val updatesDir: File get() = File(context.cacheDir, "updates")

    /**
     * Drop cached downloads that can't be installed as an update: everything
     * when [keep] is null or not newer than this build, else other releases'.
     */
    fun cleanStale(keep: Release?) {
        val files = updatesDir.listFiles() ?: return
        files.filter { keep == null || !keep.newer || !it.name.startsWith(keep.tag) }.forEach { it.delete() }
    }

    /** [release]'s APK if it's already downloaded and still verifies, else null. */
    suspend fun cached(release: Release): File? = withContext(Dispatchers.IO) {
        val done = File(updatesDir, "${release.tag}.apk")
        if (done.exists() && verifies(done, release)) done else null
    }

    private fun verifies(apk: File, release: Release): Boolean {
        val sizeOk = release.size <= 0 || apk.length() == release.size
        val shaOk = sizeOk && (release.sha256 == null || UpdateDownloader.sha256Of(apk) == release.sha256)
        return shaOk && checkSignature(apk) != Signature.MISMATCH
    }

    /** Key of the download source that worked last ([UpdateSources.Source.key]). */
    var preferredSource: String?
        get() = prefs.getString("preferredSource", null)
        set(value) = prefs.edit().putString("preferredSource", value).apply()

    /**
     * Latest release. Tries, in order: the API, the web redirect (no rate
     * limit), then each mirror (API through the mirror, then its redirect).
     * Any network failure moves on to the next; only a definite "no
     * release" (404 from the API) or a rejected token stops early. When
     * everything fails, the error lists what each source answered.
     */
    suspend fun latest(): Release = withContext(Dispatchers.IO) {
        val failures = mutableListOf<String>()
        val apiPath = "$api/repos/$REPO/releases/latest"
        val webPath = "$web/$REPO/releases/latest"
        fun attempt(label: String, block: () -> Release): Release? = try {
            block()
        } catch (e: Stop) {
            throw UpdateError(e.message.orEmpty())
        } catch (e: IOException) {
            failures += "$label: ${reason(e)}"
            null
        } catch (e: org.json.JSONException) {
            failures += "$label: ${str(R.string.update_reason_bad_response)}"
            null
        }
        val release = attempt(UpdateSources.label(api)) { viaApi(apiPath, direct = true) }
            ?: attempt(UpdateSources.label(web)) { viaWeb(webPath) }
            ?: UpdateSources.mirrors(mirror, builtInMirrors).firstNotNullOfOrNull { m ->
                attempt(UpdateSources.label(m)) { viaApi(m + apiPath, direct = false) }
                    ?: attempt(UpdateSources.label(m)) { viaWeb(m + webPath) }
            }
            ?: throw UpdateError(str(R.string.update_err_check_all) + "\n" + failures.joinToString("\n") { "• $it" })
        lastCheckMs = System.currentTimeMillis()
        remember(release)
        release
    }

    /** Ends the check early with this message (no point asking a mirror). */
    private class Stop(message: String) : Exception(message)

    private class HttpStatus(val code: Int) : IOException("HTTP $code")

    private fun viaApi(url: String, direct: Boolean): Release {
        val conn = open(URL(url), json = true, auth = direct, check = true)
        try {
            val code = conn.responseCode
            if (direct && code == 404) throw Stop(str(R.string.update_err_no_release))
            if (direct && code == 401 && !token.isNullOrBlank()) throw Stop(str(R.string.update_err_401))
            if (code !in 200..299) throw HttpStatus(code)
            return parse(JSONObject(conn.inputStream.bufferedReader().use { it.readText() }))
        } finally {
            conn.disconnect()
        }
    }

    /**
     * No-API fallback: `github.com/<repo>/releases/latest` redirects to the
     * latest tag, and the asset URL is predictable from the contract. The
     * version comes from the tag (`roundN[-P]` -> N*100 + P). Mirrors either
     * pass the redirect on (ghfast.top rewrites it to a path on itself) or
     * follow it themselves, in which case this source is skipped.
     */
    private fun viaWeb(url: String): Release {
        val conn = open(URL(url), json = false, auth = false, check = true)
        val location = try {
            val code = conn.responseCode
            if (code !in 300..399) throw HttpStatus(code)
            conn.getHeaderField("Location")
        } finally {
            conn.disconnect()
        }
        val tag = UpdateSources.tagFromLocation(location) ?: throw IOException(str(R.string.update_reason_bad_response))
        val asset = "zeron-android-$tag.apk"
        return Release(
            tag = tag,
            name = tag,
            notes = "",
            versionCode = versionFromTag(tag),
            assetName = asset,
            assetApiUrl = "",
            downloadUrl = "$web/$REPO/releases/download/$tag/$asset",
            size = 0,
            htmlUrl = "$web/$REPO/releases/tag/$tag",
            sha256 = null,
        )
    }

    private fun parse(o: JSONObject): Release {
        val tag = o.optString("tag_name")
        if (tag.isBlank()) throw IOException(str(R.string.update_reason_bad_response))
        val notes = o.optString("body")
        val assets = o.optJSONArray("assets")
        var asset: JSONObject? = null
        for (i in 0 until (assets?.length() ?: 0)) {
            val a = assets!!.getJSONObject(i)
            if (a.optString("name").endsWith(".apk")) {
                if (asset == null || a.optString("name") == "zeron-android-$tag.apk") asset = a
            }
        }
        asset ?: throw Stop(str(R.string.update_err_no_apk, tag))
        val code = Regex("""versionCode\s*[:=]\s*(\d+)""").find(notes)?.groupValues?.get(1)?.toLongOrNull()
            ?: versionFromTag(tag)
        return Release(
            tag = tag,
            name = o.optString("name").ifBlank { tag },
            notes = notes.lines().filterNot { it.trim().startsWith("versionCode") }.joinToString("\n").trim(),
            versionCode = code,
            assetName = asset.optString("name"),
            assetApiUrl = asset.optString("url"),
            downloadUrl = asset.optString("browser_download_url"),
            size = asset.optLong("size"),
            htmlUrl = o.optString("html_url"),
            sha256 = UpdateSources.sha256FromDigest(asset.optString("digest")),
        )
    }

    private fun open(url: URL, json: Boolean, auth: Boolean, check: Boolean): HttpURLConnection {
        val conn = url.openConnection() as HttpURLConnection
        // Checks are small: fail fast and move on to the next source.
        conn.connectTimeout = if (check) 8_000 else 10_000
        conn.readTimeout = if (check) 12_000 else 20_000
        conn.instanceFollowRedirects = false
        conn.setRequestProperty("User-Agent", USER_AGENT)
        if (json) {
            conn.setRequestProperty("Accept", "application/vnd.github+json")
            conn.setRequestProperty("X-GitHub-Api-Version", "2022-11-28")
        }
        val t = token
        if (auth && !t.isNullOrBlank() && url.host.endsWith("github.com")) {
            conn.setRequestProperty("Authorization", "Bearer $t")
        }
        return conn
    }

    private fun reason(e: IOException): String = when (e) {
        is HttpStatus -> if (e.code == 403 || e.code == 429) str(R.string.update_reason_rate_limited, e.code) else str(R.string.update_reason_http, e.code.toString())
        is java.net.SocketTimeoutException -> str(R.string.update_reason_timeout)
        is java.net.UnknownHostException -> str(R.string.update_reason_dns)
        is javax.net.ssl.SSLException -> str(R.string.update_reason_tls)
        else -> e.message ?: e.javaClass.simpleName
    }

    /** Why a download source was given up on, for the user. */
    fun reason(f: UpdateDownloader.Failure): String = when (f.kind) {
        UpdateDownloader.Kind.TIMEOUT -> str(R.string.update_reason_timeout)
        UpdateDownloader.Kind.SLOW -> str(R.string.update_reason_slow, speed(f.detail.toLongOrNull() ?: 0))
        UpdateDownloader.Kind.HTTP -> f.detail.toIntOrNull()?.let { str(R.string.update_reason_http, it.toString()) }
            ?: str(R.string.update_reason_unexpected, f.detail)
        UpdateDownloader.Kind.SIZE -> str(R.string.update_reason_size)
        UpdateDownloader.Kind.DIGEST -> str(R.string.update_reason_digest)
        UpdateDownloader.Kind.DNS -> str(R.string.update_reason_dns)
        UpdateDownloader.Kind.TLS -> str(R.string.update_reason_tls)
        UpdateDownloader.Kind.IO -> f.detail
    }

    /** "320 kB" (localized); used for speeds as "%s/s". */
    fun speed(bytesPerSec: Long): String = android.text.format.Formatter.formatShortFileSize(context, bytesPerSec)

    private val downloader = UpdateDownloader(USER_AGENT)

    /**
     * Download the APK into the app cache, trying every source in turn and
     * resuming across them. The result has the release's size and SHA-256
     * (when GitHub's API gave them) and is signed with this app's key.
     */
    suspend fun download(
        release: Release,
        prefer: String? = null,
        cancelled: () -> Boolean = { false },
        onFailure: (UpdateDownloader.Failure) -> Unit = {},
        progress: (UpdateDownloader.Progress) -> Unit,
    ): File = withContext(Dispatchers.IO) {
        val dir = updatesDir.apply { mkdirs() }
        dir.listFiles()?.filter { !it.name.startsWith(release.tag) }?.forEach { it.delete() }
        val done = File(dir, "${release.tag}.apk")
        if (done.exists()) {
            if (verifies(done, release)) return@withContext done
            done.delete()
        }
        val part = partFile(release)
        val sources = UpdateSources.downloadSources(release.downloadUrl, mirror, prefer ?: preferredSource, token, release.assetApiUrl, builtInMirrors)
        val won = try {
            downloader.download(sources, part, release.size, release.sha256, cancelled, onFailure, progress)
        } catch (e: UpdateDownloader.Failed) {
            throw UpdateError(str(R.string.update_err_download_all) + "\n" + e.failures.joinToString("\n") { "• ${it.source}: ${reason(it)}" })
        }
        preferredSource = won.key
        if (!part.renameTo(done)) throw UpdateError(str(R.string.update_err_save))
        if (checkSignature(done) == Signature.MISMATCH) {
            done.delete()
            throw UpdateError(str(R.string.update_err_signature, won.label))
        }
        done
    }

    private fun partFile(release: Release) = File(updatesDir, "${release.tag}.apk.part")

    /** The sources the "switch mirror" picker offers for [release]. */
    fun sources(release: Release): List<UpdateSources.Source> =
        UpdateSources.choices(release.downloadUrl, mirror, token, release.assetApiUrl, builtInMirrors)

    /** Key of the user's own mirror (高级 / Advanced settings), if one is set. */
    fun customMirrorKey(): String? = UpdateSources.normalizeMirror(mirror)

    /** Unblock a download in progress; pair with its `cancelled` returning true. */
    fun abortDownload() = downloader.abort()

    /** Bytes of [release] already on disk, which the next download resumes from. */
    fun partialBytes(release: Release): Long = downloader.withFile { partFile(release).let { if (it.exists()) it.length() else 0L } }

    /** Throw away [release]'s partial download (safe while a cancelled run is still winding down). */
    fun discardPartial(release: Release) {
        downloader.withFile { partFile(release).delete() }
    }

    /** Link for the browser / clipboard: through the mirror that worked last, if any. */
    fun browserUrl(release: Release): String {
        val m = preferredSource?.takeIf { it != UpdateSources.GITHUB } ?: UpdateSources.normalizeMirror(mirror)
        return if (m != null) m + release.downloadUrl else release.downloadUrl
    }

    enum class Signature { MATCH, MISMATCH, UNKNOWN }

    private fun checkSignature(apk: File): Signature = signatureCheck?.invoke(apk) ?: signature(apk)

    /**
     * Whether [apk] is this app, signed with the same certificate. A mirror
     * can't slip in anything the installer would accept anyway (updates must
     * match the installed signer), but checking first gives a clear message
     * instead of the installer's "package conflicts". UNKNOWN (certs not
     * readable on this Android version) leaves the decision to the installer.
     */
    fun signature(apk: File): Signature {
        val pm = context.packageManager
        val archive = runCatching { archiveInfo(pm, apk) }.getOrNull() ?: return Signature.MISMATCH
        if (archive.packageName != context.packageName) return Signature.MISMATCH
        val mine = runCatching { certs(installedInfo(pm)) }.getOrNull().orEmpty()
        val theirs = certs(archive)
        if (mine.isEmpty() || theirs.isEmpty()) return Signature.UNKNOWN
        return if (mine == theirs) Signature.MATCH else Signature.MISMATCH
    }

    @Suppress("DEPRECATION")
    private fun archiveInfo(pm: android.content.pm.PackageManager, apk: File): android.content.pm.PackageInfo? {
        val flags = if (Build.VERSION.SDK_INT >= 28) android.content.pm.PackageManager.GET_SIGNING_CERTIFICATES else android.content.pm.PackageManager.GET_SIGNATURES
        val info = pm.getPackageArchiveInfo(apk.path, flags) ?: return null
        if (certs(info).isEmpty()) {
            // Some Android versions leave signingInfo empty for archives; try the old field.
            pm.getPackageArchiveInfo(apk.path, android.content.pm.PackageManager.GET_SIGNATURES)?.let { return it }
        }
        return info
    }

    @Suppress("DEPRECATION")
    private fun installedInfo(pm: android.content.pm.PackageManager): android.content.pm.PackageInfo {
        val flags = if (Build.VERSION.SDK_INT >= 28) android.content.pm.PackageManager.GET_SIGNING_CERTIFICATES else android.content.pm.PackageManager.GET_SIGNATURES
        return pm.getPackageInfo(context.packageName, flags)
    }

    @Suppress("DEPRECATION")
    private fun certs(info: android.content.pm.PackageInfo): Set<String> {
        val sigs = if (Build.VERSION.SDK_INT >= 28 && info.signingInfo != null) {
            info.signingInfo!!.apkContentsSigners
        } else {
            info.signatures
        }
        return sigs.orEmpty().map { java.security.MessageDigest.getInstance("SHA-256").digest(it.toByteArray()).joinToString("") { b -> "%02x".format(b) } }.toSet()
    }

    /** Android 8+: the per-app "install unknown apps" switch. */
    fun canInstall(): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.O || context.packageManager.canRequestPackageInstalls()

    fun unknownSourcesIntent(): Intent =
        Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    fun installIntent(apk: File): Intent {
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.updates", apk)
        return Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
    }

    companion object {
        /** `round5` -> 500, `round5-2` / `round5.2` -> 502, else 0. */
        fun versionFromTag(tag: String): Long {
            val m = Regex("""round(\d+)(?:[.-](\d+))?""").find(tag) ?: return 0L
            val round = m.groupValues[1].toLongOrNull() ?: return 0L
            val patch = m.groupValues[2].toLongOrNull() ?: 0L
            return round * 100 + patch
        }

        const val WEB = "https://github.com"
        const val REPO = "villatothesea/zeron-android-app"
        const val API = "https://api.github.com"
        /** Automatic update checks: every 30 minutes. */
        const val QUIET_MS = 30L * 60 * 1000

        /** Shortest nap of the foreground check loop, so it can never spin. */
        const val MIN_LOOP_WAIT_MS = 60L * 1000

        /**
         * Wait from [now] until [QUIET_MS] after [last]. A [last] in the future
         * (the clock was set back) counts as due rather than blocking checks
         * until the clock catches up.
         */
        fun quietWaitMs(now: Long, last: Long): Long {
            if (last <= 0 || last > now) return 0
            return (QUIET_MS - (now - last)).coerceAtLeast(0)
        }

        /**
         * The foreground check loop: [check] now, then again whenever
         * [nextInMs] says one is due (at least [MIN_LOOP_WAIT_MS] apart).
         * Runs until its coroutine is cancelled (the app went to background).
         */
        suspend fun repeatQuietChecks(nextInMs: () -> Long, check: () -> Unit): Nothing {
            while (true) {
                check()
                kotlinx.coroutines.delay(nextInMs().coerceAtLeast(MIN_LOOP_WAIT_MS))
            }
        }
        val USER_AGENT = "zeron-android/${BuildConfig.VERSION_NAME}"
    }
}
