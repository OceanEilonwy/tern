package xyz.hollingworth.tern

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import xyz.hollingworth.tern.core.BackgroundResult

private const val WORK = "check-mail"
private const val CHANNEL = "new-mail"
private const val GROUP = "new-mail"
private const val SUMMARY_ID = 1
const val EXTRA_KEY = "conversation"
const val EXTRA_FOLDER = "folder"

/** Asks the system to wake the app this often, or not at all. What for is the engine's business. */
fun scheduleChecks(context: Context, minutes: UInt) {
    val manager = WorkManager.getInstance(context)
    if (minutes == 0u) {
        manager.cancelUniqueWork(WORK)
        return
    }
    val request = PeriodicWorkRequestBuilder<CheckMailWorker>(minutes.toLong(), TimeUnit.MINUTES)
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .build()
    manager.enqueueUniquePeriodicWork(WORK, ExistingPeriodicWorkPolicy.UPDATE, request)
}

/** The system's wake-up: the engine does the check and says what to tell the person. */
class CheckMailWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val result = Store.awaitEngine().backgroundCheck()
        showNotices(applicationContext, result)
        return if (result.retry) Result.retry() else Result.success()
    }
}

private fun manager(context: Context) = context.getSystemService(NotificationManager::class.java)

fun clearNotices(context: Context) = manager(context).cancelAll()

private fun ensureChannel(context: Context) {
    val channel = NotificationChannel(CHANNEL, "New mail", NotificationManager.IMPORTANCE_DEFAULT)
    channel.description = "New mail in the Inbox"
    manager(context).createNotificationChannel(channel)
}

private fun baseBuilder(context: Context) = Notification.Builder(context, CHANNEL)
    .setSmallIcon(R.drawable.ic_notification)
    .setColor(0xFF3584E4.toInt())
    .setCategory(Notification.CATEGORY_EMAIL)
    .setGroup(GROUP)
    .setAutoCancel(true)
    // Who wrote and about what stay off the lock screen until it is unlocked.
    .setVisibility(Notification.VISIBILITY_PRIVATE)

private fun showNotices(context: Context, result: BackgroundResult) {
    if (result.notices.isEmpty()) return
    ensureChannel(context)
    val notifications = manager(context)
    val hidden = baseBuilder(context).setContentTitle("New mail").build()
    fun open(key: String?, folder: String?, code: Int): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        if (key != null) intent.putExtra(EXTRA_KEY, key).putExtra(EXTRA_FOLDER, folder)
        return PendingIntent.getActivity(context, code, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }
    for (notice in result.notices) {
        val notification = baseBuilder(context)
            .setContentTitle(notice.who)
            .setContentText(notice.subject)
            .setStyle(Notification.BigTextStyle().bigText(notice.subject))
            .setWhen(notice.receivedMs)
            .setShowWhen(true)
            .setPublicVersion(hidden)
            .setContentIntent(open(notice.key, notice.folderId, notice.id))
            .build()
        notifications.notify(notice.id, notification)
    }
    result.summary?.let { summary ->
        val notification = baseBuilder(context)
            .setContentTitle(summary)
            .setGroupSummary(true)
            .setPublicVersion(hidden)
            .setContentIntent(open(null, null, SUMMARY_ID))
            .build()
        notifications.notify(SUMMARY_ID, notification)
    }
}
