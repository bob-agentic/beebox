package com.beebox.android

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.TextView
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
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
    private lateinit var status: TextView

    /** Set when the next page to finish should become the start of history,
     *  so Back cannot walk into `about:blank` or into the previous session —
     *  which would load without the marker, and so without the sizing. */
    private var freshHistory = false

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
        status = findViewById(R.id.status)
        configure(web)

        // Back goes back in the page's own history before it leaves the app.
        // Stepping aside for the system is for one press only: left disabled,
        // Back skipped the page's history for good once the app returned from
        // the background (Android 12+ keeps a root activity alive on Back).
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (web.visibility == View.VISIBLE && web.canGoBack()) web.goBack()
                else {
                    isEnabled = false
                    onBackPressedDispatcher.onBackPressed()
                    isEnabled = true
                }
            }
        })

        findViewById<View>(R.id.scan).setOnClickListener { scan.launch(Unit) }
        findViewById<View>(R.id.paste).setOnClickListener { promptForLink() }

        // A restored activity goes by what it saved, even when that is
        // nothing: falling back to the launching intent brought a session the
        // user had disconnected from back to life after the process was
        // evicted, because the intent still carried the original link.
        val start = if (saved != null) saved.getString(KEY_URL) else fromIntent(intent)
        if (start != null) open(start) else showConnect()
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
    }

    // ---- connecting ----------------------------------------------------

    /** The system camera hands over the code it read, so it gets the same
     *  rule as the in-app scanner. */
    private fun fromIntent(intent: Intent?): String? =
        intent?.dataString?.let(Links::fromCode)

    private fun open(url: String) {
        current = url
        freshHistory = true
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
        val origin = Links.origin(Uri.parse(url))
        marker?.remove()
        marker = WebViewCompat.addDocumentStartJavaScript(
            web,
            "window.__BEEBOX_APP__ = 'android';",
            setOf(origin),
        )
    }

    /** Back to the connect screen, saying why when there is a reason. A
     *  failure that only blanked the page left the user nowhere: the page's
     *  own way out is part of the page. */
    private fun showConnect(why: String? = null) {
        web.visibility = View.GONE
        connect.visibility = View.VISIBLE
        status.text = why
        status.visibility = if (why == null) View.GONE else View.VISIBLE
    }

    private fun promptForLink() {
        LinkDialog.show(this) { typed ->
            val url = Links.fromTyped(typed)
            if (url != null) open(url) else showConnect(getString(R.string.bad_link))
        }
    }

    /** Leaves the session. The page is unloaded rather than hidden, so its
     *  socket closes and the owner's connection list drops this device. */
    private fun leave(why: String? = null) {
        current = null
        freshHistory = true
        web.loadUrl("about:blank")
        showConnect(why)
    }

    /** The renderer died — reclaimed while backgrounded, or crashed. The
     *  WebView is unusable from here and must be replaced; without this the
     *  whole app went down with it. */
    private fun replaceWeb() {
        val parent = web.parent as ViewGroup
        val at = parent.indexOfChild(web)
        val params = web.layoutParams
        parent.removeView(web)
        web.destroy()
        marker = null // it belonged to the dead view
        web = TerminalWebView(this).also { it.id = R.id.web }
        parent.addView(web, at, params)
        configure(web)
        current?.let(::open) ?: showConnect()
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
            // Only the daemon is ever loaded. API 29 still defaults these on.
            allowFileAccess = false
            allowContentAccess = false
        }
        web.isVerticalScrollBarEnabled = false
        web.isHorizontalScrollBarEnabled = false
        // Leaving a session is the shell's to do: the page cannot show the
        // connect screen, because the connect screen is not part of the page.
        // Without this the only way out was killing the app from the
        // recents list.
        web.addJavascriptInterface(Bridge(), "__beeboxShell")

        // The page's console, in logcat. A WebView keeps it to itself
        // otherwise, which leaves `adb logcat` blind to anything the frontend
        // has to say about itself. Debug builds only: the source of every
        // message is the page URL, and that URL is the share token.
        web.webChromeClient = object : android.webkit.WebChromeClient() {
            override fun onConsoleMessage(m: android.webkit.ConsoleMessage): Boolean {
                if (BuildConfig.DEBUG) {
                    android.util.Log.i("BeeBoxWeb", "${m.message()} (${m.sourceId()}:${m.lineNumber()})")
                }
                return true
            }
        }

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
                    Links.fromCode(u.toString())?.let(::open)
                    return true
                }
                val here = current?.let { Uri.parse(it) }
                if (here != null && Links.origin(u) == Links.origin(here)) return false
                // Nothing may be installed to take it — a `mailto:` on a phone
                // with no mail app — and that must not crash the app.
                try {
                    startActivity(Intent(Intent.ACTION_VIEW, u))
                } catch (_: ActivityNotFoundException) {
                    Toast.makeText(this@MainActivity, R.string.no_handler, Toast.LENGTH_SHORT).show()
                }
                return true
            }

            override fun onPageFinished(view: WebView, url: String?) {
                if (freshHistory && url != "about:blank") {
                    view.clearHistory()
                    freshHistory = false
                }
            }

            override fun onReceivedError(
                view: WebView,
                request: WebResourceRequest,
                error: WebResourceError,
            ) {
                // Only the page itself failing is worth surfacing; a missing
                // favicon should not throw the user back to the scanner.
                if (request.isForMainFrame && current != null) {
                    leave(getString(R.string.unreachable, error.description))
                }
            }

            /** The daemon answering with an error is not a network error
             *  and never reached the handler above. Its body is a page meant
             *  for browsers, so only the status is read: 404 is how it
             *  answers any link it does not know, revoked or never made. */
            override fun onReceivedHttpError(
                view: WebView,
                request: WebResourceRequest,
                response: WebResourceResponse,
            ) {
                if (!request.isForMainFrame || current == null) return
                leave(
                    if (response.statusCode == 404) getString(R.string.link_gone)
                    else "HTTP ${response.statusCode}"
                )
            }

            override fun onRenderProcessGone(
                view: WebView,
                detail: RenderProcessGoneDetail,
            ): Boolean {
                if (view === web) replaceWeb()
                return true
            }
        }
    }

    /** What the page may ask the shell to do. Deliberately tiny: anything
     *  larger would be the page driving the app rather than living in it. */
    inner class Bridge {
        @android.webkit.JavascriptInterface
        fun disconnect() {
            runOnUiThread { leave() }
        }

        /** The link was disconnected while open. Back to the connect screen
         *  rather than a dead page with nothing to tap. */
        @android.webkit.JavascriptInterface
        fun linkGone() {
            runOnUiThread { leave(getString(R.string.link_gone)) }
        }
    }

    private companion object {
        const val KEY_URL = "beebox.url"
    }
}
