<script lang="ts">
  import { store } from '../state.svelte';
  import { sortable } from '../dragsort.svelte';
  import { agentBadge, rollupWithUnread } from '../agent-status';
  import type { TabView } from '../proto';
  import StatusIcon from './StatusIcon.svelte';
  import Icon from './Icon.svelte';

  function tabDot(tab: TabView): { phase: import('../proto').AgentPhase; unread: boolean } {
    // readRev makes this re-derive when a pane is marked read in this browser.
    void store.readRev;
    return rollupWithUnread(tab.panes);
  }

  /** A shell's OSC title is "user@host:/path", which tells you nothing you
      did not already know. Priority: the user's own name for the tab, then
      the focused agent session's title, then the running command, then the
      folder — never the hostname. */
  function label(tab: TabView): string {
    if (tab.title) return tab.title;
    const session = tab.panes.find((p) => p.session_title)?.session_title;
    if (session) return session;
    const p = tab.panes[0];
    if (!p) return 'shell';
    const t = p.title ?? '';
    if (t && !/^[\w.-]+@[\w.-]+[:\s]/.test(t)) return t;
    return p.cwd.split('/').filter(Boolean).pop() || 'shell';
  }

  function agent(tab: TabView): string {
    return agentBadge(tab.panes[0]?.agent ?? null);
  }

  const AGENT_STYLE: Record<string, string> = {
    OC: 'background:#1f2f3d;color:#7dd3fc',
    SH: 'background:#2b2b35;color:#8b8b9a',
  };

  // Brand marks for the two agents that have one; see Pane.svelte. Codex's
  // blossom is monochrome and follows the theme, Claude's is always orange.
  const AGENT_MARK: Record<string, { icon: 'claude' | 'codex'; color: string }> = {
    CC: { icon: 'claude', color: '#D97757' },
    CX: { icon: 'codex', color: 'var(--fg)' },
  };

  const ws = $derived(store.activeWs);

  // Double-click to rename, middle-click to close — the same gestures mux0
  // and every browser use.
  let editing = $state<number | null>(null);
  let draft = $state('');

  function beginRename(tab: TabView) {
    editing = tab.id;
    draft = tab.title || labels.get(tab.id) || '';
  }

  function commitRename(tab: TabView) {
    if (editing !== tab.id) return;
    editing = null;
    store.send({ t: 'rename_tab', tab: tab.id, title: draft });
  }

  function focusInput(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  /** Three tabs all called "bee-box" are three tabs you cannot tell apart, so
      duplicates get a number — the same thing a browser does. */
  const labels = $derived.by(() => {
    const tabs = ws?.tabs ?? [];
    const seen = new Map<string, number>();
    const total = new Map<string, number>();
    for (const t of tabs) {
      const n = label(t);
      total.set(n, (total.get(n) ?? 0) + 1);
    }
    return new Map(
      tabs.map((t) => {
        const n = label(t);
        if ((total.get(n) ?? 0) < 2) return [t.id, n];
        const i = (seen.get(n) ?? 0) + 1;
        seen.set(n, i);
        return [t.id, `${n} ${i}`];
      }),
    );
  });
</script>

<div class="tabbar">
  {#each ws?.tabs ?? [] as tab (tab.id)}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="tab"
      use:sortable={{
        id: tab.id,
        axis: 'x',
        ignore: '.x',
        order: () => (ws?.tabs ?? []).map((t) => t.id),
        commit: (order) => ws && store.send({ t: 'reorder_tabs', ws: ws.id, order }),
      }}
      class:active={tab.id === store.activeTab?.id}
      onclick={() => ws && store.activate(ws.id, tab.id)}
      onauxclick={(e) => {
        if (e.button === 1 && store.caps.owner) {
          e.preventDefault();
          if (confirm('Close this tab? Its terminals will be terminated.'))
            store.send({ t: 'close_tab', tab: tab.id });
        }
      }}
    >
      <StatusIcon phase={tabDot(tab).phase} unread={tabDot(tab).unread} />
      {#if AGENT_MARK[agent(tab)]}
        <span class="mark" style="color:{AGENT_MARK[agent(tab)].color}" title={agent(tab)}>
          <Icon name={AGENT_MARK[agent(tab)].icon} size={12} />
        </span>
      {:else}
        <span class="agent" style={AGENT_STYLE[agent(tab)] ?? AGENT_STYLE.SH}>{agent(tab)}</span>
      {/if}
      {#if editing === tab.id}
        <input
          class="rename"
          bind:value={draft}
          use:focusInput
          onclick={(e) => e.stopPropagation()}
          onblur={() => commitRename(tab)}
          onkeydown={(e) => {
            if (e.key === 'Enter') commitRename(tab);
            if (e.key === 'Escape') editing = null;
          }}
        />
      {:else}
        <span
          class="label"
          ondblclick={(e) => {
            if (!store.caps.owner) return;
            e.stopPropagation();
            beginRename(tab);
          }}
          oncontextmenu={(e) => {
            // Right-click on a manually named tab offers the way back to the
            // automatic title; an empty rename is exactly that on the wire.
            if (!store.caps.owner || !tab.title) return;
            e.preventDefault();
            store.send({ t: 'rename_tab', tab: tab.id, title: '' });
          }}
          title={tab.title ? 'Right-click: reset to auto title' : ''}
          >{labels.get(tab.id) ?? label(tab)}</span
        >
      {/if}
      {#if store.caps.owner}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <span
          class="x"
          role="button"
          tabindex="0"
          aria-label="Close tab"
          onclick={(e) => {
            e.stopPropagation();
            if (confirm('Close this tab? Its terminals will be terminated.'))
            store.send({ t: 'close_tab', tab: tab.id });
          }}><Icon name="x" size={13} /></span
        >
      {/if}
    </div>
  {/each}

  {#if store.caps.owner && ws}
    <button
      class="add"
      title="New tab"
      aria-label="New tab"
      onclick={() => store.send({ t: 'open_tab', ws: ws.id })}
    >
      <Icon name="plus" size={15} />
    </button>
  {/if}
</div>

<style>
  .tabbar {
    height: 34px;
    flex: 0 0 34px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: stretch;
    padding: 0 6px;
    gap: 3px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabbar::-webkit-scrollbar {
    display: none;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 11px;
    border-radius: 7px 7px 0 0;
    margin-top: 4px;
    font-size: 12px;
    color: var(--dim);
    white-space: nowrap;
    max-width: 190px;
    cursor: pointer;
    flex: 0 0 auto;
  }
  .tab:hover {
    background: var(--panel-2);
  }
  .tab.active {
    background: var(--bg);
    color: var(--fg);
    /* Same treatment as the selected workspace in the sidebar: an accent bar,
       here across the top — background alone was too subtle to spot. */
    box-shadow: inset 0 2px 0 var(--accent);
  }
  /* Lifted rather than faded: the row is being carried, and a shadow says so
     while keeping it readable. Opacity alone made it look disabled. */
  .tab:global(.dragging) {
    cursor: grabbing;
    background: var(--panel-2);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
    /* The transform is the drag itself; nothing else may animate it. */
    transition: none;
  }
  .agent {
    font-size: 9.5px;
    padding: 1px 5px;
    border-radius: 3px;
    font-weight: 700;
  }
  /* The mark identifies on its own; a pill behind it would just add noise. */
  .mark {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rename {
    min-width: 60px;
    max-width: 140px;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 1px 5px;
    font: inherit;
  }
  .rename:focus {
    outline: none;
  }
  .x {
    color: var(--faint);
    display: inline-flex;
    align-items: center;
    cursor: pointer;
  }
  .x:hover {
    color: var(--fg);
  }
  .add {
    padding: 0 9px;
    color: var(--faint);
    flex: 0 0 auto;
    align-self: center;
    display: inline-flex;
    align-items: center;
  }
  .add:hover {
    color: var(--fg);
  }
</style>
