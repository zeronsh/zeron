package sh.zeron.android.ui

import androidx.annotation.DrawableRes
import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsTopHeight
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons

/** A round tonal icon button whose shape morphs on press. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun TonalCircleButton(
    @DrawableRes icon: Int,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    size: Dp = 48.dp,
    container: Color = MaterialTheme.colorScheme.surfaceContainerHigh,
    content: Color = MaterialTheme.colorScheme.onSurface,
) {
    FilledTonalIconButton(
        onClick = onClick,
        shapes = IconButtonDefaults.shapes(),
        colors = IconButtonDefaults.filledTonalIconButtonColors(containerColor = container, contentColor = content),
        modifier = modifier.size(size),
    ) { ZIcon(icon, contentDescription, Modifier.size(22.dp)) }
}

/** The big expressive page title with a status line and round actions. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ScreenHeader(title: String, subtitle: String?, modifier: Modifier = Modifier, actions: @Composable RowScope.() -> Unit = {}) {
    Row(
        modifier.fillMaxWidth().padding(start = 24.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.displaySmallEmphasized, color = MaterialTheme.colorScheme.onSurface)
            if (subtitle != null) {
                Spacer(Modifier.height(2.dp))
                Text(subtitle, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), content = actions)
    }
}

/** A filter pill: filled when selected, tonal otherwise, with an optional count. */
@Composable
fun Pill(label: String, selected: Boolean, onClick: () -> Unit, count: Int? = null, @DrawableRes icon: Int? = null) {
    val container by animateColorAsState(
        if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceContainerHigh,
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "pill",
    )
    val fg = if (selected) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface
    Surface(onClick = onClick, shape = RoundedCornerShape(50), color = container, contentColor = fg) {
        Row(
            Modifier.heightIn(min = 40.dp).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            icon?.let { ZIcon(it, null, Modifier.size(18.dp)) }
            Text(label, style = MaterialTheme.typography.labelLarge)
            if (count != null && count > 0) {
                Box(
                    Modifier
                        .clip(CircleShape)
                        .background(if (selected) fg.copy(alpha = 0.2f) else MaterialTheme.colorScheme.primary.copy(alpha = 0.14f))
                        .padding(horizontal = 7.dp, vertical = 1.dp),
                ) {
                    Text("$count", style = MaterialTheme.typography.labelMedium, color = if (selected) fg else MaterialTheme.colorScheme.primary)
                }
            }
        }
    }
}

class NavItem(val label: String, @DrawableRes val icon: Int, val selected: Boolean, val onClick: () -> Unit)

/** Bottom navigation as a floating capsule, inset from the screen edges. */
@Composable
fun FloatingNavBar(items: List<NavItem>, modifier: Modifier = Modifier, trailing: (@Composable () -> Unit)? = null) {
    Row(modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Surface(
            shape = RoundedCornerShape(36.dp),
            color = MaterialTheme.colorScheme.surfaceContainerHigh,
            shadowElevation = 0.dp,
            modifier = Modifier.weight(1f),
        ) {
            Row(Modifier.padding(6.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                for (item in items) {
                    val bg by animateColorAsState(
                        if (item.selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
                        MaterialTheme.motionScheme.defaultEffectsSpec(),
                        label = "nav",
                    )
                    Surface(
                        onClick = item.onClick,
                        shape = RoundedCornerShape(30.dp),
                        color = bg,
                        contentColor = if (item.selected) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.weight(1f),
                    ) {
                        Column(Modifier.padding(vertical = 10.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                            ZIcon(item.icon, null, Modifier.size(24.dp))
                            Spacer(Modifier.height(2.dp))
                            Text(item.label, style = if (item.selected) MaterialTheme.typography.labelMediumEmphasized else MaterialTheme.typography.labelMedium)
                        }
                    }
                }
            }
        }
        trailing?.let {
            Spacer(Modifier.width(10.dp))
            it()
        }
    }
}

/**
 * "New session" — the always-there primary action above the nav (the iOS
 * accessory bar), with the live summary of what needs attention.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun NewSessionBar(summary: String?, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(32.dp),
        color = MaterialTheme.colorScheme.primaryContainer,
        contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
        modifier = modifier.fillMaxWidth().padding(horizontal = 16.dp),
    ) {
        Row(Modifier.padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.size(48.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary),
                contentAlignment = Alignment.Center,
            ) { ZIcon(ZIcons.NewSession, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onPrimary) }
            Spacer(Modifier.width(14.dp))
            Text("New session", style = MaterialTheme.typography.titleMediumEmphasized, modifier = Modifier.weight(1f))
            if (summary != null) {
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(end = 12.dp)) {
                    Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
                    Spacer(Modifier.width(6.dp))
                    Text(summary, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

val FloatingChromePadding = PaddingValues(bottom = 176.dp)

/** The page tone behind the status bar once content scrolls under it. */
@Composable
fun BoxScope.StatusBarScrim(scrolled: Boolean) {
    val color by animateColorAsState(
        if (scrolled) MaterialTheme.colorScheme.background else Color.Transparent,
        label = "status-scrim",
    )
    Box(
        Modifier
            .align(Alignment.TopCenter)
            .fillMaxWidth()
            .windowInsetsTopHeight(androidx.compose.foundation.layout.WindowInsets.statusBars)
            .background(color),
    )
}
