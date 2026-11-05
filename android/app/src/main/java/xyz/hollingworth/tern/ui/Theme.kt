package xyz.hollingworth.tern.ui

import android.os.Build
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import xyz.hollingworth.tern.R

val Sans = FontFamily(
    Font(R.font.noto_sans_regular, FontWeight.Normal),
    Font(R.font.noto_sans_medium, FontWeight.Medium),
    Font(R.font.noto_sans_bold, FontWeight.Bold),
)
val Mono = FontFamily(Font(R.font.noto_sans_mono))

/** Material's emphasized easing, for everything that moves between screens. */
val Emphasized = CubicBezierEasing(0.2f, 0f, 0f, 1f)

/** Colours for what a status line says beyond the scheme's roles. */
val Warning = Color(0xFFFFA348)
val Good = Color(0xFF8FF0A4)
val RuleFrom = Color(0xFF78AEED)
val RuleTo = Color(0xFFDC8ADD)
val RuleSubject = Color(0xFFFFA348)
val Accent = Color(0xFF78AEED)

private fun scheme(pureBlack: Boolean): ColorScheme {
    val ground = if (pureBlack) Color.Black else Color(0xFF1D1D20)
    val raised = if (pureBlack) Color(0xFF1C1C20) else Color(0xFF2D2D32)
    val sheet = if (pureBlack) Color(0xFF18181B) else Color(0xFF26262A)
    val line = if (pureBlack) Color(0xFF2A2A2E) else Color(0xFF3A3A40)
    return darkColorScheme(
        primary = Color(0xFF99C1F1),
        onPrimary = Color(0xFF0B2A52),
        primaryContainer = Color(0xFF2F4A6D),
        onPrimaryContainer = Color(0xFFD6E4FB),
        secondaryContainer = Color(0xFF2F4A6D),
        onSecondaryContainer = Color(0xFFD6E4FB),
        background = ground,
        onBackground = Color(0xFFF4F4F6),
        surface = ground,
        onSurface = Color(0xFFF4F4F6),
        onSurfaceVariant = Color(0xFFA8A8B0),
        surfaceContainer = raised,
        surfaceContainerHigh = sheet,
        surfaceContainerHighest = raised,
        surfaceContainerLow = sheet,
        surfaceVariant = raised,
        outline = Color(0xFF8A8A92),
        outlineVariant = line,
        inverseSurface = Color(0xFFE6E6EA),
        inverseOnSurface = Color(0xFF1D1D20),
        inversePrimary = Color(0xFF1C5FB4),
        error = Color(0xFFFFB4AB),
        onError = Color(0xFF5F1412),
    )
}

@Composable
fun TernTheme(pureBlack: Boolean, matchWallpaper: Boolean, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val colors = remember(pureBlack, matchWallpaper) {
        val base = scheme(pureBlack)
        if (matchWallpaper && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // The wallpaper's accents over Tern's grounds, so pure black stays black.
            val dynamic = dynamicDarkColorScheme(context)
            base.copy(
                primary = dynamic.primary,
                onPrimary = dynamic.onPrimary,
                primaryContainer = dynamic.primaryContainer,
                onPrimaryContainer = dynamic.onPrimaryContainer,
                secondaryContainer = dynamic.secondaryContainer,
                onSecondaryContainer = dynamic.onSecondaryContainer,
            )
        } else base
    }
    val type = remember {
        fun style(size: Int, line: Int, weight: FontWeight = FontWeight.Normal) = TextStyle(fontFamily = Sans, fontSize = size.sp, lineHeight = line.sp, fontWeight = weight)
        Typography(
            headlineSmall = style(24, 32, FontWeight.Bold),
            titleLarge = style(22, 28, FontWeight.Bold),
            titleMedium = style(16, 22, FontWeight.Bold),
            titleSmall = style(14, 20, FontWeight.Bold),
            bodyLarge = style(16, 24),
            bodyMedium = style(14, 20),
            bodySmall = style(12, 16),
            labelLarge = style(14, 20, FontWeight.Bold),
            labelMedium = style(12, 16, FontWeight.Medium),
            labelSmall = style(11, 16, FontWeight.Medium),
        )
    }
    MaterialTheme(colorScheme = colors, typography = type) {
        // Text and icons with no colour of their own read on the ground, not black on black.
        CompositionLocalProvider(LocalContentColor provides colors.onSurface, content = content)
    }
}

private val cache = HashMap<String, ImageVector>()

/** An icon from a stroke path, in the design's own hand. */
private fun stroke(d: String, width: Float = 1.8f): ImageVector = cache.getOrPut(d) {
    ImageVector.Builder(defaultWidth = 24.dp, defaultHeight = 24.dp, viewportWidth = 24f, viewportHeight = 24f).addPath(
        pathData = addPathNodes(d),
        stroke = SolidColor(Color.Black),
        strokeLineWidth = width,
        strokeLineCap = StrokeCap.Round,
        strokeLineJoin = StrokeJoin.Round,
    ).build()
}

object Icons {
    val Menu get() = stroke("M4 7h16M4 12h16M4 17h16")
    val Search get() = stroke("M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM21 21l-5-5")
    val Clip get() = stroke("M20 11.5l-8.2 8.2a5 5 0 0 1-7-7L13 4.4a3.3 3.3 0 0 1 4.7 4.7l-8.2 8.2a1.6 1.6 0 0 1-2.3-2.3l7.6-7.6")
    val Pen get() = stroke("M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4")
    val Back get() = stroke("M19 12H5M11 6l-6 6 6 6")
    val Archive get() = stroke("M4 5h16v4H4zM5 9v10h14V9M10 13h4")
    val Trash get() = stroke("M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13")
    val More get() = stroke("M12 5v.01M12 12v.01M12 19v.01", 2.6f)
    val Reply get() = stroke("M10 7l-6 5 6 5M4 12h10a6 6 0 0 1 6 6")
    val ReplyAll get() = stroke("M12 7l-6 5 6 5M6 12h9a5 5 0 0 1 5 5M8 7l-6 5 6 5")
    val Forward get() = stroke("M14 7l6 5-6 5M20 12H10a6 6 0 0 0-6 6")
    val Close get() = stroke("M6 6l12 12M18 6L6 18")
    val Send get() = stroke("M4 20l17-8L4 4l2 8zM6 12h7")
    val Inbox get() = stroke("M4 13l3-8h10l3 8v6H4zM4 13h5l1 3h4l1-3h5")
    val Spam get() = stroke("M12 3l9 16H3zM12 10v4M12 17v.01")
    val Save get() = stroke("M12 4v11M7 11l5 5 5-5M5 20h14")
    val Down get() = stroke("M6 9l6 6 6-6")
    val Next get() = stroke("M9 6l6 6-6 6")
    val Image get() = stroke("M4 5h16v14H4zM4 16l5-5 4 4 3-3 4 4")
    val Pin get() = stroke("M9 4h6l-1 6 3 4H7l3-4zM12 14v6")
    val Unpin get() = stroke("M9 4h6l-1 6 3 4H7l3-4zM12 14v6M4 4l16 16")
    val Unread get() = stroke("M4 6h16v12H4zM4 7l8 6 8-6")
    val Read get() = stroke("M4 9l8-5 8 5v10H4zM4 10l8 5 8-5")
    val Label get() = stroke("M4 12V5h7l9 9-7 7zM8 9v.01")
    val Check get() = stroke("M5 12l5 5 9-10", 3f)
    val Plus get() = stroke("M12 5v14M5 12h14", 2f)
    val Refresh get() = stroke("M20 11a8 8 0 0 0-14-4L4 9M4 4v5h5M4 13a8 8 0 0 0 14 4l2-2M20 20v-5h-5")
    val Broom get() = stroke("M14 4l6 6M12 6l6 6-6 8-8-6 8-8z")
    val Settings get() = stroke("M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19 12l2-1-2-4-2 1-2-1V4h-6v3L7 8 5 7l-2 4 2 1v2l-2 1 2 4 2-1 2 1v3h6v-3l2-1 2 1 2-4-2-1z")
    val Offline get() = stroke("M3 3l18 18M8 8a5 5 0 0 0-2 9h11M12 6a6 6 0 0 1 6 6 3.5 3.5 0 0 1 2 3", 2.2f)
    val Clock get() = stroke("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7v5l3 2", 2.2f)
    val Eye get() = stroke("M2 12s4-7 10-7 10 7 10 7-4 7-10 7-10-7-10-7zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z")
}

@Composable
fun Glyph(icon: ImageVector, description: String? = null, tint: Color = MaterialTheme.colorScheme.onSurface, size: Dp = 24.dp, modifier: Modifier = Modifier) {
    Icon(icon, description, modifier.size(size), tint)
}

val com.tern.engine.FolderIcon.glyph: ImageVector
    get() = when (this) {
        com.tern.engine.FolderIcon.INBOX -> Icons.Inbox
        com.tern.engine.FolderIcon.DRAFTS -> Icons.Pen
        com.tern.engine.FolderIcon.SENT -> Icons.Send
        com.tern.engine.FolderIcon.ARCHIVE -> Icons.Archive
        com.tern.engine.FolderIcon.SPAM -> Icons.Spam
        com.tern.engine.FolderIcon.TRASH -> Icons.Trash
        com.tern.engine.FolderIcon.FOLDER -> Icons.Archive
    }
