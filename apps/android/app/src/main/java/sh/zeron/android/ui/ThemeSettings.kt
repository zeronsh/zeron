package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AccentChoice
import sh.zeron.android.design.ThemeSeed
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronThemes
import sh.zeron.android.design.ZeronType

/**
 * Settings > Appearance: the desktop's "Light theme" / "Dark theme" pickers and its "Accent color"
 * row (Theme default + the seven presets), with the same names and colors.
 */
@Composable
internal fun ThemeSettingRows(model: ZeronModel, colors: ZeronColors, onPick: (dark: Boolean) -> Unit) {
    listOf(false to R.string.theme_light, true to R.string.theme_dark).forEach { (dark, label) ->
        val seed = ZeronThemes.seed(if (dark) model.themeDark else model.themeLight, dark)
        SettingRow(colors, stringResource(label), seed.name, onClick = { onPick(dark) }, trailing = { ThemePreview(seed) })
    }
    AccentRow(model, colors)
}

@Composable
private fun AccentRow(model: ZeronModel, colors: ZeronColors) {
    val themeId = if (colors.dark) model.themeDark else model.themeLight
    Column(
        Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated)
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Text(stringResource(R.string.accent_color), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp)
        Text(accentLabel(model.accent), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
        Spacer(Modifier.height(10.dp))
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            AccentChoice.entries.forEach { choice ->
                AccentSwatch(
                    color = ZeronThemes.swatch(choice, colors.dark, themeId),
                    themeDefault = choice == AccentChoice.THEME,
                    selected = model.accent == choice,
                    label = accentLabel(choice),
                    colors = colors,
                    onClick = { model.applyAccent(choice) },
                )
            }
        }
    }
}

@Composable
internal fun accentLabel(choice: AccentChoice): String =
    if (choice == AccentChoice.THEME) stringResource(R.string.accent_theme_default) else choice.label

@Composable
private fun AccentSwatch(color: Color, themeDefault: Boolean, selected: Boolean, label: String, colors: ZeronColors, onClick: () -> Unit) {
    Box(
        Modifier.size(36.dp).clip(CircleShape)
            .border(2.dp, if (selected) colors.text else Color.Transparent, CircleShape)
            .clickable(onClick = onClick)
            .semantics { contentDescription = label; this.selected = selected; role = Role.RadioButton }
            .testTag("accent-$label")
            .padding(4.dp),
        contentAlignment = Alignment.Center,
    ) {
        if (themeDefault) {
            // Theme default: the theme's own accent inside a hairline ring, so it reads as "automatic".
            Box(Modifier.size(28.dp).clip(CircleShape).border(1.5.dp, color, CircleShape), contentAlignment = Alignment.Center) {
                Box(Modifier.size(14.dp).clip(CircleShape).background(color))
            }
        } else {
            Box(Modifier.size(28.dp).clip(CircleShape).background(color))
        }
    }
}

/** A tiny window of the theme: page, a card, a text line and an accent dot. */
@Composable
internal fun ThemePreview(seed: ThemeSeed) {
    val (page, card, accent, text) = ZeronThemes.preview(seed)
    Box(
        Modifier.width(46.dp).height(30.dp).clip(RoundedCornerShape(7.dp)).background(page)
            .border(1.dp, text.copy(alpha = 0.14f), RoundedCornerShape(7.dp))
            .padding(5.dp),
    ) {
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(card).padding(horizontal = 4.dp, vertical = 3.dp)) {
            Box(Modifier.width(22.dp).height(3.dp).clip(RoundedCornerShape(2.dp)).background(text.copy(alpha = 0.85f)))
            Spacer(Modifier.height(3.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.width(14.dp).height(3.dp).clip(RoundedCornerShape(2.dp)).background(text.copy(alpha = 0.4f)))
                Spacer(Modifier.width(4.dp))
                Box(Modifier.size(5.dp).clip(CircleShape).background(accent))
            }
        }
    }
}

/** Bottom sheet listing every variant for one appearance, in the desktop registry's order. */
@Composable
internal fun ThemePickerSheet(model: ZeronModel, colors: ZeronColors, dark: Boolean, onDismiss: () -> Unit) {
    val current = ZeronThemes.seed(if (dark) model.themeDark else model.themeLight, dark).id
    BottomSheetFrame(colors, onDismiss, tag = "theme-picker") {
        Text(
            stringResource(if (dark) R.string.theme_dark else R.string.theme_light),
            color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 18.sp,
            modifier = Modifier.padding(bottom = 4.dp),
        )
        Text(
            stringResource(if (dark) R.string.theme_dark_sub else R.string.theme_light_sub),
            color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp,
            modifier = Modifier.padding(bottom = 10.dp),
        )
        ZeronThemes.variants(dark).forEach { seed ->
            val selected = seed.id == current
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
                    .background(if (selected) colors.accentSoft else Color.Transparent)
                    .clickable { model.applyTheme(dark, seed.id); onDismiss() }
                    .semantics { this.selected = selected }
                    .padding(horizontal = 10.dp, vertical = 9.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ThemePreview(seed)
                Spacer(Modifier.width(12.dp))
                Text(seed.name, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 15.sp, modifier = Modifier.weight(1f))
                if (selected) Text("✓", color = colors.accent, fontSize = 16.sp)
            }
        }
    }
}
