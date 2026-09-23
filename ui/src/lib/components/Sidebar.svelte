<script lang="ts">
  import { store } from '../state.svelte';
  import { sortable } from '../dragsort.svelte';
  import { rollupWithUnread } from '../agent-status';
  import type { WorkspaceView } from '../proto';
  import StatusIcon from './StatusIcon.svelte';
  import Icon from './Icon.svelte';
  import ContextMenu, { type MenuItem } from './ContextMenu.svelte';

  let { onnew, folded }: { onnew: () => void; folded: boolean } = $props();

  /** A workspace shows the most urgent state among its panes; an unread
      completion keeps the aggregate dot solid. */
  function wsDot(ws: WorkspaceView) {
    void store.readRev;
    return rollupWithUnread(ws.tabs.flatMap((t) => t.panes));
  }

  function paneCount(ws: WorkspaceView): number {
    return ws.tabs.reduce((n, t) => n + t.panes.length, 0);
  }

  // Double-click a name to rename it — and the right-click menu offers it too,
  // so the gesture is discoverable rather than folklore.
  let editing = $state<number | null>(null);
  let draft = $state('');

  function beginRename(ws: WorkspaceView) {
    editing = ws.id;
    draft = ws.name;
  }

  function commitRename(ws: WorkspaceView) {
    if (editing !== ws.id) return;
    editing = null;
    if (draft.trim() !== ws.name) {
      store.send({ t: 'rename_workspace', ws: ws.id, name: draft });
    }
  }

  /** Closing takes its terminals with it, so it asks once. */
  function close(ws: WorkspaceView) {
    const panes = paneCount(ws);
    const ok =
      panes === 0 ||
      confirm(
        `Close “${ws.name}”?\n\n${panes} terminal${panes === 1 ? '' : 's'} will be closed.`,
      );
    if (ok) store.send({ t: 'close_workspace', ws: ws.id });
  }

  // Right-click opens a menu, not an instant delete — the old behaviour put
  // "destroy this workspace and its terminals" one stray click away, with no
  // way to discover Rename at all.
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  function openMenu(e: MouseEvent, ws: WorkspaceView) {
    if (!store.caps.host) return;
    e.preventDefault();
    menu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        // Folded, there is no name on screen to edit in place.
        ...(folded ? [] : [{ label: 'Rename', onselect: () => beginRename(ws) }]),
        { label: 'Close', danger: true, onselect: () => close(ws) },
      ],
    };
  }

  function focusInput(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
</script>

<aside class="sidebar" class:folded>
  <div class="sb-head">
    {#if !folded}Workspaces{/if}
    {#if store.caps.host}
      <button class="add" title="Open a workspace  ⌘N" aria-label="Open a workspace" onclick={onnew}>
        <Icon name="plus" size={15} />
      </button>
    {/if}
  </div>

  <div class="ws-list">
    {#each store.tree.workspaces as ws (ws.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="ws"
        use:sortable={{
          id: ws.id,
          axis: 'y',
          ignore: '.x',
          order: () => store.tree.workspaces.map((w) => w.id),
          commit: (order) => store.send({ t: 'reorder_workspaces', order }),
        }}
        class:active={ws.id === store.activeWs?.id}
        title={folded ? ws.name : undefined}
        onclick={() => store.activate(ws.id, null)}
        oncontextmenu={(e) => openMenu(e, ws)}
      >
        {#if folded}
          <!-- The initial stands in for the name, the badge for the count,
               and the dot keeps its corner so a running agent still shows. -->
          <span class="tile">
            {ws.name.slice(0, 1).toUpperCase()}
            <span class="badge">{ws.tabs.length}</span>
            <span class="tile-dot">
              <StatusIcon phase={wsDot(ws).phase} unread={wsDot(ws).unread} />
            </span>
          </span>
        {:else}
          <div class="row1">
            <StatusIcon phase={wsDot(ws).phase} unread={wsDot(ws).unread} />

            {#if editing === ws.id}
              <input
                class="rename"
                bind:value={draft}
                use:focusInput
                onclick={(e) => e.stopPropagation()}
                onblur={() => commitRename(ws)}
                onkeydown={(e) => {
                  if (e.key === 'Enter') commitRename(ws);
                  if (e.key === 'Escape') editing = null;
                }}
              />
            {:else}
              <span
                class="name"
                title={ws.path}
                ondblclick={(e) => {
                  if (!store.caps.host) return;
                  e.stopPropagation();
                  beginRename(ws);
                }}>{ws.name}</span
              >
              <span class="count">{ws.tabs.length}</span>
              {#if store.caps.host}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <span
                  class="x"
                  title="Close workspace"
                  aria-label="Close workspace"
                  onclick={(e) => {
                    e.stopPropagation();
                    close(ws);
                  }}><Icon name="x" size={13} /></span
                >
              {/if}
            {/if}
          </div>
          <div class="row2">
            <span class="branch">⎇ {ws.branch || 'main'}</span>
          </div>
        {/if}
      </div>
    {/each}
  </div>
</aside>

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .sidebar {
    width: 206px;
    flex: 0 0 206px;
    background: var(--panel);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
  }
  .sb-head {
    padding: 9px 11px;
    font-size: 10px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--faint);
    display: flex;
    align-items: center;
  }
  .add {
    margin-left: auto;
    color: var(--faint);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }
  .add:hover {
    color: var(--fg);
  }
  .ws-list {
    flex: 1;
    overflow-y: auto;
    padding: 0 6px;
  }
  .ws {
    padding: 7px 9px;
    border-radius: 7px;
    margin-bottom: 2px;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .ws:hover {
    background: var(--panel-2);
  }
  .ws.active {
    background: var(--panel-2);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  /* :global — the class is toggled by the sortable action, not the markup.
     Lifted rather than faded: the row is being carried, not disabled. */
  .ws:global(.dragging) {
    background: var(--panel-2);
    cursor: grabbing;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
    /* The transform is the drag itself; nothing else may animate it. */
    transition: none;
  }
  .row1 {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .name {
    font-size: 12.5px;
    font-weight: 500;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rename {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 1px 6px;
    font: 500 12.5px inherit;
  }
  .rename:focus {
    outline: none;
  }
  .row2 {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 10.5px;
    color: var(--faint);
  }
  .branch {
    font-family: ui-monospace, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 9.5px;
    color: var(--faint);
    background: var(--bg);
    border-radius: 9px;
    padding: 1px 6px;
  }
  /* Appears on hover, so the row stays quiet until you reach for it. */
  .x {
    color: var(--faint);
    display: inline-flex;
    align-items: center;
    opacity: 0;
    flex: 0 0 auto;
  }
  .ws:hover .x {
    opacity: 1;
  }
  .x:hover {
    color: var(--err);
  }

  /* Folded: a rail of initials, 162px handed back to the terminal. */
  .sidebar.folded {
    width: 44px;
    flex-basis: 44px;
  }
  .folded .sb-head {
    justify-content: center;
    padding-inline: 0;
  }
  .folded .add {
    margin-left: 0;
  }
  .folded .ws-list {
    padding: 0 4px;
  }
  .folded .ws {
    align-items: center;
    padding: 5px 0;
  }
  .folded .ws.active {
    box-shadow: none;
    background: none;
  }
  .tile {
    position: relative;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    font-weight: 600;
    background: var(--panel-2);
    color: var(--dim);
  }
  .ws:hover .tile {
    color: var(--fg);
  }
  .ws.active .tile {
    background: var(--accent);
    color: var(--bg);
  }
  .badge {
    position: absolute;
    top: -4px;
    right: -5px;
    min-width: 13px;
    height: 13px;
    padding: 0 3px;
    border-radius: 7px;
    background: var(--panel);
    border: 1px solid var(--border);
    color: var(--dim);
    font-size: 9px;
    font-weight: 700;
    line-height: 11px;
    text-align: center;
  }
  .tile-dot {
    position: absolute;
    bottom: -2px;
    left: -2px;
    display: flex;
  }

  /* A phone held upright. 44px is a tenth of the screen, and nobody picks a
     workspace while reading a terminal on one — so here folded means gone,
     and open means laid over the top, dismissed by tapping beside it. Same
     state, one rule; the width decides what folding costs. */
  @container body (max-width: 560px) {
    .sidebar.folded {
      display: none;
    }
    .sidebar:not(.folded) {
      position: absolute;
      top: 0;
      bottom: 0;
      left: 0;
      z-index: 20;
    }
  }
</style>
