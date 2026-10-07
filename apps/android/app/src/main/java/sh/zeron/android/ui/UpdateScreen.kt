package sh.zeron.android.ui

import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.R
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.BuildConfig
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface

/** "Software Update": latest GitHub release, notes, download + install. */
@Composable
fun UpdateScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    val release = model.updateRelease
    var advanced by remember { mutableStateOf(false) }
    var token by remember { mutableStateOf("") }
    var mirror by remember { mutableStateOf(model.updater.mirror.orEmpty()) }
    val hasToken = remember(advanced) { !model.updater.token.isNullOrBlank() }
    Column(Modifier.fillMaxSize().background(colors.background).consumeBlankTaps().statusBarsPadding().navigationBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(colors, onClick = { model.showUpdate = false })
            Spacer(Modifier.width(10.dp))
            Text(stringResource(R.string.software_update), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            GroupLabel(colors, stringResource(R.string.update_installed))
            SettingRow(colors, BuildConfig.VERSION_NAME, stringResource(R.string.update_build, BuildConfig.VERSION_CODE))
            GroupLabel(colors, stringResource(R.string.update_latest_github))
            when {
                model.updateChecking -> SettingRow(colors, stringResource(R.string.update_checking), "github.com/villatothesea/zeron-android-app")
                release == null -> SettingRow(colors, stringResource(R.string.update_not_checked), null, onClick = { model.checkForUpdates() })
                else -> {
                    SettingRow(
                        colors,
                        release.name,
                        stringResource(if (release.newer) R.string.update_build_newer else R.string.update_build_current, release.versionCode.toInt()),
                    )
                    if (release.notes.isNotBlank()) {
                        // Release notes are GitHub Markdown: render, don't show the raw text.
                        MarkdownText(
                            release.notes,
                            colors,
                            modifier = Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp),
                        )
                    }
                }
            }
            model.updateProgress?.let { p ->
                Spacer(Modifier.height(10.dp))
                ProgressBar(colors, p)
                val status = model.updateStatus
                val line = when {
                    p >= 1f -> stringResource(R.string.update_downloaded)
                    status == null -> stringResource(R.string.update_downloading, (p * 100).toInt())
                    status.total > 0 -> stringResource(R.string.update_downloading_from, status.source, (p * 100).toInt(), model.updater.speed(status.bytesPerSec))
                    else -> stringResource(R.string.update_downloading_from_size, status.source, model.updater.speed(status.done), model.updater.speed(status.bytesPerSec))
                }
                Text(line, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp))
                if (p < 1f) {
                    Spacer(Modifier.height(10.dp))
                    Row {
                        SheetButton(colors, stringResource(R.string.download_switch), Modifier.weight(1f).testTag("update-switch")) { model.showSourcePicker = true }
                        Spacer(Modifier.width(8.dp))
                        SheetButton(colors, stringResource(R.string.download_cancel), Modifier.weight(1f).testTag("update-cancel"), danger = true) { model.cancelDownload() }
                    }
                }
            }
            model.updateError?.let {
                Text(it, color = colors.danger, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(top = 10.dp))
            }
            Spacer(Modifier.height(14.dp))
            val busy = model.updateChecking || (model.updateProgress != null && model.updateProgress!! < 1f)
            if (release != null && release.newer) {
                Button(colors, stringResource(if (model.updateProgress == 1f) R.string.update_install else R.string.update_download_install), primary = true, enabled = !busy) {
                    if (model.updateProgress == 1f) model.installUpdate() else model.downloadUpdate()
                }
                Spacer(Modifier.height(8.dp))
                // Way out when every source fails in-app: the browser may have its own proxy.
                Row {
                    Button(colors, stringResource(R.string.update_open_browser), modifier = Modifier.weight(1f)) { model.openUpdateInBrowser() }
                    Spacer(Modifier.width(8.dp))
                    Button(colors, stringResource(R.string.update_copy_link), modifier = Modifier.weight(1f)) { model.copyUpdateLink() }
                }
                Spacer(Modifier.height(8.dp))
            }
            Button(colors, stringResource(R.string.update_check_again), enabled = !busy) { model.checkForUpdates() }
            Text(
                stringResource(R.string.update_install_hint),
                color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(top = 10.dp, start = 4.dp),
            )
            Text(
                stringResource(R.string.update_sources_hint),
                color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp, start = 4.dp),
            )
            GroupLabel(colors, stringResource(R.string.update_advanced))
            if (!advanced) {
                SettingRow(colors, stringResource(R.string.update_mirror_token), model.updater.mirror?.let { stringResource(R.string.update_mirror_value, it) } ?: stringResource(R.string.off), onClick = { advanced = true })
            } else {
                Text(stringResource(R.string.update_mirror_hint), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(start = 4.dp, bottom = 4.dp))
                Input(colors, mirror, "https://mirror.example/", password = false) { mirror = it }
                Spacer(Modifier.height(8.dp))
                Text(stringResource(R.string.update_token_hint), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(start = 4.dp, bottom = 4.dp))
                Input(colors, token, if (hasToken) stringResource(R.string.saved_paste_replace) else "github_pat_…", password = true) { token = it }
                Spacer(Modifier.height(8.dp))
                Row {
                    Button(colors, stringResource(R.string.save), primary = true, modifier = Modifier.weight(1f)) {
                        model.updater.mirror = mirror.ifBlank { null }
                        if (token.isNotBlank()) model.updater.token = token
                        token = ""
                        advanced = false
                        model.showToast(context.getString(R.string.saved))
                    }
                    Spacer(Modifier.width(8.dp))
                    Button(colors, stringResource(R.string.clear_token), modifier = Modifier.weight(1f)) {
                        model.updater.token = null
                        model.showToast(context.getString(R.string.token_removed))
                    }
                }
            }
            Spacer(Modifier.height(40.dp))
        }
    }
}

@Composable
private fun Button(colors: sh.zeron.android.design.ZeronColors, label: String, primary: Boolean = false, enabled: Boolean = true, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Box(
        modifier.fillMaxWidth().clip(RoundedCornerShape(24.dp)).background(if (primary) colors.text else colors.controlFill)
            .then(if (enabled) Modifier.clickable(onClick = onClick) else Modifier).padding(vertical = 14.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = (if (primary) colors.background else colors.text).copy(alpha = if (enabled) 1f else 0.45f), fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 16.sp)
    }
}

@Composable
private fun Input(colors: sh.zeron.android.design.ZeronColors, value: String, placeholder: String, password: Boolean, onChange: (String) -> Unit) {
    BasicTextField(
        value = value,
        onValueChange = onChange,
        singleLine = true,
        visualTransformation = if (password) PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
        textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Mono, fontSize = 13.sp),
        cursorBrush = SolidColor(colors.accent),
        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(colors.controlFill).padding(horizontal = 14.dp, vertical = 12.dp),
        decorationBox = { inner ->
            Box {
                if (value.isEmpty()) Text(placeholder, color = colors.tertiary, fontFamily = ZeronType.Mono, fontSize = 13.sp)
                inner()
            }
        },
    )
}
