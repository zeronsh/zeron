package sh.zeron.android.voice

import android.Manifest
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Snackbar
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

/**
 * The call's app-wide side effects, composed once inside the themed root
 * (the iOS scene-level pieces): the microphone prompt the call's Prepare step
 * asks for, the screen kept awake while a call runs (like the phone app), and
 * the snackbar that explains a call ending while the stage isn't showing —
 * Retry calls again, Settings opens the microphone permission.
 *
 * Draw it last, over the app (it fills its parent but takes no touches);
 * [padding] lifts the snackbar above floating chrome.
 */
@Composable
fun VoiceHost(controller: VoiceController, modifier: Modifier = Modifier, padding: PaddingValues = PaddingValues(start = 16.dp, end = 16.dp, bottom = 176.dp)) {
    val s by controller.snapshot.collectAsState()
    val view = LocalView.current
    DisposableEffect(s.live) {
        view.keepScreenOn = s.live
        onDispose { view.keepScreenOn = false }
    }

    val request by controller.permissionRequest.collectAsState()
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { result ->
        controller.completePermission(result[Manifest.permission.RECORD_AUDIO] == true)
    }
    LaunchedEffect(request) {
        if (request == null) return@LaunchedEffect
        // The call notification rides along with the microphone prompt; only the microphone is required.
        val permissions = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }
        runCatching { launcher.launch(permissions.toTypedArray()) }.onFailure { controller.completePermission(false) }
    }

    val notice by controller.notice.collectAsState()
    LaunchedEffect(notice) {
        if (notice == null) return@LaunchedEffect
        delay(8_000)
        controller.dismissNotice()
    }
    Box(modifier.fillMaxSize()) { NoticeBar(controller, notice, padding) }
}

@Composable
private fun BoxScope.NoticeBar(controller: VoiceController, notice: VoiceNotice?, padding: PaddingValues) {
    val context = LocalContext.current
    AnimatedVisibility(
        notice != null,
        enter = fadeIn() + slideInVertically { it / 2 },
        exit = fadeOut() + slideOutVertically { it / 2 },
        modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(padding),
    ) {
        val n = notice ?: return@AnimatedVisibility
        Snackbar(
            action = {
                TextButton(onClick = { controller.noticeAction { openAppSettings(context) } }) {
                    Text(n.action, color = MaterialTheme.colorScheme.inversePrimary)
                }
            },
            dismissAction = null,
        ) { Text(n.message) }
    }
}
