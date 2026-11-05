package xyz.hollingworth.tern.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.tern.engine.ConnectionKind
import com.tern.engine.RowView
import com.tern.engine.StatusView

@Composable
fun IconAction(icon: ImageVector, description: String, enabled: Boolean = true, tint: Color = MaterialTheme.colorScheme.onSurface, onClick: () -> Unit) {
    IconButton(onClick, enabled = enabled) { Glyph(icon, description, if (enabled) tint else tint.copy(alpha = 0.4f)) }
}

/** A title with the status line under it: the one place the connection shows. */
@Composable
fun StatusTitle(title: String, status: StatusView?, modifier: Modifier = Modifier, center: Boolean = true) {
    val colors = MaterialTheme.colorScheme
    val tone = when (status?.kind) {
        ConnectionKind.OFFLINE, ConnectionKind.BUSY -> Warning
        ConnectionKind.SIGNED_OUT -> colors.error
        else -> colors.onSurfaceVariant
    }
    Column(modifier, horizontalAlignment = if (center) Alignment.CenterHorizontally else Alignment.Start) {
        Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        val line = status?.line.orEmpty()
        if (line.isNotEmpty()) {
            Row(Modifier.semantics { liveRegion = LiveRegionMode.Polite }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                when (status?.kind) {
                    ConnectionKind.WORKING -> CircularProgressIndicator(Modifier.size(10.dp), tone, strokeWidth = 1.5.dp)
                    ConnectionKind.OFFLINE -> Glyph(Icons.Offline, null, tone, 14.dp)
                    ConnectionKind.BUSY -> Glyph(Icons.Clock, null, tone, 14.dp)
                    ConnectionKind.SIGNED_OUT -> Glyph(Icons.Spam, null, tone, 14.dp)
                    else -> {}
                }
                Text(line, style = MaterialTheme.typography.bodySmall, color = tone, maxLines = 1)
            }
        }
    }
}

/** A top bar with its own room for the status bar. */
@Composable
fun TopBar(modifier: Modifier = Modifier, content: @Composable RowScope.() -> Unit) {
    Row(modifier.fillMaxWidth().statusBarsPadding().height(64.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically, content = content)
}

@Composable
fun BackBar(title: String, status: StatusView? = null, onBack: () -> Unit, trailing: @Composable RowScope.() -> Unit = { Spacer(Modifier.width(48.dp)) }) {
    TopBar {
        IconAction(Icons.Back, "Back", onClick = onBack)
        StatusTitle(title, status, Modifier.weight(1f))
        trailing()
    }
}

/** The last view there was: a screen animating away keeps showing it after the engine lets go. */
@Composable
fun <T : Any> lastOf(view: T?): T? {
    val last = remember { arrayOfNulls<Any>(1) }
    if (view != null) last[0] = view
    @Suppress("UNCHECKED_CAST")
    return last[0] as T?
}

/** One conversation in a list, as on desktop: sender and date, then the subject, then its labels. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ThreadRowItem(row: RowView, selecting: Boolean, onOpen: () -> Unit, onSelect: (() -> Unit)?, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val haptics = LocalHapticFeedback.current
    val fill = when {
        row.selected -> colors.primaryContainer.copy(alpha = 0.6f)
        row.`open` -> colors.surfaceContainer
        else -> Color.Transparent
    }
    Row(
        modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 1.dp).background(fill, RoundedCornerShape(12.dp)).combinedClickable(
            onClick = if (selecting && onSelect != null) onSelect else onOpen,
            onLongClick = onSelect?.let { select ->
                {
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    select()
                }
            },
        ).padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        if (row.selected) {
            Box(Modifier.padding(top = 2.dp, end = 12.dp).size(24.dp).background(colors.primary, CircleShape), contentAlignment = Alignment.Center) {
                Glyph(Icons.Check, "Selected", colors.onPrimary, 16.dp)
            }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (row.unread) Box(Modifier.size(8.dp).background(Accent, CircleShape))
                if (row.draft) Text("Draft", style = MaterialTheme.typography.bodyLarge, color = colors.error, fontWeight = FontWeight.Medium)
                // All the room goes to the sender and its pin, so the date stays at the edge.
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        row.who,
                        Modifier.weight(1f, fill = false),
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = if (row.unread) FontWeight.Bold else FontWeight.Medium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (row.pinned) Glyph(Icons.Pin, "Pinned", colors.onSurfaceVariant, 16.dp)
                }
                if (row.count > 1u) {
                    Text(row.count.toString(), Modifier.background(colors.outlineVariant, RoundedCornerShape(9.dp)).padding(horizontal = 7.dp), style = MaterialTheme.typography.bodySmall)
                }
                if (row.hasAttachments) Glyph(Icons.Clip, "Has attachment", colors.onSurfaceVariant, 16.dp)
                Text(row.date, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant, maxLines = 1)
            }
            Text(row.subject, style = MaterialTheme.typography.bodyMedium, color = if (row.unread) colors.onSurface else colors.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (row.labels.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    row.labels.take(3).forEach { label ->
                        Text(label, Modifier.background(colors.outlineVariant, RoundedCornerShape(6.dp)).padding(horizontal = 8.dp, vertical = 1.dp), style = MaterialTheme.typography.bodySmall, maxLines = 1)
                    }
                }
            }
        }
    }
}

@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(text, modifier.padding(start = 4.dp, bottom = 4.dp), style = MaterialTheme.typography.titleSmall)
}
