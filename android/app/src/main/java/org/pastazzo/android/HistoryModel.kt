package org.pastazzo.android

import android.app.Application
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File

data class HistoryItem(val id: String, val createdAt: Long, val origin: String, val kind: String,
    val preview: String, val size: Int, val queued: Boolean)

class HistoryModel(application: Application) : AndroidViewModel(application) {
    val client = NativeClient(application)
    var items by mutableStateOf<List<HistoryItem>>(emptyList()); private set
    var connected by mutableStateOf(false); private set
    var busy by mutableStateOf(false); private set
    var message by mutableStateOf<String?>(null)
    var approvalCode by mutableStateOf<String?>(null); private set
    var fingerprint by mutableStateOf(""); private set
    var server by mutableStateOf(""); private set
    var device by mutableStateOf(Settings.Global.getString(application.contentResolver, "device_name") ?: Build.MODEL); private set
    var search by mutableStateOf("")
    var filter by mutableStateOf("all")
    var settings by mutableStateOf(false)
    private val operations = Mutex()

    private suspend fun history() {
        val data = client.call("history").getJSONArray("items")
        items = (0 until data.length()).map { index -> data.getJSONObject(index).run {
            HistoryItem(getString("id"), getLong("created_at"), getString("origin"), getString("kind"),
                getString("preview"), getInt("size"), getBoolean("queued"))
        } }
    }

    private fun perform(block: suspend () -> Unit) = viewModelScope.launch {
        operations.withLock {
            busy = true
            try { block() } catch (e: Exception) { message = e.message ?: "The operation failed." }
            finally { busy = false }
        }
    }

    fun refresh(sync: Boolean = true) = perform {
        val status = client.call("status")
        connected = status.optBoolean("logged_in")
        if (connected) {
            device = status.getString("device_name")
            fingerprint = status.getString("account_fingerprint")
            server = status.getString("server_url")
            if (sync) {
                try { client.call("refresh"); message = null }
                catch (e: Exception) { message = "Could not sync: ${e.message}" }
            }
        }
        history()
    }

    fun save(fields: JSONObject) = perform {
        fields.put("name", device)
        val result = client.call("save", fields)
        message = if (result.optBoolean("queued")) "Saved. Sync will retry when you reconnect."
            else if (connected) "Saved and synced." else "Saved on this phone."
        history()
    }

    fun copy(item: HistoryItem) = perform {
        val content = client.call("item", JSONObject().put("id", item.id)).getJSONObject("item")
        ClipboardAccess.copy(getApplication(), content)
        message = "Copied."
    }

    fun pair(link: String) = perform {
        require(!connected) { "This phone is already connected." }
        message = null
        val monitor = viewModelScope.launch {
            while (true) {
                approvalCode = withContext(Dispatchers.IO) {
                    val file = File(client.root, "approval-code")
                    runCatching { file.readText() }.getOrNull()
                }
                delay(250)
            }
        }
        try {
            client.call("pair", JSONObject().put("link", link).put("name", device))
            connected = true
            message = "This phone is connected."
            settings = false
        } finally { monitor.cancel(); approvalCode = null }
        val status = client.call("status")
        fingerprint = status.getString("account_fingerprint")
        server = status.getString("server_url")
        history()
    }

    fun clear() = perform { client.call("clear_local"); history(); message = "Local history cleared." }
    fun disconnect() = perform {
        client.call("logout"); connected = false; fingerprint = ""; server = ""; message = "Disconnected."
    }
    fun showHistory() { settings = false; search = ""; filter = "all" }
}
