package xyz.hollingworth.tern.ui

import android.graphics.BitmapFactory
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import xyz.hollingworth.tern.Store
import com.tern.engine.Action
import com.tern.engine.AnswerKind
import com.tern.engine.AttachmentView
import com.tern.engine.BlockKind
import com.tern.engine.BlockView
import com.tern.engine.CardView
import com.tern.engine.MailRef
import com.tern.engine.SpanView

@Composable
fun ConversationScreen(pane: Boolean) {
    val conversation by Store.conversation.collectAsState()
    val view = lastOf(conversation) ?: return
    var menu by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize()) {
        TopBar {
            if (!pane) IconAction(Icons.Back, "Back to the list") { Store.send(Action.Back) }
            androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            if (view.canArchive) IconAction(Icons.Archive, "Archive") { Store.send(Action.ConversationArchive) }
            IconAction(Icons.Trash, "Move to trash") { Store.send(Action.ConversationTrash) }
            Box {
                IconAction(Icons.More, "More") { menu = true }
                DropdownMenu(menu, { menu = false }) {
                    fun pick(action: Action) {
                        menu = false
                        Store.send(action)
                    }
                    DropdownMenuItem({ Text(if (view.pinned) "Unpin" else "Pin") }, { pick(Action.ConversationPin) })
                    DropdownMenuItem({ Text("Mark as unread") }, { pick(Action.ConversationUnread) })
                    DropdownMenuItem({ Text("Labels") }, { pick(Action.ConversationLabels) })
                    if (view.canSpam) DropdownMenuItem({ Text("Move to spam…") }, { pick(Action.ConversationSpam) })
                }
            }
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 96.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item(key = "subject", contentType = "subject") {
                Column(Modifier.padding(start = 4.dp, top = 4.dp, end = 4.dp, bottom = 4.dp)) {
                    Text(view.subject, style = MaterialTheme.typography.titleLarge)
                }
            }
            items(view.cards, key = { it.id.element + it.id.list }, contentType = { "card" }) { card -> MailCard(card) }
        }
    }
}

@Composable
private fun MailCard(card: CardView) {
    val colors = MaterialTheme.colorScheme
    val ring = if (card.current && card.expanded) Accent.copy(alpha = 0.7f) else colors.outlineVariant
    // Below this width the date goes under the sender and the answer buttons lose their icons.
    BoxWithConstraints(Modifier.fillMaxWidth()) {
    val roomy = maxWidth >= 360.dp
    Column(
        Modifier.fillMaxWidth().border(if (card.current && card.expanded) 2.dp else 1.dp, ring, RoundedCornerShape(12.dp)).animateContentSize(tween(200, easing = Emphasized)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(
            Modifier.fillMaxWidth().clickable { Store.send(Action.ToggleCard(card.id)) },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(Modifier.size(40.dp).background(Color(0xFF865E3C), CircleShape), contentAlignment = Alignment.Center) {
                Text(card.initials, color = Color.White, style = MaterialTheme.typography.labelLarge)
            }
            Column(Modifier.weight(1f)) {
                Text(card.from, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    if (card.expanded) card.recipients ?: card.address else card.snippet ?: card.address,
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.onSurfaceVariant,
                    maxLines = if (card.expanded) 2 else 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (!roomy) Text(card.date, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant, maxLines = 1)
            }
            if (roomy) Text(card.date, Modifier.widthIn(max = 104.dp), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant, maxLines = 2)
        }
        if (card.expanded) {
            if (card.loading) {
                Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), colors.primary, strokeWidth = 2.dp) }
            }
            card.snippet?.let { problem -> Text(problem, style = MaterialTheme.typography.bodyMedium, color = colors.error) }
            if (card.attachments.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    card.attachments.forEach { AttachmentChip(card.id, it) }
                }
            }
            if (card.blocks.isNotEmpty()) Body(card.id, card.blocks)
            card.notice?.let { notice ->
                Text(
                    notice,
                    Modifier.clickable(enabled = card.webUrl != null) { card.webUrl?.let { Store.send(Action.OpenLink(it)) } }.padding(vertical = 4.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = Accent,
                )
            }
            if (card.draft) {
                AnswerButton(Icons.Pen, "Continue writing", Modifier, true) { Store.send(Action.EditDraft(card.id)) }
            } else if (card.blocks.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AnswerButton(Icons.Reply, "Reply", Modifier.weight(1f), roomy) { Store.send(Action.Answer(card.id, AnswerKind.REPLY)) }
                    AnswerButton(Icons.ReplyAll, "Reply all", Modifier.weight(1.25f), roomy) { Store.send(Action.Answer(card.id, AnswerKind.REPLY_ALL)) }
                    AnswerButton(Icons.Forward, "Forward", Modifier.weight(1.15f), roomy) { Store.send(Action.Answer(card.id, AnswerKind.FORWARD)) }
                }
            }
        }
    }
    }
}

@Composable
private fun AnswerButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, modifier: Modifier, withIcon: Boolean, onClick: () -> Unit) {
    FilledTonalButton(
        onClick,
        modifier.height(48.dp),
        contentPadding = PaddingValues(horizontal = 4.dp),
        colors = ButtonDefaults.filledTonalButtonColors(containerColor = MaterialTheme.colorScheme.surfaceContainer, contentColor = MaterialTheme.colorScheme.onSurface),
    ) {
        if (withIcon) {
            Glyph(icon, null, size = 18.dp)
            androidx.compose.foundation.layout.Spacer(Modifier.width(6.dp))
        }
        Text(label, style = MaterialTheme.typography.labelLarge, maxLines = 1)
    }
}

@Composable
private fun AttachmentChip(mail: MailRef, attachment: AttachmentView) {
    val colors = MaterialTheme.colorScheme
    Row(
        Modifier.background(colors.surfaceContainer, RoundedCornerShape(12.dp)).heightIn(min = 48.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier.weight(1f, fill = false).clickable { Store.send(Action.OpenAttachment(mail, attachment.id)) }.padding(horizontal = 12.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Glyph(if (attachment.image) Icons.Image else Icons.Clip, null, RuleTo)
            Column {
                Text(attachment.name, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(attachment.size, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
            }
        }
        Box(Modifier.width(1.dp).height(48.dp).background(colors.outlineVariant))
        IconAction(Icons.Save, "Save ${attachment.name}") { Store.send(Action.SaveAttachment(mail, attachment.id)) }
    }
}

@Composable
private fun Body(mail: MailRef, blocks: List<BlockView>) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        blocks.forEach { block -> androidx.compose.runtime.key(block.index) { BlockItem(mail, block) } }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun BlockItem(mail: MailRef, block: BlockView) {
    val colors = MaterialTheme.colorScheme
    val haptics = LocalHapticFeedback.current
    val pickable = block.kind !is BlockKind.Rule
    var modifier: Modifier = Modifier.fillMaxWidth()
    if (block.picked) modifier = modifier.background(colors.primaryContainer.copy(alpha = 0.6f), RoundedCornerShape(8.dp))
    if (pickable) {
        modifier = modifier.combinedClickable(
            onClick = {},
            onLongClick = {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                Store.send(Action.TogglePick(mail, block.index))
            },
            onLongClickLabel = "Pick to quote in a reply",
            indication = null,
            interactionSource = null,
        )
    }
    Row(modifier.padding(start = (block.listDepth.toInt().coerceAtMost(6) * 12).dp)) {
        repeat(block.quoteDepth.toInt().coerceAtMost(6)) {
            Box(Modifier.padding(end = 8.dp).width(2.dp).height(IntrinsicLine).background(colors.outline))
        }
        block.marker?.let { marker -> Text(marker, Modifier.padding(end = 6.dp), style = MaterialTheme.typography.bodyLarge, color = colors.onSurfaceVariant) }
        when (val kind = block.kind) {
            is BlockKind.Text -> Text(remember(kind.spans) { styled(kind.spans) }, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
            is BlockKind.Heading -> Text(
                remember(kind.spans) { styled(kind.spans) },
                Modifier.weight(1f),
                style = if (kind.level <= 2u) MaterialTheme.typography.titleLarge else MaterialTheme.typography.titleMedium,
            )
            is BlockKind.Pre -> Text(
                kind.text,
                Modifier.weight(1f).background(colors.surfaceContainer, RoundedCornerShape(8.dp)).horizontalScroll(rememberScrollState()).padding(8.dp),
                fontFamily = Mono,
                style = MaterialTheme.typography.bodyMedium,
            )
            BlockKind.Rule -> Box(Modifier.weight(1f).height(1.dp).background(colors.outlineVariant))
            is BlockKind.Image -> Picture(kind.path, kind.alt, Modifier.weight(1f))
            is BlockKind.Table -> Column(Modifier.weight(1f).horizontalScroll(rememberScrollState()).border(1.dp, colors.outlineVariant, RoundedCornerShape(8.dp)).padding(4.dp)) {
                kind.rows.forEach { row ->
                    Row {
                        row.cells.forEach { cell ->
                            Text(
                                remember(cell.spans) { styled(cell.spans) },
                                Modifier.widthIn(min = 64.dp, max = 240.dp).padding(6.dp),
                                style = MaterialTheme.typography.bodyMedium,
                                fontWeight = if (cell.header) FontWeight.Bold else null,
                            )
                        }
                    }
                }
            }
        }
    }
}

private val IntrinsicLine = 22.dp

private fun styled(spans: List<SpanView>): AnnotatedString = buildAnnotatedString {
    for (span in spans) {
        val style = SpanStyle(
            fontWeight = if (span.bold) FontWeight.Bold else null,
            fontStyle = if (span.italic) FontStyle.Italic else null,
            textDecoration = when {
                span.underline && span.strike -> TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough))
                span.underline -> TextDecoration.Underline
                span.strike -> TextDecoration.LineThrough
                else -> null
            },
            fontFamily = if (span.code) Mono else null,
        )
        val link = span.link
        if (link != null) {
            pushLink(LinkAnnotation.Clickable(link, TextLinkStyles(SpanStyle(color = Accent, textDecoration = TextDecoration.Underline))) { Store.send(Action.OpenLink(link)) })
            withStyle(style) { append(span.text) }
            pop()
        } else {
            withStyle(style) { append(span.text) }
        }
    }
}

/** A picture the mail carries, read off the main thread and never bigger than the screen needs. */
@Composable
private fun Picture(path: String?, alt: String, modifier: Modifier) {
    val bitmap by produceState<ImageBitmap?>(null, path) {
        value = path?.let { file -> withContext(Dispatchers.IO) { decode(file) } }
    }
    val image = bitmap
    if (image != null) {
        Image(image, alt, modifier.fillMaxWidth().aspectRatio(image.width.toFloat() / image.height.coerceAtLeast(1)).heightIn(max = 480.dp), contentScale = ContentScale.Fit)
    } else if (alt.isNotEmpty()) {
        Text("[$alt]", modifier, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

private fun decode(path: String): ImageBitmap? = runCatching {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(path, bounds)
    var sample = 1
    while (bounds.outWidth / sample > 1600 || bounds.outHeight / sample > 1600) sample *= 2
    BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
}.getOrNull()
