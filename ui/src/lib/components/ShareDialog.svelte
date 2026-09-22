<script lang="ts">
  import { store } from '../state.svelte';
  import { writeClipboard } from '../clipboard';
  import type { GrantScope } from '../proto';

  let { onclose }: { onclose: () => void } = $props();

  type ScopeKind = 'all' | 'workspace' | 'tab' | 'pane';
  let kind = $state<ScopeKind>('tab');
  let writable = $state(false);
  let pairing = $state(false);
  // A phone cannot read a terminal laid out for a desktop: 175 columns of text
  // arriving on a 46-column screen overlaps itself. Letting the link resize the
  // terminal is the only way it becomes readable — at the cost of resizing this
  // window too, since a PTY has one size.
  let sizing = $state(false);

  const ws = $derived(store.activeWs);
  const tab = $derived(store.activeTab);
  const pane = $derived(store.focused !== null ? store.pane(store.focused) : undefined);

  /** Workspace level and up reveals which projects and tabs exist, so those
      scopes always pair. Tab and pane are the user's call. */
  const forced = $derived(kind === 'all' || kind === 'workspace');

  $effect(() => {
    if (forced) pairing = true;
  });

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

  function create() {
    const scope = scopeValue();
    if (scope) store.send({ t: 'create_grant', scope, writable, pairing });
  }

  // Every reachable base URL, dev-server style: what the daemon enumerated
  // from its interfaces, with this page's own origin first when it is not
  // loopback (that one is known to work — the page came over it).
  const urls = $derived.by(() => {
    if (!store.share) return [] as string[];
    const path = store.share.url + (sizing ? '?phone=1' : '');
    const list = store.share.hosts.map((h) => `${h}${path}`);
    if (!/^https?:\/\/(localhost|127\.)/.test(location.origin)) {
      const own = `${location.origin}${path}`;
      if (!list.includes(own)) list.unshift(own);
    }
    return list.length ? list : [`${location.origin}${path}`];
  });

  // The Copy button must answer, or the user assumes it did nothing.
  let copied = $state<string | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;
  async function copy(url: string) {
    // A share link is most often copied on the very device that cannot reach
    // navigator.clipboard — a viewer on plain http. Hence the helper.
    if (!(await writeClipboard(url))) return;
    copied = url;
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

    <label class="pair-toggle" class:forced>
      <input type="checkbox" bind:checked={pairing} disabled={forced} />
      <span class="pt-text">
        <b>
          Require a pairing code
          {#if forced}<span class="lock">locked on</span>{/if}
        </b>
        <small>
          {forced
            ? 'Always required at workspace scope and above'
            : 'A forwarded link alone will not get in · optional at this scope'}
        </small>
      </span>
    </label>

    <label class="pair-toggle">
      <input type="checkbox" bind:checked={sizing} />
      <span class="pt-text">
        <b>Let the link resize the terminal</b>
        <small>
          For a phone · a desktop-width terminal overlaps itself on a small
          screen. <span class="dim">Your own window resizes with it.</span>
        </small>
      </span>
    </label>

    {#if store.share?.pair_code}
      <div class="paircode">
        <div class="code">{store.share.pair_code}</div>
        <div class="pc-note">
          Single use — spent once they pair<br />
          <span class="dim">Send it by another channel, not with the link</span>
        </div>
      </div>
    {/if}

    {#if store.share}
      <!-- The result lives at the bottom, where it lands after Create link:
           one URL per reachable address, like a dev server's banner. -->
      <div class="urls">
        {#each urls as url (url)}
          <div class="field">
            <span>{url}</span>
            <button class:did={copied === url} onclick={() => copy(url)}>
              {copied === url ? 'Copied ✓' : 'Copy'}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    <div class="acts">
      <button class="g" onclick={onclose}>Close</button>
      <button class="p" onclick={create}>
        {store.share ? 'New link' : 'Create link'}
      </button>
    </div>
  </div>
</div>

<style>
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

  .pair-toggle {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    cursor: pointer;
    padding: 9px 11px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg);
    margin-bottom: 8px;
  }
  .pair-toggle:has(input:checked) {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .pair-toggle.forced {
    cursor: default;
  }
  .pair-toggle input {
    margin-top: 2px;
    accent-color: var(--accent);
  }
  .pt-text b {
    font-size: 11.5px;
    font-weight: 600;
    display: block;
    color: var(--dim);
  }
  .pair-toggle:has(input:checked) .pt-text b {
    color: var(--fg);
  }
  .pt-text small {
    font-size: 10px;
    color: var(--faint);
    display: block;
    margin-top: 1px;
  }
  .lock {
    font-size: 8.5px;
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 3px;
    background: #3d2416;
    color: #fbbf24;
    vertical-align: 1px;
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

  @media (max-width: 480px) {
    .modal { width: calc(100vw - 24px); }
  }
</style>
