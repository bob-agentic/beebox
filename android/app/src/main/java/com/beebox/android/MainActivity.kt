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
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
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

    /** The injected marker, kept so it can be replaced rather than stacked. */
    private var marker: androidx.webkit.ScriptHandler? = null

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

    /** The scheme exists only to be claimable by the camera; the daemon
     *  speaks http. Nothing else about the link is rewritten — what this
     *  client is gets said once, in the injected marker, not smuggled through
     *  every URL. */
    private fun httpFrom(uri: Uri): String =
        uri.buildUpon().scheme("http").build().toString()

    private fun open(url: String) {
        current = url
        announceSelf(url)
        connect.visibility = View.GONE
        web.visibility = View.VISIBLE
        web.loadUrl(url)
    }

    /** Says what this client is, before the page's bundle runs.
     *
     *  The page builds its socket URL at module scope — including whether it
     *  may drive the terminal's size — so a marker delivered after load would
     *  arrive too late to matter. The desktop shell does the same thing with
     *  `__BEEBOX__`.
     *
     *  The origin has to be exact: a wildcard rule that omits the host is
     *  rejected at runtime, so the rule is built per connection. The previous
     *  one is dropped first — the script is additive, and re-registering
     *  without clearing would stack a copy per session. */
    private fun announceSelf(url: String) {
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) return
        val uri = Uri.parse(url)
        val origin = "${uri.scheme}://${uri.host}:${uri.port.takeIf { it != -1 } ?: 80}"
        marker?.remove()
        marker = WebViewCompat.addDocumentStartJavaScript(
            web,
            "window.__BEEBOX_APP__ = 'android';",
            setOf(origin),
        )
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
        // Long-press is how you select text on a phone, and selecting output
        // is most of why it is on screen. The terminal draws to a canvas, so
        // there is no DOM text for the system to select — xterm implements
        // selection itself, from pointer events. WebView's own long-press
        // handler fires first and opens a menu about the app, which both
        // swallows the gesture and is useless here. Returning true says it is
        // handled; the pointer events still reach the page, which is the part
        // that matters.
        web.setOnLongClickListener { true }
        // Leaving a session is the shell's to do: the page cannot show the
        // connect screen, because the connect screen is not part of the page.
        // Without this the only way out was killing the app from the
        // recents list.
        web.addJavascriptInterface(Bridge(), "__beeboxShell")
        // Keeps the browser from also starting its own text selection on top
        // of the terminal's — two selection models fighting over one gesture.
        web.isHapticFeedbackEnabled = false

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

    /** What the page may ask the shell to do. Deliberately tiny: anything
     *  larger would be the page driving the app rather than living in it. */
    inner class Bridge {
        @android.webkit.JavascriptInterface
        fun disconnect() {
            runOnUiThread {
                current = null
                web.loadUrl("about:blank")
                showConnect()
            }
        }
    }

    private companion object {
        const val KEY_URL = "beebox.url"
    }
}
