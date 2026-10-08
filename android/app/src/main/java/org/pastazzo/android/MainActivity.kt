package org.pastazzo.android

import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.Image
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import android.text.format.Formatter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.IntentCompat
import androidx.core.view.WindowCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.nio.ByteBuffer
import java.text.DateFormat
import java.util.Date
import java.util.UUID

class MainActivity : ComponentActivity() {
    lateinit var model: HistoryModel; private set
    private var pendingPaste = false
    private var pairingLink by mutableStateOf<String?>(null)
    private var pairingHost by mutableStateOf("")
    private var confirmPaste by mutableStateOf(false)
    private val scanner = registerForActivityResult(ScanContract()) { result -> result.contents?.let(::preparePair) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        model = ViewModelProvider(this)[HistoryModel::class.java]
        setContent {
            val dark = isSystemInDarkTheme()
            SideEffect {
                WindowCompat.getInsetsController(window, window.decorView).apply {
                    isAppearanceLightStatusBars = !dark
                    isAppearanceLightNavigationBars = !dark
                }
            }
            PastazzoTheme {
                PastazzoScreen(model, ::paste, { scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                    .setPrompt("Scan the QR in Pastazzo on your Mac").setBeepEnabled(false).setOrientationLocked(false)) }, ::pastePairingLink)
                if (confirmPaste) AlertDialog(onDismissRequest = { confirmPaste = false },
                    title = { Text("Paste your current clipboard?") },
                    text = { Text("This saves the text or image you copied and sends it to your connected devices.") },
                    confirmButton = { TextButton(onClick = { confirmPaste = false; pendingPaste = true }) { Text("Paste") } },
                    dismissButton = { TextButton(onClick = { confirmPaste = false }) { Text("Cancel") } })
                if (pairingLink != null) AlertDialog(onDismissRequest = { pairingLink = null },
                    title = { Text("Connect this phone?") },
                    text = { Text("Connect to $pairingHost, then confirm this phone on the Mac displaying the QR.") },
                    confirmButton = { TextButton(onClick = { val link = pairingLink; pairingLink = null; if (link != null) model.pair(link) }) { Text("Connect") } },
                    dismissButton = { TextButton(onClick = { pairingLink = null }) { Text("Cancel") } })
            }
        }
        if (savedInstanceState == null) handleIntent(intent)
        else pendingPaste = savedInstanceState.getBoolean("paste_pending")
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                while (true) { if (!model.busy) model.refresh(); delay(5_000) }
            }
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putBoolean("paste_pending", pendingPaste)
        super.onSaveInstanceState(outState)
    }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); handleIntent(intent) }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && pendingPaste) { pendingPaste = false; paste() }
    }

    private fun handleIntent(intent: Intent) {
        when (ClipboardAction.parse(intent.data)) {
            ClipboardAction.Paste -> {
                model.showHistory()
                if (intent.component?.className == "${packageName}.PasteShortcut") {
                    pendingPaste = true
                    if (hasWindowFocus()) { pendingPaste = false; paste() }
                } else confirmPaste = true
            }
            ClipboardAction.History -> model.showHistory()
            null -> if (intent.action == Intent.ACTION_VIEW && intent.data?.host == "pair") preparePair(intent.data.toString())
        }
        if (intent.action == Intent.ACTION_SEND) {
            lifecycleScope.launch {
                try {
                    val fields = withContext(Dispatchers.IO) {
                        if (intent.type?.startsWith("image/") == true) {
                            val uri = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
                                ?: intent.clipData?.getItemAt(0)?.uri ?: error("This image could not be read.")
                            ClipboardAccess.image(this@MainActivity, uri, contentResolver.getType(uri) ?: intent.type!!)
                        } else JSONObject().put("text", intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString() ?: error("Choose text or an image."))
                    }
                    val id = UUID.randomUUID()
                    fields.put("id", ClipboardAccess.encode(ByteBuffer.allocate(16).putLong(id.mostSignificantBits).putLong(id.leastSignificantBits).array()))
                    model.save(fields)
                } catch (e: Exception) { model.message = e.message }
            }
        }
    }

    private fun paste() {
        if (!hasWindowFocus()) { pendingPaste = true; return }
        lifecycleScope.launch {
            try {
                val fields = withContext(Dispatchers.IO) { ClipboardAccess.paste(this@MainActivity) }
                model.save(fields)
            } catch (e: Exception) { model.message = e.message }
        }
    }

    private fun pastePairingLink() {
        if (!hasWindowFocus()) return
        val value = getSystemService(ClipboardManager::class.java).primaryClip?.getItemAt(0)?.text?.toString()
        if (value == null) model.message = "Copy a pairing link from your Mac first." else preparePair(value)
    }

    private fun preparePair(link: String) {
        try {
            require(link.toByteArray().size <= 4096) { "This pairing link is too large." }
            val uri = Uri.parse(link)
            require(uri.scheme == "pastazzo" && uri.host == "pair") { "Scan a Pastazzo pairing QR." }
            val server = Uri.parse(ClipboardAccess.decode(uri.getQueryParameter("server") ?: "").toString(Charsets.UTF_8))
            require(server.scheme == "https" || (BuildConfig.DEBUG && server.scheme == "http" && server.host in listOf("127.0.0.1", "localhost"))) {
                "Use an HTTPS server for secure pairing."
            }
            require(!model.connected) { "This phone is already connected." }
            pairingHost = server.host ?: error("Invalid server address.")
            pairingLink = link
        } catch (e: Exception) { model.message = e.message ?: "Invalid pairing QR." }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PastazzoScreen(model: HistoryModel, paste: () -> Unit, scan: () -> Unit, pasteLink: () -> Unit) {
    var confirmClear by remember { mutableStateOf(false) }
    var confirmDisconnect by remember { mutableStateOf(false) }
    Scaffold(topBar = {
        TopAppBar(title = {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Image(painterResource(R.drawable.pastazzo_mark), contentDescription = null, modifier = Modifier.size(36.dp))
                Text(if (model.settings) "Settings" else "Pastazzo")
            }
        }, actions = { TextButton(onClick = { model.settings = !model.settings }) { Text(if (model.settings) "Done" else "Settings") } })
    }) { insets ->
        if (model.settings) LazyColumn(Modifier.fillMaxSize().padding(insets), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
            item {
                Text("Sync", style = MaterialTheme.typography.headlineSmall)
                Spacer(Modifier.height(12.dp))
                Text(if (model.connected) "Connected as ${model.device}" else "Connect this phone to your clipboard network.")
                if (!model.connected) {
                    Spacer(Modifier.height(16.dp))
                    Button(onClick = scan, enabled = !model.busy, colors = ButtonDefaults.buttonColors(containerColor = Citrus, contentColor = CitrusInk), modifier = Modifier.fillMaxWidth()) { Text("Scan Mac QR") }
                    TextButton(onClick = pasteLink, enabled = !model.busy) { Text("Use a copied pairing link") }
                } else {
                    Spacer(Modifier.height(8.dp))
                    Text(model.server, style = MaterialTheme.typography.bodySmall)
                    Spacer(Modifier.height(12.dp))
                    Text("Account fingerprint", style = MaterialTheme.typography.labelMedium)
                    Text(model.fingerprint, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                    TextButton(onClick = { confirmDisconnect = true }, enabled = !model.busy) { Text("Disconnect this phone") }
                }
                model.approvalCode?.let {
                    Spacer(Modifier.height(16.dp))
                    Text("Confirm this code on your Mac")
                    Text(it, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.headlineSmall)
                }
            }
            item {
                Text("Clipboard", style = MaterialTheme.typography.headlineSmall)
                Spacer(Modifier.height(12.dp))
                Text("Tap Paste to send text or an image you copied. Received items stay in history until you tap Copy.")
                Spacer(Modifier.height(8.dp))
                Text("You can also share text and images to Pastazzo from other apps, or add the Paste & History widget to your Home Screen.")
                TextButton(onClick = { confirmClear = true }, enabled = !model.busy) { Text("Clear local history") }
            }
            model.message?.let { message -> item { StatusMessage(message) } }
        } else {
            val filtered = model.items.filter { item -> (model.filter == "all" || item.kind == model.filter) &&
                (model.search.isBlank() || item.preview.contains(model.search, true) || item.origin.contains(model.search, true)) }
            LazyColumn(Modifier.fillMaxSize().padding(insets), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                item {
                    Text("Your clipboard, together.", style = MaterialTheme.typography.headlineMedium)
                    Spacer(Modifier.height(8.dp))
                    Text(if (model.busy) "Working…" else if (model.connected) "${model.device} · Connected" else "${model.device} · Local history", style = MaterialTheme.typography.bodySmall)
                    Spacer(Modifier.height(16.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Button(onClick = paste, enabled = !model.busy, colors = ButtonDefaults.buttonColors(containerColor = Citrus, contentColor = CitrusInk), modifier = Modifier.weight(1f)) { Text("Paste") }
                        OutlinedButton(onClick = { model.refresh() }, enabled = !model.busy) { Text("Refresh") }
                    }
                    if (!model.connected) TextButton(onClick = { model.settings = true }) { Text("Connect with Mac QR") }
                }
                model.message?.let { message -> item { StatusMessage(message) } }
                item {
                    OutlinedTextField(model.search, { model.search = it }, label = { Text("Search history") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        listOf("all" to "All", "text" to "Text", "image" to "Images").forEach { (value, label) ->
                            FilterChip(selected = model.filter == value, onClick = { model.filter = value }, label = { Text(label) })
                        }
                    }
                }
                if (filtered.isEmpty()) item {
                    Spacer(Modifier.height(28.dp))
                    Text(if (model.items.isEmpty()) "A fresh shelf." else "No matching items.", style = MaterialTheme.typography.titleLarge)
                    Spacer(Modifier.height(8.dp))
                    Text(if (model.items.isEmpty()) "Copy something, then tap Paste. Clips from your connected devices will appear here." else "Try a different search or filter.")
                }
                items(filtered, key = { it.id }) { item ->
                    Card(shape = RoundedCornerShape(16.dp), modifier = Modifier.fillMaxWidth(), colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer)) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(item.origin, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                            Text(if (item.kind == "image") "Image · ${Formatter.formatShortFileSize(LocalContext.current, item.size.toLong())}" else item.preview,
                                maxLines = 5, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(item.createdAt)) +
                                    if (item.queued) " · Waiting to sync" else "", style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
                                TextButton(onClick = { model.copy(item) }, enabled = !model.busy) { Text("Copy") }
                            }
                        }
                    }
                }
            }
        }
    }
    if (confirmClear) AlertDialog(onDismissRequest = { confirmClear = false }, title = { Text("Clear local history?") },
        text = { Text("This removes this phone's history and cancels its queued uploads.") },
        confirmButton = { TextButton(onClick = { confirmClear = false; model.clear() }) { Text("Clear") } },
        dismissButton = { TextButton(onClick = { confirmClear = false }) { Text("Cancel") } })
    if (confirmDisconnect) AlertDialog(onDismissRequest = { confirmDisconnect = false }, title = { Text("Disconnect this phone?") },
        text = { Text("This revokes this phone's sync access. Its local history stays on this phone.") },
        confirmButton = { TextButton(onClick = { confirmDisconnect = false; model.disconnect() }) { Text("Disconnect") } },
        dismissButton = { TextButton(onClick = { confirmDisconnect = false }) { Text("Cancel") } })
}

@Composable
private fun StatusMessage(message: String) {
    Surface(color = MaterialTheme.colorScheme.secondaryContainer, shape = RoundedCornerShape(12.dp), modifier = Modifier.fillMaxWidth()) {
        Text(message, Modifier.padding(12.dp), style = MaterialTheme.typography.bodyMedium)
    }
}
