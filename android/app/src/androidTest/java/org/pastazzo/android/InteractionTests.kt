package org.pastazzo.android

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.graphics.Bitmap
import android.widget.TextView
import android.widget.FrameLayout
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID
import java.io.ByteArrayOutputStream
import java.net.URL
import org.json.JSONObject
import com.journeyapps.barcodescanner.CaptureActivity

@RunWith(AndroidJUnit4::class)
class InteractionTests {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()
    @get:Rule val cameraPermission = GrantPermissionRule.grant(Manifest.permission.CAMERA)
    private fun ready() { compose.waitUntil(15000) { compose.activity.hasWindowFocus() && !compose.activity.model.busy } }
    private fun seed(): String {
        ready()
        val text = "Explicit Android clipboard ${UUID.randomUUID()}"
        compose.runOnIdle { compose.activity.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("QA", text)) }
        return text
    }
    private fun history(client: NativeClient) = runBlocking { client.call("history").getJSONArray("items") }

    @Test fun pasteRequiresAButtonAndCopyWritesOnlyAfterAnAction() {
        val text = seed()
        val model = compose.activity.model
        val count = model.items.size
        compose.onNodeWithText("Search history").assertExists()
        assertEquals(count, model.items.size)
        compose.onNodeWithText("Paste").performClick()
        compose.waitUntil(10000) { model.items.any { it.preview == text } && !model.busy }
        compose.onAllNodesWithText("Copy")[0].performClick()
        compose.waitUntil(10000) { model.message == "Copied." }
        compose.runOnIdle { assertEquals(text, compose.activity.getSystemService(ClipboardManager::class.java).primaryClip!!.getItemAt(0).text.toString()) }
    }

    @Test fun historyWidgetDoesNotImportTheClipboard() {
        seed()
        val client = compose.activity.model.client
        val before = history(client).length()
        compose.runOnIdle {
            val views = PastazzoWidget.views(compose.activity).apply(compose.activity, FrameLayout(compose.activity))
            assertEquals("History", views.findViewById<TextView>(R.id.widget_history).text.toString())
            views.findViewById<TextView>(R.id.widget_history).performClick()
        }
        compose.waitForIdle()
        assertEquals(before, history(client).length())
    }

    @Test fun pasteWidgetImportsThroughItsRealPendingIntent() {
        val text = seed()
        val client = compose.activity.model.client
        compose.runOnIdle {
            val views = PastazzoWidget.views(compose.activity).apply(compose.activity, FrameLayout(compose.activity))
            views.findViewById<TextView>(R.id.widget_paste).performClick()
        }
        compose.waitUntil(15000) {
            val items = history(client)
            (0 until items.length()).count { items.getJSONObject(it).getString("preview") == text } == 1
        }
    }

    @Test fun externalPasteLinkRequiresConfirmationAndInvalidUrlsAreRejected() {
        val text = seed()
        val client = compose.activity.model.client
        val before = history(client).length()
        compose.runOnIdle { compose.activity.startActivity(Intent(compose.activity, MainActivity::class.java)
            .setAction(Intent.ACTION_VIEW).setData(Uri.parse("pastazzo://paste"))) }
        compose.onNodeWithText("Paste your current clipboard?").assertExists()
        assertEquals(before, history(client).length())
        compose.onNodeWithText("Cancel").performClick()
        ready()
        compose.runOnIdle { compose.activity.startActivity(Intent(compose.activity, MainActivity::class.java)
            .setAction(Intent.ACTION_VIEW).setData(Uri.parse("pastazzo://paste"))) }
        compose.onNode(hasText("Paste") and hasAnyAncestor(isDialog())).performClick()
        compose.waitUntil(10000) {
            val items = history(client)
            (0 until items.length()).count { items.getJSONObject(it).getString("preview") == text } == 1
        }
        assertNull(ClipboardAction.parse(Uri.parse("pastazzo://paste?text=bad")))
        assertNull(ClipboardAction.parse(Uri.parse("pastazzo://paste/private")))
        assertNull(ClipboardAction.parse(Uri.parse("pastazzo://user@paste")))
        assertNull(ClipboardAction.parse(Uri.parse("https://history")))
    }

    @Test fun shareIntentImportsOnlyTheProvidedText() {
        val clipboardText = seed()
        val model = compose.activity.model
        val text = "Shared explicitly from another app"
        compose.runOnIdle { compose.activity.startActivity(Intent(compose.activity, MainActivity::class.java)
            .setAction(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)) }
        compose.waitUntil(10000) { model.items.any { it.preview == text } && !model.busy }
        compose.onNodeWithText(text).assertExists()
        assertFalse(model.items.any { it.preview == clipboardText })
    }

    @Test fun imagePasteCopyAndShareUsePrivateClipboardUris() {
        ready()
        val model = compose.activity.model
        val count = model.items.count { it.kind == "image" }
        val bitmap = Bitmap.createBitmap(2, 2, Bitmap.Config.ARGB_8888)
        val bytes = ByteArrayOutputStream().apply { bitmap.compress(Bitmap.CompressFormat.PNG, 100, this) }.toByteArray()
        compose.runOnIdle { ClipboardAccess.copy(compose.activity, JSONObject().put("kind", "image").put("mime", "image/png").put("data", ClipboardAccess.encode(bytes))) }
        compose.onNodeWithText("Paste").performClick()
        compose.waitUntil(10000) { model.items.count { it.kind == "image" } == count + 1 && !model.busy }
        compose.onAllNodesWithText("Copy")[0].performClick()
        compose.waitUntil(10000) { model.message == "Copied." }
        compose.runOnIdle {
            val clipboard = compose.activity.getSystemService(ClipboardManager::class.java).primaryClip!!
            assertTrue(clipboard.description.extras!!.getBoolean("android.content.extra.IS_SENSITIVE"))
            val uri = clipboard.getItemAt(0).uri
            assertEquals("${compose.activity.packageName}.clipboard", uri.authority)
            assertArrayEquals(bytes, compose.activity.contentResolver.openInputStream(uri)!!.use { it.readBytes() })
            compose.activity.startActivity(Intent(compose.activity, MainActivity::class.java)
                .setAction(Intent.ACTION_SEND).setType("image/png").putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
        }
        compose.waitUntil(10000) { model.items.count { it.kind == "image" } == count + 2 && !model.busy }
        compose.onNodeWithText("Images").performClick()
        compose.onAllNodes(hasText("Image ·", substring = true))[0].assertIsDisplayed()
    }

    @Test fun scanActionOpensTheActualCameraScanner() {
        ready()
        compose.onNodeWithText("Settings").performClick()
        compose.onNodeWithText("Scan Mac QR").performClick()
        var scanner: CaptureActivity? = null
        compose.waitUntil(10000) {
            InstrumentationRegistry.getInstrumentation().runOnMainSync {
                scanner = ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(Stage.RESUMED)
                    .filterIsInstance<CaptureActivity>().firstOrNull()
            }
            scanner != null
        }
        InstrumentationRegistry.getInstrumentation().runOnMainSync { scanner!!.finish() }
        ready()
        assertTrue(compose.activity.model.settings)
    }

    @Test fun pairingLinkRequiresConfirmationAndConnectsThroughTheRealUi() {
        ready()
        val model = compose.activity.model
        val link = URL("http://127.0.0.1:32952/link").openConnection().apply {
            connectTimeout = 5000; readTimeout = 5000
        }.getInputStream().bufferedReader().use { JSONObject(it.readText()).getString("link") }
        compose.runOnIdle { compose.activity.startActivity(Intent(compose.activity, MainActivity::class.java)
            .setAction(Intent.ACTION_VIEW).setData(Uri.parse(link))) }
        compose.onNodeWithText("Connect this phone?").assertExists()
        assertFalse(model.connected)
        compose.onNodeWithText("Connect").performClick()
        compose.waitUntil(15000) { model.connected && !model.busy }
        assertTrue(model.fingerprint.isNotEmpty())
        assertEquals("http://127.0.0.1:32951", model.server)
        compose.onNodeWithText("Settings").performClick()
        compose.onNodeWithText("Disconnect this phone").performClick()
        compose.onNodeWithText("Disconnect this phone?").assertExists()
        compose.onNodeWithText("Disconnect").performClick()
        compose.waitUntil(10000) { !model.connected && !model.busy }
        assertFalse(runBlocking { model.client.call("status").getBoolean("logged_in") })
    }
}
