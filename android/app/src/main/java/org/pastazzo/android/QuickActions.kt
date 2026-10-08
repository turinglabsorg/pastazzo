package org.pastazzo.android

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.Context
import android.content.ComponentName
import android.content.Intent
import android.net.Uri
import android.widget.RemoteViews

enum class ClipboardAction(val host: String) {
    Paste("paste"), History("history");
    val uri: Uri get() = Uri.parse("pastazzo://$host")
    companion object {
        fun parse(uri: Uri?): ClipboardAction? {
            if (uri == null || uri.scheme != "pastazzo" || uri.userInfo != null || uri.port != -1 ||
                uri.query != null || uri.fragment != null || uri.path !in listOf(null, "", "/")) return null
            return entries.firstOrNull { it.host == uri.host }
        }
    }
}

class PastazzoWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        ids.forEach { manager.updateAppWidget(it, views(context)) }
    }
    companion object {
        fun pendingIntent(context: Context, action: ClipboardAction): PendingIntent = PendingIntent.getActivity(
            context, action.ordinal + 1,
            Intent(context, MainActivity::class.java).apply {
                if (action == ClipboardAction.Paste) component = ComponentName(context, "${context.packageName}.PasteShortcut")
            }.setAction(Intent.ACTION_VIEW).setData(action.uri)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        fun views(context: Context) = RemoteViews(context.packageName, R.layout.pastazzo_widget).apply {
            setOnClickPendingIntent(R.id.widget_paste, pendingIntent(context, ClipboardAction.Paste))
            setOnClickPendingIntent(R.id.widget_history, pendingIntent(context, ClipboardAction.History))
        }
    }
}
