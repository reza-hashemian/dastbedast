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
import android.provider.Settings
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.MimeTypeMap
import android.webkit.WebView
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File

class MainActivity : TauriActivity() {
  private var multicast: WifiManager.MulticastLock? = null

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
  }

  override fun onDestroy() {
    try {
      multicast?.release()
    } catch (e: Exception) {
    }
    super.onDestroy()
  }

  override fun onWebViewCreate(webView: WebView) {
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
