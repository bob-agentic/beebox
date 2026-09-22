package com.beebox.android

import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.View
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.addCallback
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * A client, not a server. Everything runs on the daemon; this is the surface.
 *
 * The app holds one WebView pointed at a share URL. There is no address bar
 * and no navigation chrome, because the page already has all of it — the same
 * Svelte UI the desktop shell hosts. What this adds is what a browser tab
 * cannot: a scheme the system camera can hand a code to, a keyboard that has
 * an Esc key, and a window that keeps its session when you fold the phone.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var web: WebView
    private lateinit var connect: View

    /** Where we last connected. A fold, a rotation, or the app being evicted
     *  should all come back to the same terminal rather than the scanner. */
    private var current: String? = null

    private val scan = registerForActivityResult(ScanContract()) { url ->
        if (url != null) open(url)
    }

    override fun onCreate(saved: Bundle?) {
        super.onCreate(saved)
        setContentView(R.layout.main)

        // Android 15 forces edge-to-edge on targetSdk 35+, so without this the
        // page runs under the clock and under the navigation bar — the top bar
        // collided with the status icons and `daemon connected` sat beneath the
        // gesture pill. Inset the container, not the page: the WebView still
        // gets its whole rectangle verbatim, that rectangle just stops where
        // the system furniture begins.
        val root = findViewById<View>(R.id.root)
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime(),
            )
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            insets
        }

        web = findViewById(R.id.web)
        connect = findViewById(R.id.connect)
        configure(web)

        // Back goes back in the page's own history before it leaves the app.
        onBackPressedDispatcher.addCallback(this) {
            if (web.visibility == View.VISIBLE && web.canGoBack()) web.goBack()
            else {
                isEnabled = false
                onBackPressedDispatcher.onBackPressed()
            }
        }

        findViewById<View>(R.id.scan).setOnClickListener { scan.launch(Unit) }
        findViewById<View>(R.id.paste).setOnClickListener { promptForLink() }

        val saved_url = saved?.getString(KEY_URL) ?: fromIntent(intent)
        if (saved_url != null) open(saved_url) else showConnect()
    }

    /** A code scanned by the system camera arrives here, not through onCreate:
     *  launchMode is singleTask, so an already-running app is reused. */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        fromIntent(intent)?.let { open(it) }
    }

    override fun onSaveInstanceState(out: Bundle) {
        super.onSaveInstanceState(out)
        current?.let { out.putString(KEY_URL, it) }
        web.saveState(out)
    }


    // ---- connecting ----------------------------------------------------

    /** `beebox://host:port/t/token` is what a share code carries. The scheme
     *  exists only to be claimable; the daemon speaks http, so swap it back. */
    private fun fromIntent(intent: Intent?): String? {
        val data = intent?.data ?: return null
        return if (data.scheme == "beebox") httpFrom(data) else null
    }

    /** Turns a share URI into the URL to load: http, and asking the terminal
     *  to take its size from this screen.
     *
     *  The page gates that on `phone=1`, and it matters — a session laid out
     *  at 160 columns hard-wraps into nonsense on a 46-column phone. The
     *  desktop offers it as a checkbox because a desktop viewer may not want
     *  it; here there is nothing to ask. This app only ever runs on a phone,
     *  so it answers the question itself rather than making the link carry it. */
    private fun httpFrom(uri: Uri): String {
        val http = uri.buildUpon().scheme("http")
        // appendQueryParameter would happily add a second copy; a URL that
        // already carries the flag is left alone.
        if (uri.getQueryParameter("phone") == null) http.appendQueryParameter("phone", "1")
        return http.build().toString()
    }

    private fun open(url: String) {
        current = url
        connect.visibility = View.GONE
        web.visibility = View.VISIBLE
        web.loadUrl(url)
    }

    private fun showConnect() {
        web.visibility = View.GONE
        connect.visibility = View.VISIBLE
    }

    private fun promptForLink() {
        LinkDialog.show(this) { typed ->
            // A typed link deserves the same sizing as a scanned one, so it
            // goes through httpFrom either way — which also normalises the
            // scheme when someone pastes the http form.
            open(httpFrom(Uri.parse(typed.trim())))
        }
    }

    // ---- the webview ---------------------------------------------------

    @SuppressLint("SetJavaScriptEnabled")
    private fun configure(web: WebView) {
        web.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            // The page is the app. Its own layout is already responsive, so
            // none of the browser's desktop-page heuristics should apply.
            useWideViewPort = false
            loadWithOverviewMode = false
            builtInZoomControls = false
            displayZoomControls = false
            textZoom = 100
            mediaPlaybackRequiresUserGesture = false
        }
        web.isVerticalScrollBarEnabled = false
        web.isHorizontalScrollBarEnabled = false
        // Same reasoning as the desktop shell: a native app does not offer to
        // select its own furniture. The terminal handles its own selection.
        web.isLongClickable = false
        web.setOnLongClickListener { true }

        web.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: WebResourceRequest,
            ): Boolean {
                val u = request.url
                // Stay inside for our own scheme and for the daemon we are on;
                // hand anything else to the system, so a link in a terminal
                // does not replace the session with a web page.
                if (u.scheme == "beebox") {
                    open(httpFrom(u))
                    return true
                }
                val here = current?.let { Uri.parse(it) }
                if (here != null && u.host == here.host && u.port == here.port) return false
                startActivity(Intent(Intent.ACTION_VIEW, u))
                return true
            }

            override fun onReceivedError(
                view: WebView,
                request: WebResourceRequest,
                error: WebResourceError,
            ) {
                // Only the page itself failing is worth surfacing; a missing
                // favicon should not throw the user back to the scanner.
                if (request.isForMainFrame) showConnect()
            }
        }
    }

    private companion object {
        const val KEY_URL = "beebox.url"
    }
}
