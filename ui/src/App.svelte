<script lang="ts">
  import { fade } from 'svelte/transition';
  import { store } from './lib/state.svelte';
  import Sidebar, { FOLD_MS } from './lib/components/Sidebar.svelte';
  import TabBar from './lib/components/TabBar.svelte';
  import SplitTree from './lib/components/SplitTree.svelte';
  import ShareDialog from './lib/components/ShareDialog.svelte';
  import ConnectionsDialog from './lib/components/ConnectionsDialog.svelte';
  import SettingsDialog from './lib/components/SettingsDialog.svelte';
  import OpenWorkspaceDialog from './lib/components/OpenWorkspaceDialog.svelte';
  import Icon from './lib/components/Icon.svelte';
  import KeyBar from './lib/components/KeyBar.svelte';

  let dialog = $state<'share' | 'conns' | 'settings' | 'openws' | null>(null);

  /** A fresh install — or one whose last workspace was just closed — lands with
      nothing open. The daemon no longer guesses a folder for us, so invite the
      owner to choose one instead of leaving them on a bare canvas. Fires once:
      `armed` is disarmed the moment a workspace exists and re-armed only when
      the app is empty again, so dismissing the dialog to look around does not
      immediately re-open it. */
  let armed = true;
  $effect(() => {
    if (store.needsWorkspacePrompt) {
      if (armed) {
        armed = false;
        dialog = 'openws';
      }
    } else {
      // A workspace opened (or we are a viewer): re-arm for next time it empties.
      armed = true;
    }
  });

  /** Running inside the desktop shell rather than a browser tab.
   *
   *  Evaluated once, at module scope — the shell must therefore install its
   *  bridge before this bundle runs (a document-start user script), not after
   *  the page loads. */
  const desktop = '__BEEBOX__' in window;

  /** The desktop shell's bridge, or undefined in a browser. */
  const shell = () => (window as any).__BEEBOX__;
  /** The Android shell's bridge. Separate from the desktop one: they share no
      methods, and conflating them would mean each having to answer for the
      other's. */
  const appShell = () => (window as any).__beeboxShell;

  /** Opening a workspace means choosing a folder — that is the whole model. Use
      the native folder chooser when the shell offers one; otherwise fall back
      to the typed-path dialog. Both ⌘N and the sidebar's ＋ land here: there is
      no longer an implicit "reopen the last folder" path, because deciding
      which project you want is the point, not ceremony to skip. */
  async function openWorkspace() {
    const picked = await store.pickDirectory();
    if (picked) store.send({ t: 'open_workspace', path: picked });
    // `undefined` means no chooser exists; `null` means it was cancelled.
    else if (picked === undefined) dialog = 'openws';
  }

  /** The title bar is drawn by this page, so these have to do the real work. */
  async function windowAction(what: 'close' | 'minimize' | 'zoom') {
    const w = shell();
    if (!w) return;
    if (what === 'close') await w.close();
    else if (what === 'minimize') await w.minimize();
    else await w.toggleMaximize();
  }

  /** Moves the window. CSS `app-region` does nothing in a WKWebView, so the
      shell does the dragging — this only decides that a drag was meant, and
      keeps it off the controls. */
  async function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const el = e.target as HTMLElement;
    if (el.closest('button, input, select, a')) return;

    const w = shell();
    if (typeof w?.startDragging !== 'function') return;
    e.preventDefault();
    await w.startDragging();
  }

  function titleDoubleClick(e: MouseEvent) {
    const el = e.target as HTMLElement;
    if (el.closest('button, input, select, a')) return;
    void windowAction('zoom');
  }

  /** The sidebar folds to a rail of initials — or, on a phone, to nothing.
      Which of those it means is left to CSS; this only remembers the choice.
      Until someone makes one, a narrow screen starts folded and a wide one
      open, decided afresh on each load. */
  const FOLD_KEY = 'beebox.sidebar-folded';
  let folded = $state(loadFolded());

  function loadFolded(): boolean {
    try {
      const saved = localStorage.getItem(FOLD_KEY);
      if (saved !== null) return saved === '1';
    } catch {
      // Private browsing; fall through to the width.
    }
    return matchMedia('(max-width: 560px)').matches;
  }

  function setFolded(on: boolean) {
    // The sidebar's slide. Terminals fit once it lands, not on every frame of
    // it — each fit would resize the PTY and make the shell repaint.
    store.holdFitFor(FOLD_MS + 20);
    folded = on;
    try {
      localStorage.setItem(FOLD_KEY, on ? '1' : '0');
    } catch {
      // Private browsing. It still folds for this session.
    }
  }

  const tab = $derived(store.activeTab);
  const paneCount = $derived(
    store.tree.workspaces.reduce(
      (n, w) => n + w.tabs.reduce((m, t) => m + t.panes.length, 0),
      0,
    ),
  );
  const tabCount = $derived(
    store.tree.workspaces.reduce((n, w) => n + w.tabs.length, 0),
  );

  /** Every shortcut's behaviour, by name.
   *
   *  Two things reach these: the keydown handler below, and the desktop
   *  shell's menu. They must not drift, because in the desktop shell the menu
   *  is the only path that works — macOS binds ⌘N and ⌘T to its own stock menu
   *  items and consumes them before the webview sees a key at all. */
  const COMMANDS: Record<string, () => void> = {
    new_workspace: () => void openWorkspace(),
    new_tab: () => {
      const ws = store.activeWs;
      // ⌘T needs a workspace to hold the tab. With none open, "give me a
      // terminal" first means "in which project?" — so route to the chooser
      // rather than silently conjuring a $HOME workspace no one asked for.
      if (ws) store.send({ t: 'open_tab', ws: ws.id });
      else void openWorkspace();
    },
    // ⌘D splits side by side, ⇧⌘D stacks — as iTerm2 does.
    split_v: () => {
      const pane = store.targetPane();
      if (pane !== null) store.send({ t: 'split', pane, dir: 'vertical' });
    },
    split_h: () => {
      const pane = store.targetPane();
      if (pane !== null) store.send({ t: 'split', pane, dir: 'horizontal' });
    },
    close_pane: () => {
      const pane = store.targetPane();
      // A close kills the process inside — worth one question. Restart is
      // cheap, an agent's context is not.
      if (pane !== null && confirm('Close this pane? Its process will be terminated.')) {
        store.send({ t: 'close_pane', pane });
      }
    },
    prev_tab: () => store.cycleTab(-1),
    next_tab: () => store.cycleTab(1),
  };

  function run(cmd: string) {
    // Opening a tab is the one thing a share may do, and only a writable
    // whole-machine one. The server enforces this too; this only keeps the
    // shortcut from looking broken.
    if (cmd === 'new_tab' ? !store.caps.may_open_tab : !store.caps.host) return;
    COMMANDS[cmd]?.();
  }

  // The desktop shell forwards its menu clicks here. A plain global callback,
  // deliberately: a bare module specifier would have to resolve at runtime, and
  // when it failed it failed silently — leaving every shortcut dead with
  // nothing in the console to say why.
  $effect(() => {
    const s = (window as any).__BEEBOX__;
    if (typeof s?.onMenu !== 'function') return;
    return s.onMenu(run);
  });

  function onkeydown(e: KeyboardEvent) {
    if (!e.metaKey) return;

    // Appearance is local to this browser, so a read-only viewer gets it too.
    // ⌘, is the platform convention; ⌘K stays as a working alias for the
    // e2e/muscle-memory path but is no longer advertised.
    if (e.key === ',' || e.key.toLowerCase() === 'k') {
      e.preventDefault();
      dialog = 'settings';
      return;
    }
    // Local too: folding moves nothing but this screen's furniture.
    if (e.key.toLowerCase() === 'b' && store.caps.show_sidebar) {
      e.preventDefault();
      setFolded(!folded);
      return;
    }
    if (!store.caps.host && !store.caps.may_open_tab) return;

    // In the desktop shell the menu already owns these, and handling them
    // twice would open two workspaces for one keypress.
    const cmd = desktop ? null : keyCommand(e);
    if (cmd) {
      e.preventDefault();
      run(cmd);
    }
  }

  /** No "Reload" or "Inspect Element" on the furniture.
   *
   *  The same reasoning as not letting the chrome be selected: a native app
   *  does not offer to reload itself. The terminal keeps its menu, which is
   *  where copy and paste live, and anything that has put up a menu of its own
   *  has already called preventDefault by the time this runs. */
  function oncontextmenu(e: MouseEvent) {
    const el = e.target as HTMLElement | null;
    if (el?.closest('.xterm, input, textarea')) return;
    e.preventDefault();
  }

  /** Maps a keystroke to a command name, or null if it is not a shortcut. */
  function keyCommand(e: KeyboardEvent): string | null {
    switch (e.key.toLowerCase()) {
      case 'n':
        return 'new_workspace';
      case 't':
        return 'new_tab';
      case 'd':
        return e.shiftKey ? 'split_h' : 'split_v';
      case 'w':
        return 'close_pane';
      // ⇧⌘[ / ⇧⌘] — the same pair Chrome and Safari use for tabs. macOS
      // reports the unshifted character here, so both forms are matched.
      case '[':
      case '{':
        return 'prev_tab';
      case ']':
      case '}':
        return 'next_tab';
      default:
        return null;
    }
  }
</script>

<svelte:window {onkeydown} {oncontextmenu} />

{#if store.closedReason === 'revoked'}
  <!-- The same page the server gives any unknown address, so a link that
       has been disconnected says nothing about what used to be behind it. -->
  <div class="gone">
    <div>
      <b>404</b>
      <p>This page isn’t available.</p>
    </div>
  </div>
{/if}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="titlebar" onpointerdown={startDrag} ondblclick={titleDoubleClick}>
  <!-- In the desktop shell macOS draws the real traffic lights on top, so we
       only reserve the space. In a browser tab we draw our own. -->
  {#if desktop}
    <div class="traffic-space"></div>
  {:else}
    <div class="traffic">
      <button class="tl close" aria-label="Close" onclick={() => windowAction('close')}
      ></button><button
        class="tl min"
        aria-label="Minimize"
        onclick={() => windowAction('minimize')}
      ></button><button class="tl zoom" aria-label="Zoom" onclick={() => windowAction('zoom')}
      ></button>
    </div>
  {/if}

  <!-- The sidebar has its own; this one is for a phone, where the folded
       sidebar is gone and its button with it. -->
  {#if store.caps.show_sidebar}
    <button
      class="tb-btn command phone-fold"
      aria-label={folded ? 'Show workspaces' : 'Hide workspaces'}
      title="Workspaces  ⌘B"
      onclick={() => setFolded(!folded)}
    >
      <Icon name="sidebar" size={15} />
    </button>
  {/if}

  <div class="slot-hint">+ plugins</div>

  <div class="spacer"></div>
  {#if store.caps.host}
    <button
      class="tb-btn with-icon"
      onclick={() => (dialog = 'share')}
    >
      <Icon name="share" size={13} />
      Share
    </button>
  {/if}
  <!-- Scrollback runs to tens of thousands of lines, which is more flicks
       than anyone will make on a phone. On a desktop Home and End already do
       this, so these appear only where neither exists. -->
  {#if store.sizing}
    <button
      class="tb-btn command"
      aria-label="Jump to the oldest line"
      title="Oldest"
      onclick={() => store.jumpVisible('top')}
    >
      <Icon name="top" size={15} />
    </button>
    <button
      class="tb-btn command"
      aria-label="Jump to the newest line"
      title="Newest"
      onclick={() => store.jumpVisible('bottom')}
    >
      <Icon name="bottom" size={15} />
    </button>
  {/if}
  <!-- Leaving a session, for the app: there is no address bar to navigate
       away with, so without this the only way out is killing the app from
       recents. The shell owns the connect screen, so it does the work. -->
  {#if appShell()?.disconnect}
    <button
      class="tb-btn command"
      aria-label="Disconnect"
      title="Disconnect"
      onclick={() => appShell().disconnect()}
    >
      <Icon name="unplug" size={15} />
    </button>
  {/if}
  <!-- The owner, a share that moves the owner's view, and a phone that
       asked to drive its own size.
       The terminal has one size, and whoever spoke last holds it — so after a
       phone narrows a session to 46 columns the desktop stays there, with
       nothing to make it measure again. Its ResizeObserver only fires when
       the window changes, and the window did not. This asks. -->
  {#if store.sizing || store.caps.may_open_tab}
    <button
      class="tb-btn command"
      aria-label="Re-fit to this screen"
      title="Re-fit to this screen"
      onclick={() => store.refit()}
    >
      <Icon name="refresh" size={15} />
    </button>
  {/if}
  <button
    class="tb-btn command"
    aria-label="Settings"
    title="Settings  ⌘,"
    onclick={() => (dialog = 'settings')}
  >
    <Icon name="settings" size={15} />
  </button>
</div>

<div class="body">
  {#if store.caps.show_sidebar}
    <Sidebar onnew={openWorkspace} ontoggle={() => setFolded(!folded)} {folded} />
    {#if !folded}
      <!-- Only drawn where the open sidebar lies over the terminal; tapping
           beside it puts it away. -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="scrim"
        transition:fade={{ duration: FOLD_MS }}
        onclick={() => setFolded(true)}
      ></div>
    {/if}
  {/if}

  <main class="main">
    {#if store.caps.show_tabs}
      <TabBar />
    {/if}

    <!-- Every tab in every workspace stays mounted and is hidden with CSS.
         Unmounting would dispose its terminals, so switching away and back
         would lose the scrollback — which is exactly what you keep a long
         agent run around for. -->
    <div class="stage">
      {#each store.tree.workspaces as w (w.id)}
        {#each w.tabs as t (t.id)}
          <div
            class="sheet"
            class:on={w.id === store.activeWs?.id && t.id === tab?.id}
          >
            <SplitTree node={t.layout} tab={t.id} />
          </div>
        {/each}
      {:else}
        <div class="empty">
          {#if store.caps.host}
            <div class="empty-icon"><Icon name="folder" size={30} stroke={1.5} /></div>
            <div class="empty-title">No workspace open</div>
            <p class="empty-sub">
              A workspace is a project folder. Open one to start a terminal.
            </p>
            <button class="empty-cta" onclick={openWorkspace}>
              <Icon name="plus" size={14} />
              Open a workspace
            </button>
            <div class="empty-hint">or press ⌘N</div>
          {:else}
            <div class="empty-icon"><Icon name="folder" size={30} stroke={1.5} /></div>
            <div class="empty-title">Nothing shared yet</div>
            <p class="empty-sub">The owner hasn't opened a workspace to share.</p>
          {/if}
        </div>
      {/each}
    </div>
  </main>
</div>

<div class="statusbar">
  <span class="item" class:off={!store.connected}>
    {#if store.connected}
      daemon connected · :{location.port || 17788}
    {:else}
      reconnecting…
    {/if}
  </span>
  {#if store.caps.show_sidebar}
    <div class="sep-v scope-sep"></div>
    <span class="item stats">
      {store.tree.workspaces.length} workspaces · {tabCount} tabs · {paneCount} panes
    </span>
  {/if}

  <div class="spacer"></div>
  {#if store.caps.host}
    <!-- The daemon always serves loopback (the terminal itself rides on it);
         this toggles whether anyone else on the network is let in. -->
    <button
      class="item"
      class:live={store.webExposed}
      title={store.webExposed
        ? 'Web server is open to the network — click to close'
        : 'Click to allow network clients (needed before sharing)'}
      onclick={() => store.send({ t: 'set_web_server', exposed: !store.webExposed })}
    >
      {store.webExposed ? '● web server on' : '○ web server off'}
    </button>
    <div class="sep-v"></div>
    {#if store.webExposed && store.peers.length > 0}
      <button class="item live" onclick={() => (dialog = 'conns')}>
        ● {store.peers.length} shared
      </button>
      <div class="sep-v"></div>
    {/if}
  {/if}
  <span class="item">UTF-8</span>
</div>

<!-- A phone's keyboard has no Esc, Ctrl or arrows. Last on the page, so it
     sits directly on top of the soft keyboard. -->
{#if store.sizing}
  <KeyBar />
{/if}

{#if dialog === 'share'}
  <ShareDialog onclose={() => (dialog = null)} />
{:else if dialog === 'conns'}
  <ConnectionsDialog onclose={() => (dialog = null)} />
{:else if dialog === 'settings'}
  <SettingsDialog onclose={() => (dialog = null)} />
{:else if dialog === 'openws'}
  <OpenWorkspaceDialog onclose={() => (dialog = null)} />
{/if}

<style>
  /* Matches the daemon's own 404 page. */
  .gone {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    text-align: center;
    background: #0f0f13;
    color: #8b8b95;
    font-family: system-ui, -apple-system, sans-serif;
  }
  .gone b {
    display: block;
    font: 600 56px ui-monospace, Menlo, monospace;
    color: #3a3a44;
    letter-spacing: 4px;
  }
  .gone p {
    margin-top: 10px;
    font-size: 14px;
  }

  .titlebar {
    height: 38px;
    flex: 0 0 38px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px;
  }
  /* macOS puts its buttons at x∈[7,61] and will not be talked out of it — a
     titlebar accessory does not re-centre them, and moving them by hand gets
     undone on every resize. Measured, then left alone. */
  .traffic-space {
    width: 58px;
    flex: 0 0 58px;
  }
  .traffic {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-right: 4px;
  }
  /* Real buttons: in a browser tab there are no native ones to defer to. */
  .tl {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    display: block;
    padding: 0;
    border: none;
  }
  .tl.close { background: #ff5f57; }
  .tl.min   { background: #febc2e; }
  .tl.zoom  { background: #28c840; }
  .tl:hover { filter: brightness(1.15); }
  .tl:active { filter: brightness(0.85); }

  .slot-hint {
    font-size: 10px;
    color: var(--faint);
    border: 1px dashed var(--border);
    border-radius: 6px;
    padding: 3px 8px;
  }

  .spacer {
    flex: 1;
    min-width: 8px;
  }
  .tb-btn {
    padding: 4px 9px;
    border-radius: 6px;
    color: var(--dim);
    font-size: 12px;
  }
  .tb-btn:hover {
    background: var(--panel-2);
    color: var(--fg);
  }
  .tb-btn.with-icon {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .tb-btn.command {
    min-width: 33px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  /* The same width as the sidebar's container query, which decides when
     folded means gone. */
  @media (min-width: 561px) {
    .tb-btn.phone-fold {
      display: none;
    }
  }

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
    position: relative;
    /* Measured by the sidebar: what folding means depends on the room this
       box has, not on what kind of device it is. */
    container: body / inline-size;
  }
  .scrim {
    display: none;
  }
  @container body (max-width: 560px) {
    .scrim {
      display: block;
      position: absolute;
      inset: 0;
      z-index: 19;
      background: rgba(0, 0, 0, 0.45);
    }
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .stage {
    flex: 1;
    min-height: 0;
    padding: 6px;
    position: relative;
  }
  /* Hidden rather than unmounted. `visibility` keeps the box measurable, so a
     terminal in a background tab still knows its size and does not need a
     reflow when it comes back. */
  .sheet {
    position: absolute;
    inset: 6px;
    display: flex;
    gap: 6px;
    visibility: hidden;
  }
  .sheet.on {
    visibility: visible;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    color: var(--faint);
    text-align: center;
    padding: 24px;
  }
  .empty-icon {
    color: var(--border);
    margin-bottom: 8px;
  }
  .empty-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--dim);
  }
  .empty-sub {
    font-size: 12px;
    color: var(--faint);
    max-width: 280px;
    margin-bottom: 14px;
    line-height: 1.5;
  }
  .empty-cta {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--bg);
    font-size: 12.5px;
    font-weight: 600;
  }
  .empty-cta:hover {
    filter: brightness(1.08);
  }
  .empty-hint {
    margin-top: 9px;
    font-size: 11px;
    color: var(--faint);
  }

  .statusbar {
    height: 26px;
    flex: 0 0 26px;
    background: var(--panel);
    border-top: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    font-size: 11px;
    color: var(--faint);
  }
  .sep-v {
    width: 1px;
    height: 12px;
    background: var(--border);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .item.off {
    color: var(--wait);
  }
  .live {
    color: #86efac;
    padding: 2px 7px;
    border-radius: 7px;
  }
  button.live:hover {
    background: #14251a;
  }

  @media (max-width: 640px) {
    .slot-hint { display: none; }
    .titlebar { gap: 6px; padding-inline: 8px; }
    .tb-btn { padding-inline: 7px; }
    .statusbar .stats, .statusbar .scope-sep { display: none; }
    .statusbar { padding-inline: 8px; gap: 5px; }
  }
</style>
