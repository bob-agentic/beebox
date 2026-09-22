<script lang="ts">
  import { store } from './lib/state.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import TabBar from './lib/components/TabBar.svelte';
  import SplitTree from './lib/components/SplitTree.svelte';
  import ShareDialog from './lib/components/ShareDialog.svelte';
  import ConnectionsDialog from './lib/components/ConnectionsDialog.svelte';
  import SettingsDialog from './lib/components/SettingsDialog.svelte';
  import OpenWorkspaceDialog from './lib/components/OpenWorkspaceDialog.svelte';
  import Icon from './lib/components/Icon.svelte';

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
    if (!store.caps.owner) return;
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
    if (!store.caps.owner) return;

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

{#if store.needsPairing}
  <!-- A paired share link: nothing renders until the code is given. The code
       travelled a second channel; this is where it lands. -->
  <div class="pair-gate">
    <div class="pair-card">
      <h3>Pairing code required</h3>
      <p>Ask the person who shared this link for the 6-character code.</p>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          const input = e.currentTarget.querySelector('input');
          if (input) store.submitPairCode(input.value);
        }}
      >
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="pair-input"
          class:err={store.pairError}
          maxlength="6"
          autofocus
          spellcheck="false"
          autocomplete="off"
          placeholder="······"
        />
        <button class="pair-go" type="submit">Connect</button>
      </form>
      {#if store.pairError}
        <div class="pair-err">That code didn't match — check it and try again.</div>
      {/if}
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

  <div class="slot-hint">+ plugins</div>

  <div class="spacer"></div>
  {#if store.caps.owner}
    <button
      class="tb-btn with-icon"
      onclick={() => (dialog = 'share')}
    >
      <Icon name="share" size={13} />
      Share
    </button>
  {/if}
  <!-- Only where this client drives the terminal's size — which today means
       the Android app, the one thing that sends `?phone=1`. Carrying a
       session from a phone to a tablet leaves the terminal at the first
       screen's width, and nothing re-measures on its own; this asks. On a
       desktop the size is the owner's and the button would do nothing. -->
  {#if store.sizing}
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
    <Sidebar onnew={openWorkspace} />
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
          {#if store.caps.owner}
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
    {:else if store.closedReason === 'kicked'}
      disconnected by the owner
    {:else if store.closedReason === 'revoked'}
      this share link was revoked
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
  {#if store.caps.owner}
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
  .pair-gate {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg);
  }
  .pair-card {
    width: 320px;
    padding: 24px;
    border: 1px solid var(--border);
    border-radius: 13px;
    background: var(--panel);
    text-align: center;
  }
  .pair-card h3 {
    font-size: 15px;
    margin-bottom: 6px;
  }
  .pair-card p {
    font-size: 11.5px;
    color: var(--faint);
    margin-bottom: 16px;
  }
  .pair-card form {
    display: flex;
    gap: 8px;
  }
  .pair-input {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 9px 12px;
    font-family: ui-monospace, monospace;
    font-size: 17px;
    letter-spacing: 6px;
    text-align: center;
    text-transform: uppercase;
  }
  .pair-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .pair-input.err {
    border-color: var(--err);
  }
  .pair-go {
    padding: 9px 16px;
    border-radius: 8px;
    background: var(--accent);
    /* The theme background always contrasts with its own accent. */
    color: var(--bg);
    font-weight: 600;
    font-size: 12px;
  }
  .pair-err {
    margin-top: 10px;
    font-size: 11px;
    color: var(--err);
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

  .plugin-slot {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 9px;
    font-size: 11px;
    max-width: 200px;
    overflow: hidden;
    white-space: nowrap;
  }
  .plugin-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--run);
    flex: 0 0 auto;
  }
  .plugin-label { color: var(--dim); }
  .plugin-value {
    color: var(--fg);
    font-family: ui-monospace, monospace;
  }
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

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
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
    .slot-hint, .plugin-slot { display: none; }
    .titlebar { gap: 6px; padding-inline: 8px; }
    .tb-btn { padding-inline: 7px; }
    .body :global(.sidebar) { display: none; }
    .statusbar .stats, .statusbar .scope-sep { display: none; }
    .statusbar { padding-inline: 8px; gap: 5px; }
  }
</style>
