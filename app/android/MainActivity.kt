package app.dastbedast

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.view.KeyEvent
import android.view.View
import android.view.WindowManager
import android.webkit.JavascriptInterface
import android.webkit.MimeTypeMap
import android.webkit.WebView
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import org.json.JSONArray
import org.json.JSONObject

private const val PHOTO_REQUEST = 4711

class MainActivity : TauriActivity() {
  private var multicast: WifiManager.MulticastLock? = null
  private var web: WebView? = null
  private var shared = ""
  /** The remote is open: the volume buttons go to the page and the screen stays on. */
  @Volatile private var remoteKeys = false
  /** The photo being taken, and the device it is for. */
  private var photo: File? = null
  private var photoFor = ""

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    // The page starts below the status bar and ends above the navigation bar and the keyboard.
    val content = findViewById<View>(android.R.id.content)
    val night = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    content.setBackgroundColor(Color.parseColor(if (night) "#15252E" else "#FAFCFD"))
    val bars = WindowCompat.getInsetsController(window, window.decorView)
    bars.isAppearanceLightStatusBars = !night
    bars.isAppearanceLightNavigationBars = !night
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val types = WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      val inset = insets.getInsets(types)
      view.setPadding(inset.left, inset.top, inset.right, inset.bottom)
      WindowInsetsCompat.CONSUMED
    }
    // Without this lock most phones drop the announcements other devices send on the network.
    try {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      multicast = wifi.createMulticastLock("dastbedast").apply {
        setReferenceCounted(false)
        acquire()
      }
    } catch (e: Exception) {
    }
    // A restored activity is handed its old intent again; what it carried was dealt with the first time.
    if (savedInstanceState == null) keepShared(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    keepShared(intent)
  }

  /** Remembers what another app shared with this one, until the page takes it. */
  @Suppress("DEPRECATION")
  private fun keepShared(intent: Intent?) {
    if (intent == null || intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
    val uris = ArrayList<Uri>()
    when (intent.action) {
      Intent.ACTION_SEND -> intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)?.let { uris.add(it) }
      Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.let { uris.addAll(it) }
      else -> return
    }
    val files = JSONArray()
    // Only what another app lends through the system; a plain path could point at this app's own files.
    for (uri in uris) if (uri.scheme == "content") files.put(JSONObject().put("uri", uri.toString()).put("name", nameOf(uri)))
    val text = if (uris.isEmpty()) intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim() ?: "" else ""
    if (files.length() == 0 && text.isEmpty()) return
    synchronized(this) { shared = JSONObject().put("files", files).put("text", text).toString() }
    // If the page is already up it is told now; otherwise it asks when it starts.
    web?.let { it.post { it.evaluateJavascript("window.dbdShared && window.dbdShared()", null) } }
  }

  private fun nameOf(uri: Uri): String {
    try {
      contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) return c.getString(0) ?: ""
      }
    } catch (e: Exception) {
    }
    return ""
  }

  @Synchronized
  fun takeShared(): String {
    val s = shared
    shared = ""
    return s
  }

  override fun onDestroy() {
    try {
      multicast?.release()
    } catch (e: Exception) {
    }
    super.onDestroy()
  }

  fun remoteMode(on: Boolean) {
    runOnUiThread {
      remoteKeys = on
      if (on) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
      else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }
  }

  /** Opens the camera; the picture goes to the device [peer] once it is taken. */
  fun takePhoto(peer: String) {
    runOnUiThread {
      try {
        val dir = File(cacheDir, "photos").apply { mkdirs() }
        // Pictures from earlier visits were sent already.
        dir.listFiles()?.forEach { if (System.currentTimeMillis() - it.lastModified() > 86_400_000L) it.delete() }
        val stamp = SimpleDateFormat("yyyy-MM-dd HH.mm.ss", Locale.US).format(Date())
        val file = File(dir, "photo $stamp.jpg")
        val uri = FileProvider.getUriForFile(this, "$packageName.fileprovider", file)
        val intent = Intent(MediaStore.ACTION_IMAGE_CAPTURE)
          .putExtra(MediaStore.EXTRA_OUTPUT, uri)
          .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        photo = file
        photoFor = peer
        @Suppress("DEPRECATION")
        startActivityForResult(intent, PHOTO_REQUEST)
      } catch (e: Exception) {
        photo = null
      }
    }
  }

  @Deprecated("Deprecated in Java")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    @Suppress("DEPRECATION")
    super.onActivityResult(requestCode, resultCode, data)
    if (requestCode != PHOTO_REQUEST) return
    val file = photo
    photo = null
    if (resultCode != RESULT_OK || file == null || !file.exists() || file.length() == 0L) return
    val js = "window.dbdPhoto && window.dbdPhoto(" + JSONObject.quote(photoFor) + ", " + JSONObject.quote(file.absolutePath) + ")"
    web?.let { it.post { it.evaluateJavascript(js, null) } }
  }

  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    val key = when (event.keyCode) {
      KeyEvent.KEYCODE_VOLUME_UP -> "up"
      KeyEvent.KEYCODE_VOLUME_DOWN -> "down"
      else -> null
    }
    val page = web
    if (!remoteKeys || key == null || page == null) return super.dispatchKeyEvent(event)
    // Both the press and the release are kept from the system, so the phone's own volume does not change.
    if (event.action == KeyEvent.ACTION_DOWN) {
      page.evaluateJavascript("window.dbdKey && window.dbdKey('$key', ${event.repeatCount > 0})", null)
    }
    return true
  }

  override fun onWebViewCreate(webView: WebView) {
    web = webView
    webView.addJavascriptInterface(Bridge(this), "DbdAndroid")
  }
}

/** What the page can ask of the phone: `window.DbdAndroid` in the UI. */
class Bridge(private val activity: MainActivity) {
  /** Whether the app may read and write anywhere in shared storage. */
  @JavascriptInterface
  fun allFiles(): Boolean {
    if (Build.VERSION.SDK_INT >= 30) return Environment.isExternalStorageManager()
    return activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
  }

  /** What another app shared with this one, as JSON `{files: [{uri, name}], text}`; empty when there is nothing. Given out once. */
  @JavascriptInterface
  fun takeShared(): String = activity.takeShared()

  /** The remote is open (`true`) or closed: see [MainActivity.remoteMode]. */
  @JavascriptInterface
  fun remoteMode(on: Boolean) = activity.remoteMode(on)

  /** Takes a photo with the camera app and sends it to the paired device [peer]. */
  @JavascriptInterface
  fun takePhoto(peer: String) = activity.takePhoto(peer)

  @JavascriptInterface
  fun askAllFiles() {
    activity.runOnUiThread {
      if (Build.VERSION.SDK_INT >= 30) {
        try {
          activity.startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + activity.packageName)))
        } catch (e: Exception) {
          try {
            activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
          } catch (e2: Exception) {
          }
        }
      } else {
        activity.requestPermissions(arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE, Manifest.permission.READ_EXTERNAL_STORAGE), 1)
      }
    }
  }

  /** Opens a file with whatever app handles its type, or a folder in the phone's file manager. */
  @JavascriptInterface
  fun open(path: String, folder: Boolean): Boolean {
    return try {
      val file = File(path)
      val intent = Intent(Intent.ACTION_VIEW)
      if (folder || file.isDirectory) {
        val root = Environment.getExternalStorageDirectory().absolutePath
        if (!path.startsWith(root)) return false
        val inside = path.removePrefix(root).trim('/')
        val uri = DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", "primary:$inside")
        intent.setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR)
      } else {
        val uri = FileProvider.getUriForFile(activity, activity.packageName + ".fileprovider", file)
        val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase()) ?: "*/*"
        intent.setDataAndType(uri, type)
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      activity.startActivity(intent)
      true
    } catch (e: Exception) {
      false
    }
  }

  @JavascriptInterface
  fun openUrl(url: String): Boolean {
    if (!url.startsWith("http://") && !url.startsWith("https://")) return false
    return try {
      activity.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      true
    } catch (e: Exception) {
      false
    }
  }
}
