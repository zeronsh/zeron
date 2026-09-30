package sh.zeron.android

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import sh.zeron.android.core.LaunchOptions
import sh.zeron.android.ui.ZeronRoot

/**
 * Launch extras mirror the iOS launch arguments, e.g.
 * `adb shell am start -n sh.zeron.android/.MainActivity --ez demo true --es route chat:chat-veil`
 * (`--es wallpaper <path>` sets the wallpaper from a file the app can read;
 * debuggable builds: `--es dev-edge http://10.0.2.2:27740 --es dev-user u
 * --es dev-org o` signs in to an `AUTH_MODE=dev` edge).
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
                devEdge = extras?.getString("dev-edge"),
                devUser = extras?.getString("dev-user"),
                devOrg = extras?.getString("dev-org"),
                route = extras?.getString("route"),
                wallpaper = extras?.getString("wallpaper"),
                wallpaperEffect = extras?.getString("wallpaper-effect"),
            ),
        )
        handleCallback(intent)
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = model.onForeground()
            override fun onStop(owner: LifecycleOwner) = model.onBackground()
        })
        setContent { ZeronRoot(model) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleCallback(intent)
    }

    private fun handleCallback(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme == "zeron") model.handleCallback(data.toString())
    }
}
