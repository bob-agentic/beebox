<script lang="ts">
  import { store } from '../state.svelte';
  import { sortable } from '../dragsort.svelte';
  import { agentBadge, rollupWithUnread } from '../agent-status';
  import type { Shelf, TabId, TabView } from '../proto';
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
  // Only what is on the strip. Hibernated tabs are still in `ws.tabs` — they
  // are running — so every walk of the strip goes through the store's filter.
  const tabs = $derived(store.liveTabs);
  // Filing only. A shelved tab runs, is shared, and carries a status dot
  // exactly as it did on the strip — it has simply given up its place.
  const SHELVES = [
    { key: 'archive' as const, icon: 'archive' as const, label: 'Archive', hint: 'done with, kept to look back at' },
    { key: 'later' as const, icon: 'later' as const, label: 'Later', hint: 'not started yet' },
  ];

  /** What is on each shelf, and the dot rolled up from it. Same rollup the
      strip uses, so a shelved agent catches your eye the same way. */
  const shelves = $derived.by(() => {
    void store.readRev;
    return SHELVES.map((s) => {
      const tabs = store.shelved(s.key);
      return { ...s, tabs, dot: rollupWithUnread(tabs.flatMap((t) => t.panes)) };
    });
  });

  let openShelf = $state<Shelf | null>(null);
  let shelfPos = $state({ top: 0, left: 0 });
  function toggleShelf(e: MouseEvent, key: Shelf) {
    e.stopPropagation();
    if (openShelf === key) {
      openShelf = null;
      return;
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    shelfPos = { top: r.bottom + 6, left: r.left };
    openShelf = key;
  }
  function shelve(tab: TabId, shelf: Shelf | null) {
    store.send({ t: 'shelve_tab', tab, shelf });
  }

  // Once the strip overflows, a tab opened with ⌘T lands past the right edge:
  // the thing you just asked for is the one thing you cannot see.
  //
  // Typing counts too. Having scrolled the strip away to look at something
  // else and then started working, the tab you are actually in is the one
  // that should be on screen — so `typedRev` is read here as well, purely to
  // subscribe to it.
  //
  // Scrolled by hand rather than with `scrollIntoView`, which also scrolls
  // every scrollable ancestor — here that would drag the whole pane area.
  let strip: HTMLDivElement | undefined;
  let revealFrame = 0;
  $effect(() => {
    const id = store.activeTab?.id;
    void store.typedRev;
    if (id == null || !strip) return;
    // Coalesced to one frame: this runs on every keystroke, and a burst of
    // typing must not cost a layout read each. Deferring also lets a tab that
    // was opened this tick reach the DOM before it is measured.
    if (revealFrame) return;
    revealFrame = requestAnimationFrame(() => {
      revealFrame = 0;
      const el = strip?.querySelector<HTMLElement>(`[data-sort-id="${id}"]`);
      if (!el || !strip) return;
      const pad = 12;
      const left = el.offsetLeft - pad;
      const right = el.offsetLeft + el.offsetWidth + pad;
      // Already visible: leave the strip exactly where the user put it.
      if (left >= strip.scrollLeft && right <= strip.scrollLeft + strip.clientWidth) {
        return;
      }
      strip.scrollTo({
        left: left < strip.scrollLeft ? left : right - strip.clientWidth,
        behavior: 'smooth',
      });
    });
  });

  // A pending frame must not fire against a strip that is gone.
  $effect(() => () => {
    if (revealFrame) cancelAnimationFrame(revealFrame);
  });

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
    const tabs = store.liveTabs;
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

<div class="tabrow">
  <!-- Outside the scroller, not pinned inside it: the shelves own their width
       and the tabs simply have less room. Nothing of theirs can pass under
       them, which is the only way an edge this busy stays clean. -->
  {#if store.caps.host}
    <div class="shelves">
      {#each shelves as s (s.key)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="shelf shelf-{s.key}"
          data-shelf={s.key}
          class:empty={s.tabs.length === 0}
          title={s.tabs.length
            ? `${s.label} — ${s.tabs.length} ${s.hint}. Click to open, or drop a tab here`
            : `${s.label} — ${s.hint}. Drop a tab here`}
          onclick={(e) => toggleShelf(e, s.key)}
        >
          <!-- The dot is the icon: colour and breathing come from the same
               rollup the strip's own dots use, so there is nothing new to
               learn. The count beside it stays still — a number that pulses
               is hard to read. -->
          <span
            class="shelf-icon phase-{s.dot.phase}"
            class:unread={s.dot.unread}
          >
            <Icon name={s.icon} size={13} />
          </span>
          <span class="n">{s.tabs.length}</span>
        </div>
      {/each}
    </div>
  {/if}

  <div class="tabbar" bind:this={strip}>
    {#each tabs as tab (tab.id)}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="tab"
      use:sortable={{
        id: tab.id,
        axis: 'x',
        ignore: '.x',
        // Must match what the DOM holds, or the reorder guard silently
        // rejects every drag.
        order: () => tabs.map((t) => t.id),
        commit: (order) => ws && store.send({ t: 'reorder_tabs', ws: ws.id, order }),
        // Either shelf takes a drop; which one is read off the element.
        dropTarget: '.shelf',
        drop: (id, el) =>
          shelve(id, (el?.dataset.shelf as Shelf | undefined) ?? 'archive'),
      }}
      class:active={tab.id === store.activeTab?.id}
      onclick={() => ws && store.activate(ws.id, tab.id)}
      onauxclick={(e) => {
        if (e.button === 1 && store.caps.host) {
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
            if (!store.caps.host) return;
            e.stopPropagation();
            beginRename(tab);
          }}
          oncontextmenu={(e) => {
            // Right-click on a manually named tab offers the way back to the
            // automatic title; an empty rename is exactly that on the wire.
            if (!store.caps.host || !tab.title) return;
            e.preventDefault();
            store.send({ t: 'rename_tab', tab: tab.id, title: '' });
          }}
          title={tab.title ? 'Right-click: reset to auto title' : ''}
          >{labels.get(tab.id) ?? label(tab)}</span
        >
      {/if}
      {#if store.caps.host}
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

    {#if store.caps.may_open_tab && ws}
      <button
        class="add"
        title="New tab"
        aria-label="New tab"
        onclick={() => store.openTab(ws.id)}
      >
        <Icon name="plus" size={15} />
      </button>
    {/if}
  </div>
</div>

{#if openShelf}
  {@const cur = shelves.find((s) => s.key === openShelf)!}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="shelf-scrim" onclick={() => (openShelf = null)}></div>
  <div class="shelf-drawer" style="top:{shelfPos.top}px; left:{shelfPos.left}px">
    <div class="shelf-head">
      <span>{cur.label}{cur.tabs.length ? ` · ${cur.tabs.length}` : ''}</span>
      {#if cur.tabs.length > 1}
        <button
          class="take-all"
          onclick={() => {
            for (const t of cur.tabs) shelve(t.id, null);
            openShelf = null;
          }}>Take all back</button
        >
      {/if}
    </div>
    {#if cur.tabs.length === 0}
      <p class="shelf-empty">
        Nothing here — {cur.hint}. Drag a tab onto this icon to file it. It
        keeps running and stays shared; it just gives up its place on the
        strip.
      </p>
    {:else}
      {#each cur.tabs as tab (tab.id)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="shelf-row"
          onclick={() => {
            shelve(tab.id, null);
            openShelf = null;
          }}
        >
          <StatusIcon phase={tabDot(tab).phase} unread={tabDot(tab).unread} />
          {#if AGENT_MARK[agent(tab)]}
            <span class="mark" style="color:{AGENT_MARK[agent(tab)].color}">
              <Icon name={AGENT_MARK[agent(tab)].icon} size={11} />
            </span>
          {:else}
            <span class="agent" style={AGENT_STYLE[agent(tab)] ?? AGENT_STYLE.SH}>{agent(tab)}</span>
          {/if}
          <span class="shelf-label">{label(tab)}</span>
          <!-- The other shelf, one click away: a thing you meant to do and
               then did belongs in the archive, and moving it should not mean
               putting it back on the strip first. -->
          <button
            class="shelf-move"
            title={openShelf === 'archive' ? 'Move to Later' : 'Move to Archive'}
            onclick={(e) => {
              e.stopPropagation();
              shelve(tab.id, openShelf === 'archive' ? 'later' : 'archive');
            }}
          >
            <Icon name={openShelf === 'archive' ? 'later' : 'archive'} size={12} />
          </button>
          <span class="shelf-take">Take back</span>
        </div>
      {/each}
    {/if}
  </div>
{/if}

<style>
  /* The strip: a fixed area for the den, then everything else scrolls. */
  .tabrow {
    height: 34px;
    flex: 0 0 34px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: stretch;
    padding: 0 6px;
    gap: 3px;
    min-width: 0;
  }
  .tabbar {
    /* The offset parent for the tabs, so scrolling one into view is a
       subtraction against this scroller rather than against the page. */
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: stretch;
    gap: 3px;
    overflow-x: auto;
    scrollbar-width: none;
    padding-left: 8px;
    /* Tabs are simply clipped at the edges, as they are in any editor with a
       strip like this. A mask to soften the left edge was tried and removed:
       it interfered with scrolling, and a fade is not worth that. */
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
  /* Sticky, so it stays put — and stays droppable — however far the strip is
     scrolled. Above the tabs, which lift to z-index 5 while being dragged;
     a drop target you cannot see under the thing you are dropping is no
     target at all. */
  /* Its own fixed area, not something the tabs pass beneath: they scroll in
     the box to its right and simply have that much less room. */
  .shelves {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    align-self: center;
    gap: 2px;
    padding-right: 6px;
    margin-right: 2px;
    border-right: 1px solid var(--border);
  }
  .shelf {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 7px;
    border-radius: 7px;
    color: var(--dim);
    cursor: pointer;
    white-space: nowrap;
    background: var(--panel-2);
    border: 1px solid var(--border);
    transition: background 140ms ease, border-color 140ms ease, color 140ms ease;
  }
  .shelf:hover {
    color: var(--fg);
    border-color: var(--faint);
  }
  /* Nothing on it yet: present enough to be found, quiet enough to ignore. */
  .shelf.empty {
    color: var(--faint);
    border-style: dashed;
  }
  /* Lit while a tab is held over it — the only signal that letting go will
     do something. Toggled by the sortable action, not the markup. */
  .shelf:global(.drop-over) {
    color: var(--fg);
    background: color-mix(in srgb, var(--accent) 18%, var(--panel-2));
    border-color: var(--accent);
  }
  .shelf .n {
    font-size: 10.5px;
    font-weight: 700;
    min-width: 9px;
    text-align: center;
  }

  /* The icon carries the status, in the same colours the strip's dots use —
     there is no second vocabulary to learn. Only the icon breathes; the count
     beside it holds still, because a number that pulses is hard to read. */
  .shelf-icon {
    display: inline-flex;
    line-height: 0;
  }
  .shelf-icon.phase-running,
  .shelf-icon.phase-needs_input {
    color: var(--accent);
    animation: shelf-breathe 1.8s ease-in-out infinite;
  }
  .shelf-icon.phase-success.unread {
    color: var(--accent);
  }
  .shelf-icon.phase-failed.unread {
    color: var(--danger, #f85149);
  }
  @keyframes shelf-breathe {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .shelf-icon {
      animation: none !important;
    }
  }

  /* Catches the click-away without dimming: a shelf is not a modal. */
  .shelf-scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  /* Fixed and positioned from the strip's own box: the tab bar's ancestors
     carry no positioning, and `.tabbar` itself clips on the x axis. */
  .shelf-drawer {
    position: fixed;
    z-index: 31;
    width: 330px;
    max-height: 60vh;
    overflow-y: auto;
    padding: 5px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 10px 34px rgba(0, 0, 0, 0.5);
  }
  .shelf-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 7px 9px 9px;
    font-size: 10px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--faint);
    font-weight: 600;
  }
  .take-all {
    font-size: 10.5px;
    color: var(--faint);
    text-transform: none;
    letter-spacing: 0;
  }
  .take-all:hover {
    color: var(--fg);
  }
  .shelf-row {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 9px;
    border-radius: 7px;
    font-size: 12px;
    color: var(--dim);
    cursor: pointer;
  }
  .shelf-row:hover {
    background: var(--panel-2);
    color: var(--fg);
  }
  .shelf-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .shelf-take {
    opacity: 0;
    font-size: 10.5px;
    color: var(--accent);
  }
  .shelf-row:hover .shelf-take {
    opacity: 1;
  }
  .shelf-empty {
    margin: 0;
    padding: 4px 9px 12px;
    color: var(--faint);
    font-size: 11.5px;
    line-height: 1.6;
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
