package sh.zeron.android.ui

import android.content.ClipboardManager
import android.view.HapticFeedbackConstants
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SheetValue
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ApiBaseUrlCheck
import uniffi.zeron_core.apiKeySuffix
import uniffi.zeron_core.apiKeyTakesBaseUrl
import uniffi.zeron_core.checkApiBaseUrl
import kotlin.coroutines.cancellation.CancellationException

/**
 * Adds an API key to Cloud's vault — the desktop's `CloudKeyDialog`
 * (`apps/zgx/ui/ds/provider_card.luau`, words from `ds.provider_card.cloud_key_*`)
 * and iOS's `APIKeySheetViewController`, in the same order: the key, then
 * for a key that can take one ([apiKeyTakesBaseUrl]) where its requests go,
 * Anthropic's API or an LLM gateway's base URL. The base URL is judged as it
 * is typed by the vault's own rules ([checkApiBaseUrl]): quiet while half
 * typed, the vault's words when refused, the exact route once valid. Save
 * waits for that; a key the vault refuses keeps the sheet open, as typed,
 * with the reason under it.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ApiKeySheet(agent: CloudStore.Agent, onDismiss: () -> Unit, onSave: suspend (key: String, baseUrl: String?) -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var saving by remember { mutableStateOf(false) }
    // A save under way can't be swiped away: its reply decides whether the sheet stays.
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true, confirmValueChange = { it != SheetValue.Hidden || !saving })
    val takesBaseUrl = remember(agent.key) { apiKeyTakesBaseUrl(agent.key) }
    var key by rememberSaveable { mutableStateOf("") }
    var gateway by rememberSaveable { mutableStateOf(false) }
    var url by rememberSaveable { mutableStateOf("") }
    var refused by remember { mutableStateOf<String?>(null) }
    val keyFocus = remember { FocusRequester() }
    val urlFocus = remember { FocusRequester() }
    val suffix = remember(key) { apiKeySuffix(key) }
    val check = remember(url) { checkApiBaseUrl(agent.key, url) }
    val route = (check as? ApiBaseUrlCheck.Ready)?.takeIf { gateway }
    val ready = key.isNotEmpty() && (!gateway || route != null) && !saving
    // "Anthropic API key" → "Anthropic API", the route's own name (`cloud_key_api`).
    val api = agent.keyName.removeSuffix(" key")

    LaunchedEffect(Unit) { runCatching { keyFocus.requestFocus() } }
    LaunchedEffect(gateway) { if (gateway && url.isEmpty()) runCatching { urlFocus.requestFocus() } }

    fun close() {
        if (saving) return
        scope.launch { sheet.hide() }.invokeOnCompletion { onDismiss() }
    }

    fun save() {
        if (!ready) return
        saving = true
        refused = null
        scope.launch {
            val error = try {
                onSave(key, route?.url)
                null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                CloudStore.message(e)
            }
            saving = false
            if (error == null) {
                view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                sheet.hide()
                onDismiss()
            } else {
                refused = error
                view.performHapticFeedback(HapticFeedbackConstants.REJECT)
            }
        }
    }

    fun paste() {
        val clip = context.getSystemService(ClipboardManager::class.java).primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()?.trim()
        if (!text.isNullOrEmpty()) {
            key = text
            refused = null
        }
    }

    ModalBottomSheet(onDismissRequest = { if (!saving) onDismiss() }, sheetState = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .imePadding()
                .padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            PageIntro("Add ${agent.keyName}", "${agent.name} uses it on every Cloud machine, from its next turn.")

            OutlinedTextField(
                key,
                {
                    key = it.trim()
                    refused = null
                },
                label = { Text("API key") },
                placeholder = { Text(if (agent.key == "anthropic-key") "sk-ant-…" else "sk-…") },
                singleLine = true,
                enabled = !saving,
                isError = refused != null,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(
                    keyboardType = KeyboardType.Password,
                    autoCorrectEnabled = false,
                    imeAction = if (gateway) ImeAction.Next else ImeAction.Done,
                ),
                keyboardActions = KeyboardActions(onNext = { runCatching { urlFocus.requestFocus() } }, onDone = { save() }),
                // The key's last characters once typed, as the vault will list it; Paste before that.
                trailingIcon = {
                    if (suffix != null) {
                        Text(
                            suffix,
                            fontFamily = GeistMono,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(end = 16.dp).semantics { contentDescription = "Ends in ${suffix.trimStart('…')}" },
                        )
                    } else {
                        IconButton(onClick = ::paste, enabled = !saving) {
                            ZIcon(ZIcons.Paste, "Paste", Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                },
                supportingText = { Text(refused ?: "Kept in Zeron's vault, never on this phone.") },
                shape = RoundedCornerShape(16.dp),
                modifier = Modifier.fillMaxWidth().focusRequester(keyFocus),
            )

            if (takesBaseUrl) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("Requests go to", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                        listOf(api to false, "LLM gateway" to true).forEachIndexed { i, (label, toGateway) ->
                            SegmentedButton(
                                selected = gateway == toGateway,
                                onClick = {
                                    gateway = toGateway
                                    refused = null
                                },
                                shape = SegmentedButtonDefaults.itemShape(i, 2),
                                enabled = !saving,
                            ) { Text(label) }
                        }
                    }
                }
                AnimatedVisibility(gateway) {
                    OutlinedTextField(
                        url,
                        {
                            url = it.trim()
                            refused = null
                        },
                        label = { Text("Base URL") },
                        placeholder = { Text("https://llm-gateway.example.com", fontFamily = GeistMono) },
                        singleLine = true,
                        enabled = !saving,
                        isError = check is ApiBaseUrlCheck.Refused,
                        textStyle = LocalTextStyle.current.copy(fontFamily = GeistMono),
                        keyboardOptions = KeyboardOptions(
                            keyboardType = KeyboardType.Uri,
                            capitalization = KeyboardCapitalization.None,
                            autoCorrectEnabled = false,
                            imeAction = ImeAction.Done,
                        ),
                        keyboardActions = KeyboardActions(onDone = { save() }),
                        supportingText = {
                            when (val c = check) {
                                is ApiBaseUrlCheck.Refused -> Text(c.reason)
                                // The exact URL Claude Code will call, before anything is saved.
                                is ApiBaseUrlCheck.Ready -> Text(
                                    buildAnnotatedString {
                                        append("${agent.name} sends each request to\n")
                                        withStyle(SpanStyle(fontFamily = GeistMono, color = MaterialTheme.colorScheme.onSurface)) {
                                            append(c.url)
                                            append(c.path)
                                        }
                                    },
                                )
                                ApiBaseUrlCheck.Empty, ApiBaseUrlCheck.Typing -> Text("Your gateway's https:// address, without /v1.")
                            }
                        },
                        shape = RoundedCornerShape(16.dp),
                        modifier = Modifier.fillMaxWidth().focusRequester(urlFocus),
                    )
                }
            }

            Column {
                PrimaryCapsule("Save", onClick = ::save, enabled = ready, busy = saving)
                PlainCapsule("Cancel", onClick = ::close)
            }
        }
    }
}
