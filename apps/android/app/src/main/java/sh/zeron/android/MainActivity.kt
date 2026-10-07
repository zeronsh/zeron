package sh.zeron.android

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import android.widget.FrameLayout
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.appcompat.app.AppCompatActivity
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.GlassFrameLayout
import sh.zeron.android.design.LocalGlassFrame
import sh.zeron.android.ui.ZeronApp

/**
 * AppCompatActivity so the per-app language (AppLanguage) applies below
 * Android 13 too.
 *
 * Language and dark-mode changes don't recreate the activity (manifest
 * configChanges="uiMode|locale|layoutDirection"): recreation flashed the
 * window and dropped screen state such as the Settings scroll position.
 * The resources are updated in place (by the system on Android 13+, by
 * AppCompat below), and [onConfigurationChanged] hands the new
 * configuration to Compose so every stringResource recomposes.
 */
class MainActivity : AppCompatActivity() {
    private val model: ZeronModel by viewModels()

    /** Latest configuration for Compose; null until the first change. */
    private var configuration by mutableStateOf<Configuration?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        handleIntent(intent)
        val frame = GlassFrameLayout(this)
        val compose = ComposeView(this).apply {
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
            setContent {
                // Below Android 13, AppCompat applies a language change by
                // calling onConfigurationChanged directly, without the view
                // dispatch that would update Compose's own LocalConfiguration.
                val config = configuration ?: LocalConfiguration.current
                CompositionLocalProvider(LocalGlassFrame provides frame, LocalConfiguration provides config) {
                    ZeronApp(model)
                }
            }
        }
        frame.addView(compose, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))
        setContentView(frame)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        configuration = Configuration(newConfig)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIntent(intent)
    }

    override fun onStart() {
        super.onStart()
        model.onForeground()
    }

    override fun onStop() {
        model.onBackground()
        super.onStop()
    }

    private fun handleIntent(intent: Intent?) {
        val data = intent?.dataString
        if (data != null && data.startsWith("zeron://")) model.completeAuth(data)
        val route = intent?.getStringExtra("route") ?: return
        if (route == "addmachine") {
            model.launchAddMachine(intent.getStringExtra("name"), intent.getStringExtra("host"), intent.getStringExtra("port"), intent.getStringExtra("user"))
            return
        }
        model.applyLaunch(route, intent.getStringExtra("chat"), intent.getStringExtra("theme"), intent.getStringExtra("query"))
    }
}
