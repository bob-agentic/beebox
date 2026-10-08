<script lang="ts" module>
  /** How long the sidebar takes to fold; App times its scrim and fit hold by it. */
  export const FOLD_MS = 240;
</script>

<script lang="ts">
  import { slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { store } from '../state.svelte';
  import { sortable } from '../dragsort.svelte';
  import { rollupWithUnread } from '../agent-status';
  import type { WorkspaceView } from '../proto';
  import StatusIcon from './StatusIcon.svelte';
  import Icon from './Icon.svelte';
  import ContextMenu, { type MenuItem } from './ContextMenu.svelte';
  import WorktreeCloseDialog from './WorktreeCloseDialog.svelte';
  import { groupWorkspaces, orderFromGroups, orderWithKids } from '../ws-groups';

  let {
    onnew,
    ontoggle,
    folded,
  }: { onnew: () => void; ontoggle: () => void; folded: boolean } = $props();

  /** A workspace shows the most urgent state among its panes; an unread
      completion keeps the aggregate dot solid. Frozen tabs run nothing, so
      whatever their panes last said is left out. */
  function wsDot(ws: WorkspaceView) {
    void store.readRev;
    return rollupWithUnread(ws.tabs.filter((t) => t.shelf !== 'freezer').flatMap((t) => t.panes));
  }

  /** Worktrees sit under their repository; see ws-groups.ts. */
  const groups = $derived(groupWorkspaces(store.tree.workspaces));

  /** A group's dot. Open, the worktrees show their own, so the repository
      speaks only for itself; folded, it speaks for all of them. */
  function groupDot(g: { ws: WorkspaceView; kids: WorkspaceView[] }) {
    return isOpen(g.ws.id) ? wsDot(g.ws) : rollupAll([g.ws, ...g.kids]);
  }
  function rollupAll(list: WorkspaceView[]) {
    void store.readRev;
    return rollupWithUnread(
      list.flatMap((w) => w.tabs.filter((t) => t.shelf !== 'freezer').flatMap((t) => t.panes)),
    );
  }

  // Folded groups, by repository workspace id. Local to this browser, like
  // read-state: how you like your sidebar is not the daemon's business.
  const FOLD_KEY = 'beebox.folded-groups';
  let closedGroups = $state(new Set<number>(JSON.parse(localStorage.getItem(FOLD_KEY) ?? '[]')));
  function isOpen(id: number) {
    return !closedGroups.has(id);
  }
  function toggleGroup(id: number) {
    const next = new Set(closedGroups);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    closedGroups = next;
    localStorage.setItem(FOLD_KEY, JSON.stringify([...next]));
  }

  // Workspaces opened from the `beebox` command arrive behind what you are
  // looking at, so the sidebar is the only place they show up. A brief glow
  // there says where. Not on the first frame: that is just the list loading.
  let known: Set<number> | null = null;
  let born = $state(new Set<number>());
  $effect(() => {
    const ids = store.tree.workspaces.map((w) => w.id);
    if (known === null) {
      if (store.connected) known = new Set(ids);
      return;
    }
    const fresh = ids.filter((id) => !known!.has(id) && id !== store.activeWs?.id);
    known = new Set(ids);
    if (!fresh.length) return;
    born = new Set([...born, ...fresh]);
    setTimeout(() => {
      born = new Set([...born].filter((id) => !fresh.includes(id)));
    }, 2400);
  });

  /** A worktree being closed: the dialog that deletes it. */
  let removing = $state<WorkspaceView | null>(null);

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

  /** Closing takes its terminals with it, so it asks once. A worktree goes
      with them, so for one it asks properly. */
  function close(ws: WorkspaceView) {
    if (ws.worktree) {
      removing = ws;
      return;
    }
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
        ws.worktree
          ? { label: 'Delete worktree…', danger: true, onselect: () => close(ws) }
          : { label: 'Close', danger: true, onselect: () => close(ws) },
      ],
    };
  }

  function focusInput(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
</script>

<!-- Keyed on the fold, so the rail and the full list are two elements: one
     slides shut while the other slides open, and Svelte runs both. -->
{#key folded}
<aside class="sidebar" class:folded transition:slide={{ axis: 'x', duration: FOLD_MS, easing: cubicOut }}>
  <div class="sb-head">
    {#if !folded}Workspaces{/if}
    <button
      class="fold"
      title="Workspaces  ⌘B"
      aria-label={folded ? 'Show workspaces' : 'Hide workspaces'}
      onclick={ontoggle}
    >
      <Icon name="sidebar" size={15} />
    </button>
  </div>

  <div class="ws-list">
    {#each groups as g (g.ws.id)}
      {@const ws = g.ws}
      {@const open = isOpen(ws.id)}
      {@const dot = groupDot(g)}
      <div
        class="group"
        use:sortable={{
          id: ws.id,
          axis: 'y',
          ignore: '.x, .caret, .kids',
          order: () => groups.map((x) => x.ws.id),
          commit: (top) => store.send({ t: 'reorder_workspaces', order: orderFromGroups(groups, top) }),
        }}
      >
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="ws"
          class:active={ws.id === store.activeWs?.id ||
            (!open && !folded && g.kids.some((k) => k.id === store.activeWs?.id))}
          class:born={born.has(ws.id)}
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
                <StatusIcon phase={dot.phase} unread={dot.unread} />
              </span>
            </span>
          {:else}
            <div class="row1">
              {#if g.kids.length}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <span
                  class="caret"
                  class:shut={!open}
                  role="button"
                  tabindex="-1"
                  aria-label={open ? 'Fold worktrees' : 'Show worktrees'}
                  onclick={(e) => {
                    e.stopPropagation();
                    toggleGroup(ws.id);
                  }}>▾</span
                >
              {/if}
              <StatusIcon phase={dot.phase} unread={dot.unread} />
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
                    title={ws.worktree ? 'Delete worktree' : 'Close workspace'}
                    aria-label={ws.worktree ? 'Delete worktree' : 'Close workspace'}
                    onclick={(e) => {
                      e.stopPropagation();
                      close(ws);
                    }}><Icon name="x" size={13} /></span
                  >
                {/if}
              {/if}
            </div>
            <div class="row2">
              {#if ws.branch}<span class="branch">⎇ {ws.branch}</span>{/if}
              {#if g.kids.length && !open}
                <!-- Folded, the group still says how its worktrees are doing. -->
                <span class="sum">
                  {g.kids.length} worktree{g.kids.length === 1 ? '' : 's'}
                  {#each g.kids as k (k.id)}
                    {@const kd = wsDot(k)}
                    <StatusIcon phase={kd.phase} unread={kd.unread} />
                  {/each}
                </span>
              {/if}
            </div>
          {/if}
        </div>

        {#if g.kids.length && (open || folded)}
          <div class="kids" transition:slide={{ duration: 160, easing: cubicOut }}>
            {#each g.kids as k (k.id)}
              {@const kd = wsDot(k)}
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="wt"
                class:active={k.id === store.activeWs?.id}
                class:born={born.has(k.id)}
                title={folded ? k.name : k.path}
                use:sortable={{
                  id: k.id,
                  axis: 'y',
                  ignore: '.x',
                  order: () => g.kids.map((x) => x.id),
                  commit: (kids) => store.send({ t: 'reorder_workspaces', order: orderWithKids(groups, ws.id, kids) }),
                }}
                onclick={() => store.activate(k.id, null)}
                oncontextmenu={(e) => openMenu(e, k)}
              >
                {#if folded}
                  <span class="subtile"><StatusIcon phase={kd.phase} unread={kd.unread} /></span>
                {:else}
                  <StatusIcon phase={kd.phase} unread={kd.unread} />
                  {#if editing === k.id}
                    <input
                      class="rename"
                      bind:value={draft}
                      use:focusInput
                      onclick={(e) => e.stopPropagation()}
                      onblur={() => commitRename(k)}
                      onkeydown={(e) => {
                        if (e.key === 'Enter') commitRename(k);
                        if (e.key === 'Escape') editing = null;
                      }}
                    />
                  {:else}
                    <span
                      class="wt-name"
                      ondblclick={(e) => {
                        if (!store.caps.host) return;
                        e.stopPropagation();
                        beginRename(k);
                      }}>{k.name}</span
                    >
                    <span class="count">{k.tabs.length}</span>
                    {#if store.caps.host}
                      <!-- svelte-ignore a11y_click_events_have_key_events -->
                      <span
                        class="x"
                        title="Delete worktree"
                        aria-label="Delete worktree"
                        onclick={(e) => {
                          e.stopPropagation();
                          close(k);
                        }}><Icon name="x" size={13} /></span
                      >
                    {/if}
                  {/if}
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </div>
  <!-- At the foot, where a list grows towards: the next workspace goes below
       the last one. -->
  {#if store.caps.host}
    <button class="add" title="Open a workspace  ⌘N" aria-label="Open a workspace" onclick={onnew}>
      <Icon name="plus" size={14} />
      {#if !folded}<span>Open workspace</span>{/if}
    </button>
  {/if}
</aside>
{/key}

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}
{#if removing}
  <WorktreeCloseDialog ws={removing} onclose={() => (removing = null)} />
{/if}

<style>
  /* Width, not flex-basis: the slide animates width, and a basis would pin it. */
  .sidebar {
    width: 206px;
    flex-shrink: 0;
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
  .fold {
    margin-left: auto;
    color: var(--faint);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }
  .fold:hover {
    color: var(--fg);
  }
  .add {
    margin: 6px;
    padding: 7px 9px;
    border-radius: 7px;
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--faint);
    font-size: 12px;
  }
  .add:hover {
    background: var(--panel-2);
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
     Lifted rather than faded: the row is being carried, not disabled. A
     group carries its worktrees with it. */
  .group:global(.dragging),
  .wt:global(.dragging) {
    background: var(--panel-2);
    cursor: grabbing;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
    /* The transform is the drag itself; nothing else may animate it. */
    transition: none;
  }
  .group {
    border-radius: 7px;
  }
  /* Arrived from the `beebox` command, behind what you were looking at. */
  .born {
    animation: born 2.4s cubic-bezier(0.33, 1, 0.68, 1);
  }
  @keyframes born {
    0%,
    30% {
      background: color-mix(in srgb, var(--accent) 22%, transparent);
    }
    100% {
      background: transparent;
    }
  }
  .caret {
    width: 10px;
    margin-right: -2px;
    flex: 0 0 auto;
    color: var(--faint);
    font-size: 9px;
    text-align: center;
    transition: transform 0.18s cubic-bezier(0.33, 1, 0.68, 1);
  }
  .caret:hover {
    color: var(--fg);
  }
  .caret.shut {
    transform: rotate(-90deg);
  }
  .sum {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
  }
  .kids {
    margin: 0 0 4px 13px;
    padding-left: 4px;
    border-left: 1px solid var(--border);
  }
  .wt {
    padding: 5px 8px;
    border-radius: 6px;
    margin-bottom: 1px;
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }
  .wt:hover {
    background: var(--panel-2);
  }
  .wt.active {
    background: var(--panel-2);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .wt:hover .x {
    opacity: 1;
  }
  .wt-name {
    flex: 1;
    min-width: 0;
    font: 11.5px ui-monospace, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wt .count {
    font-size: 9px;
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
  }
  .folded .sb-head {
    justify-content: center;
    padding-inline: 0;
  }
  .folded .fold {
    margin-left: 0;
  }
  .folded .add {
    margin: 6px 4px;
    padding: 7px 0;
    justify-content: center;
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
  /* Folded, a worktree is its dot on a small tile under the repository's. */
  .folded .kids {
    margin: 0;
    padding: 0;
    border: 0;
  }
  .folded .wt {
    justify-content: center;
    padding: 2px 0;
  }
  .folded .wt.active {
    box-shadow: none;
    background: none;
  }
  .subtile {
    width: 20px;
    height: 20px;
    border-radius: 6px;
    background: var(--panel-2);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .wt.active .subtile {
    box-shadow: inset 0 0 0 1.5px var(--accent);
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
