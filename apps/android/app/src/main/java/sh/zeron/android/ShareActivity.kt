package sh.zeron.android

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.core.content.IntentCompat
import sh.zeron.android.design.ZeronTheme
import sh.zeron.android.ui.ShareScreen

/**
 * "Share to Zeron": the share-sheet target (ACTION_SEND / SEND_MULTIPLE).
 * Stages the shared content into the phone engine's outbox, lets you pick
 * one of your devices and sends it from the engine (docs/file-transfer.md
 * § Clients).
 */
class ShareActivity : ComponentActivity() {
    private val model get() = (application as ZeronApplication).model

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        model.ensureBooted()
        val uris = sharedUris(intent)
        val text = if (uris.isEmpty()) intent.getStringExtra(Intent.EXTRA_TEXT) else null
        setContent {
            val appearance by model.appearance.collectAsState()
            ZeronTheme(appearance) {
                ShareScreen(
                    model,
                    uris,
                    text,
                    onClose = { finish() },
                    onOpenApp = { route ->
                        startActivity(
                            Intent(this, MainActivity::class.java)
                                .apply { if (route != null) putExtra("route", route) }
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                        )
                        finish()
                    },
                )
            }
        }
    }

    private fun sharedUris(intent: Intent): List<Uri> {
        val extra = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE -> IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> emptyList()
        }
        if (extra.isNotEmpty()) return extra.distinct()
        // Some senders only fill ClipData.
        val clip = intent.clipData ?: return emptyList()
        return (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }.distinct()
    }
}
