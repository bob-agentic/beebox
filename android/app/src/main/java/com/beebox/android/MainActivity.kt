package com.beebox.android

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import androidx.annotation.ColorRes
import androidx.annotation.StringRes
import androidx.activity.OnBackPressedCallback
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.material.snackbar.Snackbar

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
    private lateinit var status: View
    private lateinit var recentTitle: View
    private lateinit var recentList: LinearLayout
    private lateinit var recents: Recents

    /** Set when the next page to finish should become the start of history,
     *  so Back cannot walk into `about:blank` or into the previous session. */
    private var freshHistory = false

    /** Where we last connected. A fold, a rotation, or the app being evicted
     *  should all come back to the same terminal rather than the scanner. */
    private var current: String? = null

    /** What the page says `current` shows. It can say so before the page has
     *  finished loading, so it waits here until the link is kept. */
    private var currentName: String? = null

    /** A daemon on the LAN answers in milliseconds. One that has not answered
     *  by now is on another network, and the WebView would otherwise wait out
     *  its own timeout — most of a minute — on a blank screen. */
    private val handler = Handler(Looper.getMainLooper())
    private val timeout = Runnable { unreachable() }

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
        recentTitle = findViewById(R.id.recent_title)
        recentList = findViewById(R.id.recent)
        recents = Recents(this)
        configure(web)

        // Back goes back in the page's own history before it leaves the app.
        // Stepping aside for the system is for one press only: left disabled,
        // Back skipped the page's history for good once the app returned from
        // the background (Android 12+ keeps a root activity alive on Back).
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (web.visibility == View.VISIBLE && web.canGoBack()) web.goBack()
                // Connecting or failed: Back gives up on the link.
                else if (status.visibility == View.VISIBLE) leave()
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

    /** Loads the link behind the status screen, which says so until the
     *  page has loaded. Invisible rather than gone, so the page is laid out
     *  at its real size from the start. */
    private fun open(url: String) {
        current = url
        currentName = null
        freshHistory = true
        web.loadUrl(url)
        showStatus(url, Tone.OK, null, R.string.connecting, labelOf(url),
            secondary = android.R.string.cancel to ::leave)
        handler.removeCallbacks(timeout)
        handler.postDelayed(timeout, TIMEOUT_MS)
    }

    /** The page is up: the link works, so it is worth remembering. */
    private fun opened(url: String) {
        handler.removeCallbacks(timeout)
        recents.add(url, currentName)
        status.visibility = View.GONE
        web.visibility = View.VISIBLE
    }

    /** Back to the connect screen. A failure that only blanked the page left
     *  the user nowhere: the page's own way out is part of the page. */
    private fun showConnect() {
        web.visibility = View.INVISIBLE
        status.visibility = View.GONE
        connect.visibility = View.VISIBLE
        showRecents()
    }

    /** One link, full screen: connecting when `mark` is null, else why it
     *  did not. The secondary action is always a way back. */
    private fun showStatus(
        url: String,
        tone: Tone,
        mark: String?,
        @StringRes title: Int,
        detail: String?,
        tips: Boolean = false,
        primary: Pair<Int, () -> Unit>? = null,
        secondary: Pair<Int, () -> Unit> = R.string.back to ::showConnect,
    ) {
        web.visibility = View.INVISIBLE
        connect.visibility = View.GONE
        status.visibility = View.VISIBLE
        findViewById<View>(R.id.status_icon).backgroundTintList = ContextCompat.getColorStateList(this, tone.disc)
        findViewById<View>(R.id.status_spin).visibility = if (mark == null) View.VISIBLE else View.GONE
        findViewById<TextView>(R.id.status_mark).apply {
            text = mark
            setTextColor(getColor(tone.mark))
        }
        findViewById<TextView>(R.id.status_title).setText(title)
        findViewById<TextView>(R.id.status_host).text = hostOf(url)
        findViewById<TextView>(R.id.status_detail).apply {
            text = detail
            visibility = if (detail == null) View.GONE else View.VISIBLE
        }
        findViewById<View>(R.id.status_tips).visibility = if (tips) View.VISIBLE else View.GONE
        findViewById<Button>(R.id.status_primary).apply {
            visibility = if (primary == null) View.GONE else View.VISIBLE
            primary?.let { (text, action) -> setText(text); setOnClickListener { action() } }
        }
        findViewById<Button>(R.id.status_secondary).apply {
            setText(secondary.first)
            setOnClickListener { secondary.second() }
        }
    }

    /** The link did not open, but may once the network or the owner allows:
     *  kept, and offered again right there. */
    private fun fail(tone: Tone, @StringRes title: Int, detail: String?, tips: Boolean = false) {
        val url = current ?: return
        stop()
        showStatus(url, tone, "!", title, detail, tips, primary = R.string.retry to { open(url) })
    }

    /** No answer at all — no route, no Wi-Fi, nothing listening — and
     *  `net::ERR_ADDRESS_UNREACHABLE` tells nobody what to do about it. */
    private fun unreachable() = fail(Tone.ERR, R.string.unreachable, null, tips = true)

    /** A snackbar for the one message that is not about a link on its way. */
    private fun say(text: String) = Snackbar.make(connect, text, Snackbar.LENGTH_LONG).show()

    private fun showRecents() {
        val list = recents.all()
        recentTitle.visibility = if (list.isEmpty()) View.GONE else View.VISIBLE
        recentList.removeAllViews()
        for (entry in list) {
            val row = LayoutInflater.from(this).inflate(R.layout.recent, recentList, false)
            row.findViewById<TextView>(R.id.name).text = label(entry.url, entry.name)
            row.findViewById<TextView>(R.id.link).text = entry.url
            row.setOnClickListener { open(entry.url) }
            row.findViewById<View>(R.id.forget).setOnClickListener {
                recents.remove(entry.url)
                showRecents()
            }
            recentList.addView(row)
        }
    }

    /** "Tab · tmp", or just "Tab" before the page has named it. */
    private fun label(url: String, name: String?): String =
        scopeOf(url).let { scope -> name?.let { getString(R.string.recent_name, scope, it) } ?: scope }

    private fun labelOf(url: String): String =
        label(url, recents.all().find { it.url == url }?.name)

    private fun hostOf(url: String): String = Uri.parse(url).let { u ->
        if (u.port == -1) u.host.orEmpty() else "${u.host}:${u.port}"
    }

    /** What the link opens, read off its path: the daemon files each scope
     *  under its own prefix, and the owner's address carries a key instead. */
    private fun scopeOf(url: String): String = getString(
        when (Uri.parse(url).pathSegments.firstOrNull()) {
            "a" -> R.string.scope_all
            "w" -> R.string.scope_workspace
            "t" -> R.string.scope_tab
            "p" -> R.string.scope_pane
            else -> R.string.scope_owner
        }
    )

    private fun promptForLink() {
        LinkDialog.show(this) { typed ->
            val url = Links.fromTyped(typed)
            if (url != null) open(url) else say(getString(R.string.bad_link))
        }
    }

    /** Leaves the session. The page is unloaded rather than hidden, so its
     *  socket closes and the owner's connection list drops this device. */
    private fun leave() {
        stop()
        showConnect()
    }

    private fun stop() {
        handler.removeCallbacks(timeout)
        current = null
        freshHistory = true
        web.loadUrl("about:blank")
    }

    /** The daemon has disowned the link. It will not work again, so it
     *  does not stay on offer. */
    private fun gone() {
        val url = current ?: return
        recents.remove(url)
        stop()
        showStatus(url, Tone.ERR, "✕", R.string.link_gone, getString(R.string.link_gone_detail),
            primary = R.string.scan_new to { scan.launch(Unit) })
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
            // Says this is the phone app, the one client that sets the
            // terminal's size (`is_phone_app` in core/src/http.rs). In the user
            // agent because every request carries it — the socket included, so
            // the daemon hears it, not only the page.
            userAgentString = "$userAgentString BeeBoxApp/android"
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

            // Not onPageCommitVisible: a hidden WebView draws no frames, so
            // that never comes while the connect screen covers it. A failed
            // load has already left by now and cleared `current`.
            override fun onPageFinished(view: WebView, url: String?) {
                if (view !== web || url == "about:blank") return
                if (freshHistory) {
                    view.clearHistory()
                    freshHistory = false
                }
                current?.let(::opened)
            }

            override fun onReceivedError(
                view: WebView,
                request: WebResourceRequest,
                error: WebResourceError,
            ) {
                // Only the page itself failing is worth surfacing; a missing
                // favicon should not throw the user back to the scanner.
                if (current == null || !request.isForMainFrame) return
                if (BuildConfig.DEBUG) android.util.Log.i("BeeBox", "load failed: ${error.errorCode} ${error.description}")
                // Every main-frame failure is the network's, and the link may
                // well work again once back on the right network.
                unreachable()
            }

            /** The daemon answering with an error is not a network error
             *  and never reached the handler above. Its body is a page meant
             *  for browsers, so only the status is read: 404 is how it
             *  answers any link it does not know, revoked or never made, and
             *  503 how it answers everyone while sharing is off. */
            override fun onReceivedHttpError(
                view: WebView,
                request: WebResourceRequest,
                response: WebResourceResponse,
            ) {
                if (current == null || !request.isForMainFrame) return
                when (val code = response.statusCode) {
                    404 -> gone()
                    503 -> fail(Tone.WARN, R.string.sharing_off, getString(R.string.sharing_off_detail))
                    else -> fail(Tone.ERR, R.string.http_error, getString(R.string.http_error_detail, code))
                }
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

        /** What the link shows, by name: `shareName` in App.svelte. */
        @android.webkit.JavascriptInterface
        fun named(name: String) {
            runOnUiThread {
                // Said again on every change to the tree; written only when new.
                if (name == currentName) return@runOnUiThread
                currentName = name
                // Only once kept: an entry is made when the page has loaded.
                current?.takeIf { web.visibility == View.VISIBLE }?.let { recents.rename(it, name) }
            }
        }

        /** The link was revoked while open. The status screen says so
         *  rather than leaving a dead page with nothing to tap. */
        @android.webkit.JavascriptInterface
        fun linkGone() {
            runOnUiThread { gone() }
        }
    }

    /** The status icon's colours: a mark on a dark disc of its own hue. */
    private enum class Tone(@ColorRes val mark: Int, @ColorRes val disc: Int) {
        OK(R.color.accent, R.color.accent_bg),
        WARN(R.color.warn, R.color.warn_bg),
        ERR(R.color.err, R.color.err_bg),
    }

    private companion object {
        const val KEY_URL = "beebox.url"
        const val TIMEOUT_MS = 10_000L
    }
}
