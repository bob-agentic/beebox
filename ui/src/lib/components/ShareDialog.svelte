<script lang="ts">
  import qrcode from 'qrcode-generator';
  import { store } from '../state.svelte';
  import { writeClipboard } from '../clipboard';
  import Icon from './Icon.svelte';
  import type { GrantScope } from '../proto';

  let { onclose }: { onclose: () => void } = $props();

  type ScopeKind = 'all' | 'workspace' | 'tab' | 'pane';
  let kind = $state<ScopeKind>('tab');
  let writable = $state(false);
  /** Which URL's code is being shown, if any. */
  let showing = $state<string | null>(null);
  /** What the link on show was made for, frozen at Create so the result
      cannot drift from the choices above it. */
  let made = $state<{ kind: ScopeKind; what: string; writable: boolean } | null>(null);

  // A link from an earlier opening must never greet the next one: whoever
  // copies it would be sending out a scope they did not just choose.
  store.share = null;

  const ws = $derived(store.activeWs);
  const tab = $derived(store.activeTab);
  const pane = $derived(store.focused !== null ? store.pane(store.focused) : undefined);

  const SCOPE_NAME: Record<ScopeKind, string> = {
    all: 'All workspaces',
    workspace: 'This workspace',
    tab: 'This tab',
    pane: 'One pane',
  };

  const WHAT_THEY_SEE: Record<ScopeKind, string> = {
    all: 'They see: full sidebar and every tab',
    workspace: 'They see: this workspace’s tabs only, no sidebar',
    tab: 'They see: this tab’s panes only, no sidebar or tab bar',
    pane: 'They see: one full-screen terminal, no navigation',
  };

  function scopeValue(): GrantScope | null {
    switch (kind) {
      case 'all':
        return { kind: 'all' };
      case 'workspace':
        return ws ? { kind: 'workspace', ws: ws.id } : null;
      case 'tab':
        return tab ? { kind: 'tab', tab: tab.id } : null;
      case 'pane':
        return pane ? { kind: 'pane', pane: pane.id } : null;
    }
  }

  /** Sent and not yet answered. Create stays shut meanwhile, so a second
      request cannot land its link under the first one's label. */
  const creating = $derived(made !== null && store.share === null);

  function create() {
    const scope = scopeValue();
    if (!scope || creating) return;
    const what = { all: '', workspace: ws?.name, tab: tab?.title, pane: pane?.title || pane?.agent }[kind];
    made = { kind, what: what ?? '', writable };
    store.send({ t: 'create_grant', scope, writable });
  }

  /** Back to the choices. The link stays valid; it just leaves the screen. */
  function again() {
    store.share = null;
    made = null;
  }

  // Every reachable base URL, dev-server style: what the daemon enumerated
  // from its interfaces, with this page's own origin first when it is not
  // loopback (that one is known to work — the page came over it).
  const urls = $derived.by(() => {
    if (!store.share) return [];
    const path = store.share.url;
    const list = store.share.hosts.map((h) => `${h}${path}`);
    if (!/^https?:\/\/(localhost|127\.)/.test(location.origin)) {
      const own = `${location.origin}${path}`;
      if (!list.includes(own)) list.unshift(own);
    }
    const plain = list.length ? list : [`${location.origin}${path}`];
    // Two forms of the same link. The code carries the `beebox://` one, which
    // the phone's own camera hands to the app; the app knows it is a phone and
    // takes the terminal's size accordingly, so nothing needs to say so here.
    return plain.map((http) => ({
      http,
      app: http.replace(/^https?:\/\//, 'beebox://'),
    }));
  });

  /** The code for whichever link is being shown, as an SVG string.
   *
   *  Type 0 lets the library pick the smallest version that fits; level M
   *  tolerates a quarter of the code being obscured, which is the usual
   *  choice for something read off a screen. */
  const codeSvg = $derived.by(() => {
    if (!showing) return '';
    const qr = qrcode(0, 'M');
    qr.addData(showing);
    qr.make();
    return qr.createSvgTag({ cellSize: 5, margin: 2, scalable: true });
  });

  // The Copy button must answer, or the user assumes it did nothing.
  let copied = $state<string | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;
  async function copy(text: string) {
    // A share link is most often copied on the very device that cannot reach
    // navigator.clipboard — a viewer on plain http. Hence the helper.
    if (!(await writeClipboard(text))) return;
    copied = text;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = null), 1600);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal">
    <h3>Share</h3>
    <div class="sub">Anyone with the link sees a live terminal — nothing to install</div>

    {#if store.share && made}
      <div class="made">
        <b>
          {SCOPE_NAME[made.kind]}{#if made.what}<span class="what"> · {made.what}</span>{/if}
        </b>
        <span class="tag {made.writable ? 'warn' : 'ok'}">
          {made.writable ? 'can type' : 'read-only'}
        </span>
      </div>
      <div class="sees">{WHAT_THEY_SEE[made.kind]}</div>

      {#if store.share.pair_code}
        {@const code = store.share.pair_code}
        <div class="paircode">
          <div class="code">{code}</div>
          <div class="pc-note">
            Single use — spent once they pair<br />
            <span class="dim">Send it by another channel, not with the link</span>
          </div>
          <button class:did={copied === code} onclick={() => copy(code)}>
            {copied === code ? 'Copied ✓' : 'Copy'}
          </button>
        </div>
      {/if}

      <!-- One URL per reachable address, like a dev server's banner. -->
      <div class="urls">
        {#each urls as u (u.http)}
          <div class="field">
            <span>{u.http}</span>
            <button
              class="qr-btn"
              title="Show a code for the app to scan"
              aria-label="Show QR code"
              onclick={() => (showing = u.app)}
            >
              <Icon name="qr" size={13} />
            </button>
            <button class:did={copied === u.http} onclick={() => copy(u.http)}>
              {copied === u.http ? 'Copied ✓' : 'Copy'}
            </button>
          </div>
        {/each}
      </div>

      <div class="acts">
        <button class="g" onclick={again}>Share something else</button>
        <button class="p" onclick={onclose}>Done</button>
      </div>
    {:else}
      <div class="scope-label">Scope</div>
      <div class="scope">
        <button class:on={kind === 'all'} onclick={() => (kind = 'all')}>
          <b>All workspaces <span class="tag warn">whole machine</span></b>
          <small>
            {store.tree.workspaces.length} workspaces ·
            {store.tree.workspaces.reduce((n, w) => n + w.tabs.length, 0)} tabs ·
            they get the same view you have
          </small>
        </button>

        <button class:on={kind === 'workspace'} onclick={() => (kind = 'workspace')}>
          <b>This workspace</b>
          <small>{ws?.name ?? '—'} · all {ws?.tabs.length ?? 0} tabs, switchable</small>
        </button>

        <button class:on={kind === 'tab'} onclick={() => (kind = 'tab')}>
          <b>This tab</b>
          <small>{tab?.title || 'current tab'} · {tab?.panes.length ?? 0} panes</small>
        </button>

        <button class:on={kind === 'pane'} onclick={() => (kind = 'pane')}>
          <b>One pane <span class="tag ok">smallest surface</span></b>
          <small>{pane?.title || pane?.agent || '—'} · this terminal only, nothing else</small>
        </button>
      </div>

      <div class="sees">{WHAT_THEY_SEE[kind]}</div>

      <div class="perm">
        <button class:on={!writable} onclick={() => (writable = false)}>
          Read-only<small>They watch, they cannot type</small>
        </button>
        <button class:on={writable} onclick={() => (writable = true)}>
          Can type<small>They can take the keyboard</small>
        </button>
      </div>

      <div class="acts">
        <button class="g" onclick={onclose}>Close</button>
        <button class="p" disabled={creating} onclick={create}>
          {creating ? 'Creating…' : 'Create link'}
        </button>
      </div>
    {/if}
  </div>
</div>

{#if showing}
  <!-- Over the dialog rather than inside the URL list, which scrolls and would
       crop it. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="qr-mask" onclick={() => (showing = null)}>
    <div class="qr-card">
      <div class="qr-svg">{@html codeSvg}</div>
      <div class="qr-url">{showing}</div>
      <div class="qr-note">
        Scan with the phone's own camera — the app opens it directly.
      </div>
    </div>
  </div>
{/if}

<style>
  /* The code sits above the dialog: the URL list scrolls, and anything drawn
     inside it would be cropped. */
  .qr-mask {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.72);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 60;
  }
  .qr-card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 13px;
    padding: 18px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 11px;
    max-width: 300px;
  }
  /* White behind the code regardless of theme: a scanner needs the contrast,
     and half the themes here are light-on-dark. */
  .qr-svg {
    width: 196px;
    height: 196px;
    background: #fff;
    border-radius: 9px;
    padding: 9px;
  }
  .qr-svg :global(svg) {
    width: 100%;
    height: 100%;
    display: block;
  }
  .qr-url {
    font: 10.5px/1.4 ui-monospace, monospace;
    color: var(--dim);
    word-break: break-all;
    text-align: center;
  }
  .qr-note {
    font-size: 11px;
    color: var(--faint);
    text-align: center;
    line-height: 1.5;
  }
  .qr-btn {
    color: var(--faint);
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
  }
  .qr-btn:hover {
    color: var(--fg);
  }

  .mask {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }
  .modal {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 13px;
    width: 420px;
    padding: 18px;
    max-height: 90vh;
    overflow-y: auto;
  }
  h3 {
    font-size: 14px;
    margin-bottom: 4px;
  }
  .sub {
    font-size: 11.5px;
    color: var(--faint);
    margin-bottom: 14px;
  }
  .scope-label {
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--faint);
    margin-bottom: 6px;
  }
  .scope {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-bottom: 14px;
  }
  .scope button {
    padding: 8px 11px;
    border-radius: 7px;
    border: 1px solid var(--border);
    background: var(--bg);
    text-align: left;
  }
  .scope button:hover {
    border-color: var(--faint);
  }
  .scope button.on {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .scope button b {
    font-size: 12px;
    font-weight: 600;
    color: var(--dim);
    display: block;
  }
  .scope button.on b {
    color: var(--fg);
  }
  .scope button small {
    display: block;
    font-size: 10px;
    color: var(--faint);
    margin-top: 1px;
  }
  .tag {
    font-size: 8.5px;
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 3px;
    margin-left: 5px;
    vertical-align: 1px;
  }
  .tag.warn {
    background: #3d2416;
    color: #fbbf24;
  }
  .tag.ok {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: #86efac;
  }

  .perm {
    display: flex;
    gap: 7px;
    margin-bottom: 14px;
  }
  .perm button {
    flex: 1;
    padding: 9px;
    border-radius: 7px;
    border: 1px solid var(--border);
    background: var(--bg);
    font-size: 11.5px;
    color: var(--dim);
    text-align: left;
  }
  .perm button.on {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--fg);
  }
  .perm button small {
    display: block;
    font-size: 9.5px;
    color: var(--faint);
    margin-top: 2px;
  }

  .urls {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 12px;
    max-height: 140px;
    overflow-y: auto;
  }
  .urls .field {
    margin-bottom: 0;
  }
  .field button.did {
    color: var(--accent);
  }
  .field {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 9px 11px;
    font: 11.5px ui-monospace, monospace;
    color: #7dd3fc;
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }
  .field span {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .field button {
    color: var(--dim);
    font-size: 11px;
    padding: 2px 7px;
    background: var(--panel-2);
    border-radius: 4px;
  }
  /* The result names what it was made for, so a link is never copied
     without its scope in view. */
  .made {
    display: flex;
    align-items: center;
    padding: 10px 12px;
    margin-bottom: 12px;
    border: 1px solid var(--accent);
    border-radius: 7px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    font-size: 12.5px;
  }
  .made b {
    font-weight: 600;
    color: var(--fg);
  }
  .made .what {
    font-weight: 400;
    color: var(--dim);
  }
  .sees {
    font-size: 10px;
    color: var(--faint);
    margin: -6px 0 12px 2px;
  }

  .paircode {
    display: flex;
    align-items: center;
    gap: 11px;
    margin-bottom: 13px;
    padding: 11px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
  }
  .code {
    font: 700 21px ui-monospace, monospace;
    letter-spacing: 0.16em;
    color: var(--accent);
    flex: 0 0 auto;
  }
  .pc-note {
    flex: 1;
    font-size: 9.5px;
    color: var(--dim);
    line-height: 1.5;
  }
  .pc-note .dim {
    color: var(--faint);
  }
  .paircode button {
    color: var(--dim);
    font-size: 11px;
    padding: 2px 7px;
    background: var(--panel-2);
    border-radius: 4px;
  }
  .paircode button.did {
    color: var(--accent);
  }

  .acts {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .acts button {
    padding: 7px 15px;
    border-radius: 7px;
    font-size: 12px;
  }
  .acts .g {
    background: var(--panel-2);
    color: var(--dim);
  }
  .acts .p {
    background: var(--accent);
    /* The theme background always contrasts with its own accent. */
    color: var(--bg);
    font-weight: 600;
  }
  .acts .p:disabled {
    opacity: 0.6;
  }

  @media (max-width: 480px) {
    .modal { width: calc(100vw - 24px); }
  }
</style>
