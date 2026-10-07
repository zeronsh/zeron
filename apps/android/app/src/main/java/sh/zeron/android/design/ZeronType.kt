package sh.zeron.android.design

import android.content.Context
import androidx.compose.ui.text.font.AndroidFont
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontLoadingStrategy
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import uniffi.zeron_core.FaceRole

/**
 * Geist, the same family the Rust core measures, resolved through [FontChain]
 * so Compose labels fall back to Noto Sans CJK SC exactly like the transcript.
 */
object ZeronType {
    val Sans = FontFamily(
        ChainFont(FaceRole.SANS, FontWeight.Normal),
        ChainFont(FaceRole.SANS_MEDIUM, FontWeight.Medium),
        ChainFont(FaceRole.SANS_SEMIBOLD, FontWeight.SemiBold),
        ChainFont(FaceRole.SANS_BOLD, FontWeight.Bold),
        ChainFont(FaceRole.SANS_ITALIC, FontWeight.Normal, FontStyle.Italic),
        ChainFont(FaceRole.SANS_MEDIUM_ITALIC, FontWeight.Medium, FontStyle.Italic),
        ChainFont(FaceRole.SANS_SEMIBOLD_ITALIC, FontWeight.SemiBold, FontStyle.Italic),
        ChainFont(FaceRole.SANS_BOLD_ITALIC, FontWeight.Bold, FontStyle.Italic),
    )
    val Mono = FontFamily(
        ChainFont(FaceRole.MONO, FontWeight.Normal),
        ChainFont(FaceRole.MONO_MEDIUM, FontWeight.Medium),
        ChainFont(FaceRole.MONO_SEMIBOLD, FontWeight.SemiBold),
        ChainFont(FaceRole.MONO_ITALIC, FontWeight.Normal, FontStyle.Italic),
    )
}

private object ChainLoader : AndroidFont.TypefaceLoader {
    override fun loadBlocking(context: Context, font: AndroidFont): android.graphics.Typeface =
        FontChain.face(context, (font as ChainFont).role)

    override suspend fun awaitLoad(context: Context, font: AndroidFont): android.graphics.Typeface =
        loadBlocking(context, font)
}

private class ChainFont(
    val role: FaceRole,
    override val weight: FontWeight,
    override val style: FontStyle = FontStyle.Normal,
) : AndroidFont(FontLoadingStrategy.Blocking, ChainLoader, FontVariation.Settings()) {
    override fun equals(other: Any?) = other is ChainFont && other.role == role
    override fun hashCode() = role.hashCode()
    override fun toString() = "ChainFont($role)"
}
