package sh.zeron.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.view.HapticFeedbackConstants
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.GithubDeviceFlowInfo

/** Where the connect sheet is (iOS `GitHubConnectViewController`). */
private sealed interface Connect {
    data object Starting : Connect
    data class Waiting(val flow: GithubDeviceFlowInfo) : Connect
    data class Connected(val account: String?) : Connect
    data class Failed(val message: String, val flow: GithubDeviceFlowInfo?) : Connect
}

/**
 * Connect GitHub for Cloud: GitHub's device flow. The code is shown big (and
 * copied), GitHub opens in the browser, and the sheet follows the approval
 * until the account is connected.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun GitHubConnectSheet(model: AppModel, onDismiss: () -> Unit, onConnected: () -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var state by remember { mutableStateOf<Connect>(Connect.Starting) }
    // Bumped by Try again: restarts the flow.
    var attempt by remember { mutableIntStateOf(0) }

    LaunchedEffect(attempt) {
        val client = model.client.value ?: return@LaunchedEffect
        state = Connect.Starting
        val flow = try {
            client.githubConnectStart()
        } catch (e: Exception) {
            state = Connect.Failed(CloudStore.message(e), null)
            return@LaunchedEffect
        }
        state = Connect.Waiting(flow)
        val interval = maxOf(flow.intervalSecs.toLong(), 2L) * 1000
        while (true) {
            delay(interval)
            if (flow.expiresAtMs > 0 && System.currentTimeMillis() > flow.expiresAtMs) {
                state = Connect.Failed("The code expired. Get a new one.", null)
                return@LaunchedEffect
            }
            val progress = runCatching { client.githubConnectPoll(flow.flowId) }.getOrNull() ?: continue
            when (progress.state) {
                "connected" -> {
                    view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                    state = Connect.Connected(progress.account)
                    model.cloud.refreshGitHub()
                    delay(900)
                    sheet.hide()
                    onDismiss()
                    onConnected()
                    return@LaunchedEffect
                }
                "failed" -> {
                    view.performHapticFeedback(HapticFeedbackConstants.REJECT)
                    state = Connect.Failed(progress.error ?: "GitHub didn't connect. Try again.", flow)
                    return@LaunchedEffect
                }
            }
        }
    }

    fun openGitHub(flow: GithubDeviceFlowInfo) {
        context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("GitHub code", flow.userCode))
        model.browser.open(context, flow.verificationUri)
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            Box(
                Modifier.size(64.dp).clip(RoundedCornerShape(22.dp)).background(chipContainer()),
                contentAlignment = Alignment.Center,
            ) { ZIcon(ZIcons.GitHub, null, Modifier.size(32.dp), tint = MaterialTheme.colorScheme.onSurface) }
            PageIntro(
                "Connect GitHub",
                "Enter this code on GitHub to let Cloud agents clone your repositories and push their work.",
                centered = true,
            )
            // The code card: the code itself, or where the flow ended.
            Surface(shape = RoundedCornerShape(24.dp), color = cardColor(), modifier = Modifier.fillMaxWidth()) {
                AnimatedContent(state, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "code", contentKey = { it::class }) { s ->
                    Row(
                        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 22.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.Center,
                    ) {
                        when (s) {
                            Connect.Starting -> Text("· · · ·", fontFamily = GeistMono, fontSize = 34.sp, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.outline)
                            is Connect.Waiting -> {
                                Text(
                                    s.flow.userCode,
                                    fontFamily = GeistMono,
                                    fontSize = 34.sp,
                                    fontWeight = FontWeight.SemiBold,
                                    modifier = Modifier.semantics { contentDescription = "GitHub code ${s.flow.userCode}" },
                                )
                                Spacer(Modifier.width(8.dp))
                                IconButton(onClick = {
                                    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("GitHub code", s.flow.userCode))
                                    view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                                    model.toast("Code copied")
                                }) { ZIcon(ZIcons.Copy, "Copy code", Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) }
                            }
                            is Connect.Connected -> Text("Connected", style = MaterialTheme.typography.headlineSmallEmphasized, color = successColor())
                            is Connect.Failed -> ZIcon(ZIcons.Warning, null, Modifier.size(32.dp), tint = MaterialTheme.colorScheme.error)
                        }
                    }
                }
            }
            // Status line under the card.
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center) {
                val (text, busy, error) = when (val s = state) {
                    Connect.Starting -> Triple("Getting a code…", true, false)
                    is Connect.Waiting -> Triple("Waiting for GitHub…", true, false)
                    is Connect.Connected -> Triple(s.account?.let { "Signed in to GitHub as @$it." } ?: "GitHub is connected.", false, false)
                    is Connect.Failed -> Triple(s.message, false, true)
                }
                if (busy) {
                    LoadingIndicator(Modifier.size(24.dp))
                    Spacer(Modifier.width(8.dp))
                }
                Text(
                    text,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(4.dp))
            when (val s = state) {
                is Connect.Waiting -> PrimaryCapsule("Copy code & open GitHub", onClick = { openGitHub(s.flow) }, icon = ZIcons.Link)
                is Connect.Failed -> PrimaryCapsule("Try again", onClick = { attempt++ }, icon = ZIcons.Refresh)
                Connect.Starting -> PrimaryCapsule("Copy code & open GitHub", onClick = {}, icon = ZIcons.Link, enabled = false)
                is Connect.Connected -> Unit
            }
            if (state !is Connect.Connected) {
                PlainCapsule("Cancel", onClick = {
                    scope.launch { sheet.hide() }.invokeOnCompletion { onDismiss() }
                })
            }
        }
    }
}
