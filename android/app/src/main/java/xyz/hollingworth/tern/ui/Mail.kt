package xyz.hollingworth.tern.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.pullToRefresh
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import xyz.hollingworth.tern.Store
import xyz.hollingworth.tern.core.Action
import xyz.hollingworth.tern.core.ConnectionKind
import xyz.hollingworth.tern.core.FolderView
import xyz.hollingworth.tern.core.ListState
import xyz.hollingworth.tern.core.ListView
import xyz.hollingworth.tern.core.SelectionView

/** The mailbox: a list with a folder menu on a phone; rail, list and conversation when unfolded. */
@Composable
fun MailScreen(wide: Boolean, conversationOpen: Boolean) {
    if (wide) {
        val list by Store.list.collectAsState()
        BoxWithConstraints(Modifier.fillMaxSize()) {
        // The list takes its share of the room beside the rail, and the mail being read the rest.
        val listWidth = ((maxWidth - 96.dp) * 0.4f).coerceIn(264.dp, 400.dp)
        Row(Modifier.fillMaxSize()) {
            FolderRail(list?.folders.orEmpty())
            Box(Modifier.width(listWidth).fillMaxHeight().border(0.5.dp, MaterialTheme.colorScheme.outlineVariant)) {
                ConversationList(wide = true, onMenu = {})
            }
            Box(Modifier.weight(1f).fillMaxHeight()) {
                AnimatedContent(
                    conversationOpen,
                    transitionSpec = { fadeIn(tween(200)) togetherWith fadeOut(tween(120)) },
                    label = "detail",
                ) { open ->
                    if (open) ConversationScreen(pane = true) else NothingOpen()
                }
            }
        }
        }
    } else {
        val drawer = rememberDrawerState(DrawerValue.Closed)
        val scope = rememberCoroutineScope()
        val list by Store.list.collectAsState()
        val shell by Store.shell.collectAsState()
        BackHandler(drawer.isOpen) { scope.launch { drawer.close() } }
        ModalNavigationDrawer(
            drawerState = drawer,
            drawerContent = {
                ModalDrawerSheet(drawerContainerColor = MaterialTheme.colorScheme.surfaceContainerHigh) {
                    FolderMenu(list?.folders.orEmpty(), shell?.address.orEmpty(), shell?.cleanupOffer) { scope.launch { drawer.close() } }
                }
            },
        ) {
            ConversationList(wide = false, onMenu = { scope.launch { drawer.open() } })
        }
    }
}

@Composable
private fun NothingOpen() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text("Pick a conversation", color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyLarge)
    }
}

@Composable
private fun FolderRail(folders: List<FolderView>) {
    val colors = MaterialTheme.colorScheme
    Column(
        Modifier.width(96.dp).fillMaxHeight().statusBarsPadding().navigationBarsPadding().padding(top = 8.dp, bottom = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        folders.forEach { folder ->
            Column(
                Modifier.width(88.dp).height(64.dp).clickable(role = Role.Tab) { Store.send(Action.OpenFolder(folder.id)) },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Box(
                    Modifier.size(56.dp, 32.dp).background(if (folder.selected) colors.secondaryContainer else colors.background, RoundedCornerShape(16.dp)),
                    contentAlignment = Alignment.Center,
                ) { Glyph(folder.icon.glyph, null, if (folder.selected) colors.onSecondaryContainer else colors.onSurface, 22.dp) }
                Text(folder.name, style = MaterialTheme.typography.labelMedium, maxLines = 1)
            }
        }
        Spacer(Modifier.weight(1f))
        IconAction(Icons.Settings, "Settings") { Store.send(Action.OpenSettings) }
    }
}

@Composable
private fun FolderMenu(folders: List<FolderView>, address: String, cleanup: String?, close: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    @Composable
    fun item(glyph: androidx.compose.ui.graphics.vector.ImageVector, name: String, selected: Boolean = false, note: String? = null, dot: Boolean = false, onClick: () -> Unit) {
        Row(
            Modifier.fillMaxWidth().height(56.dp).background(if (selected) colors.secondaryContainer else colors.surfaceContainerHigh, RoundedCornerShape(28.dp)).clickable(role = Role.Button) {
                onClick()
                close()
            }.padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box {
                Glyph(glyph, null, if (selected) colors.onSecondaryContainer else colors.onSurface)
                if (dot) Box(Modifier.align(Alignment.TopEnd).size(8.dp).background(colors.primary, CircleShape))
            }
            Text(name, Modifier.weight(1f), if (selected) colors.onSecondaryContainer else colors.onSurface, style = MaterialTheme.typography.labelLarge, maxLines = 1)
            note?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = if (selected) colors.onSecondaryContainer else colors.onSurfaceVariant) }
        }
    }
    Column(Modifier.fillMaxHeight().statusBarsPadding().navigationBarsPadding().padding(12.dp)) {
        Column(Modifier.padding(start = 12.dp, top = 12.dp, bottom = 16.dp)) {
            Text("Tern", style = MaterialTheme.typography.titleMedium)
            Text(address, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        }
        folders.forEach { f ->
            item(f.icon.glyph, f.name, f.selected, f.unread.takeIf { it > 0u }?.toString()) { Store.send(Action.OpenFolder(f.id)) }
        }
        Spacer(Modifier.weight(1f))
        Box(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp).height(1.dp).background(colors.outlineVariant))
        item(Icons.Refresh, "Check for new mail") { Store.send(Action.Refresh) }
        if (cleanup != null) item(Icons.Broom, "Clean up", note = cleanup, dot = true) { Store.send(Action.OpenCleanup) }
        item(Icons.Settings, "Settings") { Store.send(Action.OpenSettings) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ConversationList(wide: Boolean, onMenu: () -> Unit) {
    val listView by Store.list.collectAsState()
    val shell by Store.shell.collectAsState()
    val list = listView ?: return
    val pull = rememberPullToRefreshState()
    val selection = list.selection
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            AnimatedContent(
                selection != null,
                transitionSpec = { fadeIn(tween(120)) togetherWith fadeOut(tween(90)) },
                label = "bar",
            ) { selecting ->
                if (selecting) {
                    SelectBar(list.selection?.count ?: 0u)
                } else {
                    TopBar {
                        if (wide) Spacer(Modifier.width(12.dp)) else IconAction(Icons.Menu, "Open folders", onClick = onMenu)
                        StatusSlot(list.title, Modifier.weight(1f), center = !wide)
                        IconAction(Icons.Search, "Search") { Store.send(Action.OpenSearch) }
                    }
                }
            }
            // The list follows the finger; on release it springs back and the header says "Fetching…".
            val offset = with(LocalDensity.current) { 72.dp.toPx() }
            Box(
                Modifier.fillMaxSize().pullToRefresh(
                    isRefreshing = false,
                    state = pull,
                    enabled = selection == null,
                    onRefresh = { Store.send(Action.Refresh) },
                ),
            ) {
                Box(Modifier.fillMaxSize().graphicsLayer { translationY = pull.distanceFraction.coerceAtMost(1.4f) * offset }) {
                    ListBody(list)
                }
                PullDisc({ pull.distanceFraction }, Modifier.align(Alignment.TopCenter))
            }
        }
        AnimatedVisibility(
            selection == null,
            Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(16.dp),
            enter = scaleIn(tween(160)) + fadeIn(tween(120)),
            exit = scaleOut(tween(120)) + fadeOut(tween(90)),
        ) {
            ExtendedFloatingActionButton(
                onClick = { Store.send(Action.ComposeNew) },
                modifier = Modifier.semantics { contentDescription = "New message" },
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
                shape = RoundedCornerShape(16.dp),
                icon = { Glyph(Icons.Pen, null, MaterialTheme.colorScheme.onPrimary) },
                text = { Text("New", style = MaterialTheme.typography.titleMedium) },
            )
        }
        AnimatedVisibility(
            selection != null,
            Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 16.dp),
            enter = slideInVertically(tween(220, easing = Emphasized)) { it } + fadeIn(tween(120)),
            exit = slideOutVertically(tween(180, easing = Emphasized)) { it } + fadeOut(tween(120)),
        ) {
            SelectionToolbar(selection ?: lastSelection)
        }
        val current = shell
        if (current != null && current.status.kind == ConnectionKind.SIGNED_OUT) {
            TextButton(onClick = { Store.send(Action.SignInAgain) }, Modifier.align(Alignment.TopEnd).statusBarsPadding().padding(top = 12.dp, end = 52.dp)) {
                Text("Sign in", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            }
        }
    }
    selection?.let { lastSelection = it }
}

/** What the toolbar last acted on, so it does not change its icons as it leaves. */
private var lastSelection = SelectionView(1u, true, false)

@Composable
private fun StatusSlot(title: String, modifier: Modifier, center: Boolean) {
    val shell by Store.shell.collectAsState()
    StatusTitle(title, shell?.status, modifier, center)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PullDisc(fraction: () -> Float, modifier: Modifier) {
    val colors = MaterialTheme.colorScheme
    Box(
        modifier.graphicsLayer {
            val f = fraction()
            alpha = (f * 2.5f).coerceIn(0f, 1f)
            translationY = (f.coerceAtMost(1.2f) * 72.dp.toPx()) - 44.dp.toPx()
            rotationZ = f * 180f
        }.size(40.dp).background(colors.surfaceContainerHigh, CircleShape).border(1.dp, colors.outlineVariant, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        CircularProgressIndicator(progress = { fraction().coerceIn(0f, 1f) * 0.8f }, Modifier.size(24.dp), colors.primary, strokeWidth = 3.dp, trackColor = androidx.compose.ui.graphics.Color.Transparent)
    }
}

@Composable
private fun ListBody(list: ListView) {
    val colors = MaterialTheme.colorScheme
    when (list.state) {
        ListState.LOADING -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = colors.primary) }
        ListState.EMPTY, ListState.FAILED -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 120.dp, start = 32.dp, end = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            item { Text(list.emptyTitle, style = MaterialTheme.typography.titleMedium) }
            list.emptyDetail?.let { detail -> item { Text(detail, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant) } }
        }
        ListState.ROWS -> {
            val state = rememberLazyListState()
            val selecting = list.selection != null
            LaunchedEffect(state, list.more, list.rows.size) {
                if (!list.more) return@LaunchedEffect
                snapshotFlow { state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0 }.distinctUntilChanged().collect { last ->
                    if (last >= list.rows.size - 10) Store.send(Action.LoadMore)
                }
            }
            LazyColumn(Modifier.fillMaxSize(), state, contentPadding = PaddingValues(bottom = 112.dp)) {
                items(list.rows, key = { it.key }, contentType = { "row" }) { row ->
                    ThreadRowItem(
                        row,
                        selecting,
                        onOpen = { Store.send(Action.OpenThread(row.key, row.folderId)) },
                        onSelect = { Store.send(Action.ToggleSelect(row.key)) },
                        Modifier.animateItem(),
                    )
                }
                if (selecting.not() && list.more) item(key = "more", contentType = "more") { Spacer(Modifier.height(48.dp)) }
            }
        }
    }
}

@Composable
private fun SelectBar(count: UInt) {
    TopBar {
        IconAction(Icons.Close, "Clear selection") { Store.send(Action.ClearSelection) }
        Text("$count selected", Modifier.weight(1f).padding(start = 4.dp), style = MaterialTheme.typography.titleMedium)
        TextButton(onClick = { Store.send(Action.SelectAll) }) { Text("Select all", color = Accent, style = MaterialTheme.typography.labelLarge) }
    }
}

/** The floating toolbar from design B: acts on what is selected, and only then. */
@Composable
private fun SelectionToolbar(selection: SelectionView) {
    val colors = MaterialTheme.colorScheme
    val many = selection.count > 1u
    Row(
        Modifier.height(64.dp).background(colors.secondaryContainer, RoundedCornerShape(32.dp)).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        val tint = colors.onSecondaryContainer
        IconAction(if (selection.pin) Icons.Pin else Icons.Unpin, if (selection.pin) (if (many) "Pin all" else "Pin") else "Unpin", tint = tint) { Store.send(Action.SelectionPin) }
        IconAction(if (selection.markRead) Icons.Read else Icons.Unread, if (selection.markRead) "Mark as read" else "Mark as unread", tint = tint) { Store.send(Action.SelectionRead) }
        IconAction(Icons.Label, "Labels", tint = tint) { Store.send(Action.SelectionLabels) }
        IconAction(Icons.Spam, "Move to spam", tint = tint) { Store.send(Action.SelectionSpam) }
        IconAction(Icons.Trash, "Move to trash", tint = tint) { Store.send(Action.SelectionTrash) }
    }
}
