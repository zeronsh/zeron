package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.ExpandedFeedback
import sh.zeron.android.feedback.TapFeedback
import sh.zeron.android.feedback.play
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.DropdownMenuGroup
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenuPopup
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.foundation.BorderStroke
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

/** What the composer's trailing button does right now. */
enum class ComposerAction { Send, Queue, Stop }

/** Send / queue / stop: one control whose shape morphs on press. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ActionButton(action: ComposerAction, hasText: Boolean, onClick: () -> Unit) {
    val fb = LocalFeedback.current
    AnimatedContent(
        action == ComposerAction.Stop && !hasText,
        transitionSpec = { (scaleIn() + fadeIn()) togetherWith (scaleOut() + fadeOut()) },
        label = "action",
    ) { stop ->
        if (stop) {
            FilledTonalIconButton(
                onClick = {
                    fb.both(Haptic.Confirm, Cue.Close) // stopping a run: the falling cue
                    onClick()
                },
                shapes = IconButtonDefaults.shapes(),
                colors = IconButtonDefaults.filledTonalIconButtonColors(
                    containerColor = MaterialTheme.colorScheme.errorContainer,
                    contentColor = MaterialTheme.colorScheme.onErrorContainer,
                ),
                modifier = Modifier.size(40.dp),
            ) { sh.zeron.android.design.ZIcon(sh.zeron.android.design.ZIcons.Stop, "Stop", Modifier.size(18.dp)) }
        } else {
            FilledIconButton(
                onClick = {
                    // Send and steer sound the same; queueing behind a running turn is its own cue.
                    fb.both(Haptic.Confirm, if (action == ComposerAction.Queue) Cue.Queued else Cue.Send)
                    onClick()
                },
                enabled = hasText,
                shapes = IconButtonDefaults.shapes(),
                colors = IconButtonDefaults.filledIconButtonColors(
                    // White on the accent in both appearances, like iOS (the
                    // dark scheme's on-primary is a navy that muddies the arrow).
                    contentColor = Color.White,
                    disabledContainerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                    disabledContentColor = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f),
                ),
                modifier = Modifier.size(40.dp),
            ) { sh.zeron.android.design.ZIcon(sh.zeron.android.design.ZIcons.Send, if (action == ComposerAction.Queue) "Queue" else "Send", Modifier.size(22.dp)) }
        }
    }
}

/** A context chip in the composer toolbar; `menu` is its (lazily built) choice menu. */
@Composable
fun ContextChip(
    label: String,
    leading: @Composable () -> Unit,
    onClick: () -> Unit,
    tint: Color = MaterialTheme.colorScheme.onSurfaceVariant,
    menu: @Composable () -> Unit = {},
) {
    Box {
        Surface(
            onClick = tapAction(action = onClick),
            shape = RoundedCornerShape(50),
            color = chipContainer(),
            contentColor = tint,
        ) {
            Row(
                Modifier.heightIn(min = 34.dp).padding(start = 10.dp, end = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Box(Modifier.size(16.dp), contentAlignment = Alignment.Center) { leading() }
                Text(label, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        menu()
    }
}

class MenuChoice(
    val label: String,
    val selected: Boolean = false,
    val supporting: String? = null,
    val leading: (@Composable () -> Unit)? = null,
    val onClick: () -> Unit,
)

class MenuSection(val title: String?, val choices: List<MenuChoice>)

/** [haptic] / [cue] answer the choice in place of the default tap (null = the default). */
class MenuAction(
    val label: String,
    @androidx.annotation.DrawableRes val icon: Int,
    val destructive: Boolean = false,
    val haptic: Haptic? = null,
    val cue: Cue? = null,
    val onClick: () -> Unit,
)

/** An expressive action menu (one segmented group). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ActionMenu(expanded: Boolean, onDismiss: () -> Unit, actions: List<MenuAction>) {
    val fb = LocalFeedback.current
    ExpandedFeedback(expanded)
    DropdownMenuPopup(expanded = expanded, onDismissRequest = onDismiss) {
        DropdownMenuGroup(shapes = MenuDefaults.groupShape(0, 1)) {
            actions.forEachIndexed { i, a ->
                val tint = if (a.destructive) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface
                DropdownMenuItem(
                    onClick = {
                        // The item answers (its own feedback, else the default tap); the menu's close stays quiet.
                        val tap = fb as? TapFeedback
                        tap?.quietClose()
                        if (a.haptic != null || a.cue != null) fb.play(a.haptic, a.cue)
                        onDismiss()
                        a.onClick()
                        if (a.haptic == null && a.cue == null) tap?.defaultTap(0)
                    },
                    text = { Text(a.label, color = tint) },
                    shape = MenuDefaults.itemShape(i, actions.size).shape,
                    leadingIcon = { sh.zeron.android.design.ZIcon(a.icon, null, Modifier.size(20.dp), tint = tint) },
                )
            }
        }
    }
}
