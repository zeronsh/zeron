package sh.zeron.android

import android.content.Intent
import android.os.Bundle
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.GlassFrameLayout
import sh.zeron.android.design.LocalGlassFrame
import sh.zeron.android.ui.ZeronApp

class MainActivity : ComponentActivity() {
    private val model: ZeronModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        handleIntent(intent)
        val frame = GlassFrameLayout(this)
        val compose = ComposeView(this).apply {
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
            setContent {
                CompositionLocalProvider(LocalGlassFrame provides frame) {
                    ZeronApp(model)
                }
            }
        }
        frame.addView(compose, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))
        setContentView(frame)
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
