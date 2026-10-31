package sh.zeron.android

import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.view.MotionEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import sh.zeron.android.core.LaunchOptions
import sh.zeron.android.ui.ZeronRoot

/**
 * Launch extras mirror the iOS launch arguments, e.g.
 * `adb shell am start -n sh.zeron.android/.MainActivity --ez demo true --es route chat:chat-veil`
 * (`--es wallpaper <path>` sets the wallpaper from a file the app can read).
 */
class MainActivity : ComponentActivity() {
    private val model get() = (application as ZeronApplication).model

    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val extras = intent.extras
        model.boot(
            LaunchOptions(
                demo = extras?.getBoolean("demo") == true,
                fast = extras?.getBoolean("fast") == true,
                longReply = extras?.getBoolean("longreply") == true,
                big = extras?.getBoolean("big") == true,
                huge = extras?.getBoolean("huge") == true,
                noProjects = extras?.getBoolean("noprojects") == true,
                signedOut = extras?.getBoolean("signedout") == true,
                route = extras?.getString("route"),
                companion = extras?.getString("companion"),
                terminalDemo = extras?.getBoolean("terminal-demo") == true,
                cloud = extras?.getBoolean("cloud") == true,
                wallpaper = extras?.getString("wallpaper"),
                wallpaperEffect = extras?.getString("wallpaper-effect"),
                voicePreview = extras?.getBoolean("voice-preview") == true,
                voiceStage = extras?.getBoolean("voice-stage") == true,
                remoteVoice = extras?.getBoolean("remote-voice") == true,
                voiceSelfTest = extras?.getBoolean("voice-selftest") == true,
            ),
        )
        handleCallback(intent)
        setContent { ZeronRoot(model) }
    }

    // Some OEMs (Nothing OS) hold third-party windows below the panel's peak rate unless they ask for
    // a display mode. Ask while the user is touching the app and for a while after (flings, the next
    // gesture: each switch costs one long frame), then hand the choice back so an idle screen doesn't
    // stay pinned at the peak rate.
    private val fastestModeId by lazy { fastestDisplayModeId() }
    private val releaseFastestMode = Runnable { preferDisplayMode(0) }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                window.decorView.removeCallbacks(releaseFastestMode)
                preferDisplayMode(fastestModeId)
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL ->
                window.decorView.postDelayed(releaseFastestMode, 10_000)
        }
        return super.dispatchTouchEvent(ev)
    }

    private fun preferDisplayMode(id: Int) {
        if (window.attributes.preferredDisplayModeId != id) {
            window.attributes = window.attributes.apply { preferredDisplayModeId = id }
        }
    }

    private fun fastestDisplayModeId(): Int {
        @Suppress("DEPRECATION")
        val display = (if (Build.VERSION.SDK_INT >= 30) display else windowManager.defaultDisplay) ?: return 0
        val current = display.mode
        return display.supportedModes
            .filter { it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight }
            .maxByOrNull { it.refreshRate }?.modeId ?: 0
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleCallback(intent)
    }

    private fun handleCallback(intent: Intent?) {
        // The call notification (or `zeron://voice`) returns to the stage.
        if (sh.zeron.android.voice.VoiceController.opensStage(intent)) {
            if (model.voice.live) model.voice.requestStage()
            return
        }
        val data = intent?.data ?: return
        if (data.scheme != "zeron") return
        if (data.host == "callback") model.handleCallback(data.toString()) else model.browser.links.tryEmit(data)
    }

    override fun onResume() {
        super.onResume()
        model.browser.resumes.tryEmit(Unit)
    }
}
