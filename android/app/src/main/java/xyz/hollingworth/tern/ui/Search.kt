package xyz.hollingworth.tern.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import xyz.hollingworth.tern.Store
import com.tern.engine.Action

private val TERMS: List<Pair<String, Color?>> = listOf("from:" to RuleFrom, "to:" to RuleTo, "subject:" to RuleSubject, "in:" to null, "has:attachment" to null)

@Composable
fun SearchScreen() {
    val searchView by Store.search.collectAsState()
    val view = searchView ?: return
    val colors = MaterialTheme.colorScheme
    var field by remember(view.revision) { mutableStateOf(TextFieldValue(view.query, TextRange(view.query.length))) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(view.revision) { runCatching { focus.requestFocus() } }
    fun change(value: TextFieldValue) {
        val changed = value.text != field.text
        field = value
        if (changed) Store.send(Action.SearchText(value.text))
    }
    Column(Modifier.fillMaxSize().imePadding()) {
        Row(
            Modifier.statusBarsPadding().padding(start = 12.dp, end = 12.dp, top = 8.dp).fillMaxWidth().height(56.dp).background(colors.surfaceContainer, RoundedCornerShape(28.dp)).padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconAction(Icons.Back, "Close search") { Store.send(Action.Back) }
            Box(Modifier.weight(1f)) {
                if (field.text.isEmpty()) Text("Search mail", color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodyLarge)
                BasicTextField(
                    field,
                    ::change,
                    Modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = "Search mail" },
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = colors.onSurface),
                    singleLine = true,
                    cursorBrush = SolidColor(colors.primary),
                )
            }
            if (field.text.isNotEmpty()) IconAction(Icons.Close, "Clear") { change(TextFieldValue("")) }
        }
        Row(Modifier.padding(horizontal = 12.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TERMS.forEach { (term, tone) ->
                Box(
                    Modifier.height(48.dp).clickable {
                        val joined = if (field.text.isEmpty() || field.text.endsWith(" ")) field.text + term else "${field.text} $term"
                        change(TextFieldValue(joined, TextRange(joined.length)))
                    },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        term,
                        Modifier.border(1.dp, colors.outline, RoundedCornerShape(8.dp)).padding(horizontal = 12.dp, vertical = 6.dp),
                        tone ?: colors.onSurface,
                        style = MaterialTheme.typography.labelLarge,
                    )
                }
            }
        }
        Text(view.note, Modifier.padding(start = 20.dp, end = 20.dp, top = 4.dp, bottom = 8.dp), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        if (view.empty) {
            Box(Modifier.fillMaxWidth().padding(top = 64.dp), contentAlignment = Alignment.Center) {
                Text("No matches in the mail on this phone", color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodyLarge)
            }
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
            items(view.rows, key = { it.key }, contentType = { "row" }) { row ->
                ThreadRowItem(row, selecting = false, onOpen = { Store.send(Action.OpenThread(row.key, row.folderId)) }, onSelect = null, Modifier.animateItem())
            }
        }
    }
}
