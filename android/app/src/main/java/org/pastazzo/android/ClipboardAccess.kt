package org.pastazzo.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.PersistableBundle
import android.system.Os
import androidx.core.content.FileProvider
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.UUID
import android.util.Base64

object ClipboardAccess {
    const val MAX_IMAGE = 25 * 1024 * 1024
    fun encode(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
    fun decode(value: String): ByteArray = Base64.decode(value, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)

    fun image(context: Context, uri: Uri, mime: String): JSONObject {
        require(mime.startsWith("image/")) { "Choose text or an image." }
        val data = context.contentResolver.openInputStream(uri)?.use { input ->
            val output = ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            var count = input.read(buffer)
            while (count >= 0) {
                require(output.size() + count <= MAX_IMAGE) { "Images must be at most 25 MB." }
                output.write(buffer, 0, count)
                count = input.read(buffer)
            }
            output.toByteArray()
        } ?: error("This image could not be read.")
        return JSONObject().put("kind", "image").put("mime", mime).put("data", encode(data))
    }

    fun paste(context: Context): JSONObject {
        val item = context.getSystemService(ClipboardManager::class.java).primaryClip?.getItemAt(0)
            ?: error("Copy text or an image first.")
        if (item.text != null) return JSONObject().put("text", item.text.toString())
        val uri = item.uri ?: error("Copy text or an image first.")
        return image(context, uri, context.contentResolver.getType(uri) ?: "")
    }

    fun copy(context: Context, item: JSONObject) {
        val clip = if (item.getString("kind") == "text") ClipData.newPlainText("Pastazzo", item.getString("text")) else {
            val directory = File(context.cacheDir, "clipboard").apply { mkdirs(); Os.chmod(path, 448) }
            directory.listFiles()?.sortedByDescending { it.lastModified() }?.drop(7)?.forEach { it.delete() }
            val mime = item.getString("mime")
            val extension = when (mime) { "image/jpeg" -> "jpg"; "image/gif" -> "gif"; "image/webp" -> "webp"; else -> "png" }
            val file = File(directory, "${UUID.randomUUID()}.$extension")
            file.writeBytes(decode(item.getString("data")))
            Os.chmod(file.path, 384)
            val uri = FileProvider.getUriForFile(context, "${context.packageName}.clipboard", file)
            ClipData.newUri(context.contentResolver, "Pastazzo", uri)
        }
        clip.description.extras = PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
        context.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
    }
}
