package xyz.hollingworth.tern

import android.app.Application
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.util.Log
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import kotlin.concurrent.thread
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import com.tern.engine.Action
import com.tern.engine.ComposerView
import com.tern.engine.ConversationView
import com.tern.engine.Effect
import com.tern.engine.Engine
import com.tern.engine.EngineConfig
import com.tern.engine.EngineListener
import com.tern.engine.ListView
import com.tern.engine.Screen
import com.tern.engine.RuleEditorView
import com.tern.engine.RulesView
import com.tern.engine.SearchView
import com.tern.engine.SettingsView
import com.tern.engine.SheetView
import com.tern.engine.ShellView
import com.tern.engine.SignInView
import com.tern.engine.Update

/** What the engine last published for each part of the screen. Holds no state of its own. */
object Store {
    val shell = MutableStateFlow<ShellView?>(null)
    val signIn = MutableStateFlow<SignInView?>(null)
    val list = MutableStateFlow<ListView?>(null)
    val conversation = MutableStateFlow<ConversationView?>(null)
    val composer = MutableStateFlow<ComposerView?>(null)
    val search = MutableStateFlow<SearchView?>(null)
    val settings = MutableStateFlow<SettingsView?>(null)
    val rules = MutableStateFlow<RulesView?>(null)
    val ruleEditor = MutableStateFlow<RuleEditorView?>(null)
    val sheet = MutableStateFlow<SheetView?>(null)
    val effects = MutableSharedFlow<Effect>(extraBufferCapacity = 16, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    @Volatile
    private var engine: Engine? = null
    private val early = ConcurrentLinkedQueue<Action>()
    private val started = CountDownLatch(1)

    fun send(action: Action) {
        val ready = engine
        if (ready != null) ready.send(action) else early.add(action)
    }

    fun attach(ready: Engine) {
        engine = ready
        while (true) ready.send(early.poll() ?: break)
        started.countDown()
    }

    /** The engine, once it has started. For background threads only. */
    fun awaitEngine(): Engine {
        started.await()
        return engine!!
    }
}

private object Listener : EngineListener {
    override fun onUpdate(update: Update) {
        when (update) {
            is Update.Shell -> Store.shell.value = update.view
            is Update.SignIn -> Store.signIn.value = update.view
            is Update.List -> Store.list.value = update.view
            is Update.Conversation -> Store.conversation.value = update.view
            is Update.Composer -> Store.composer.value = update.view
            is Update.Search -> Store.search.value = update.view
            is Update.Settings -> Store.settings.value = update.view
            is Update.Rules -> Store.rules.value = update.view
            is Update.RuleEditor -> Store.ruleEditor.value = update.view
            is Update.Sheet -> Store.sheet.value = update.view
        }
    }

    override fun onEffect(effect: Effect) {
        if (effect is Effect.Log) Log.i("Tern", effect.message) else Store.effects.tryEmit(effect)
    }
}

class TernApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // Off the main thread: the first screen does not wait for the library to load.
        thread(name = "tern-start") {
            val config = EngineConfig(
                configDir = filesDir.resolve("config").path,
                cacheDir = noBackupFilesDir.resolve("data").path,
                certDirs = "/apex/com.android.conscrypt/cacerts:/system/etc/security/cacerts",
            )
            Store.attach(Engine(config, KeystoreSecrets(this), Listener))
        }
        watchNetwork()
        // The engine says how often to check. Not before it knows whether there is an account: a
        // process started just for a check must not cancel the checks.
        MainScope().launch {
            Store.shell.filterNotNull().filter { it.stack.last() != Screen.STARTING }.map { it.backgroundMinutes }.distinctUntilChanged().collect {
                scheduleChecks(this@TernApp, it)
            }
        }
    }

    private fun watchNetwork() {
        val connectivity = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val online = connectivity.getNetworkCapabilities(connectivity.activeNetwork)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true
        if (!online) Store.send(Action.NetworkChanged(false))
        connectivity.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = Store.send(Action.NetworkChanged(true))
            override fun onLost(network: Network) = Store.send(Action.NetworkChanged(false))
        })
    }
}
