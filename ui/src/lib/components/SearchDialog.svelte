<script lang="ts">
  // ⇧⌘F: find something said in a past Claude conversation, then go back to
  // it. Results are grouped by conversation; picking one swaps the list for
  // the whole conversation in place, and ← swaps it back. Only from there can
  // it be opened. Nothing is kept once the dialog closes but the words, so
  // reopening searches again rather than holding results nobody may look at.
  import { tick } from 'svelte';
  import { store } from '../state.svelte';
  import { writeClipboard } from '../clipboard';
  import {
    clock,
    conversationItems,
    hasWord,
    markWords,
    queryWords,
    resumeCommand,
    span,
    when,
    whereIs,
    WHERE_LABEL,
  } from '../conv-search';
  import { renderMd } from '../md';
  import type { ConvMsg, SearchSession, Who, WorkspaceView } from '../proto';
  import Icon from './Icon.svelte';

  let { onclose }: { onclose: () => void } = $props();

  /** The workspace in front when the dialog opened: what "here" means. */
  const here = store.activeWs;
  const SHOWN_PER_GROUP = 3;

  let q = $state(store.lastSearch);
  let scope = $state<'ws' | 'all'>(here ? 'ws' : 'all');
  let who = $state<'all' | 'user' | 'ai'>('all');
  let sessions = $state<SearchSession[]>([]);
  let took = $state<number | null>(null);
  let busy = $state(false);
  /** The connection dropped before an answer came. */
  let lost = $state(false);
  let sel = $state(0);
  let peek = $state(false);
  let conv = $state<ConvMsg[] | null>(null);
  /** The message the conversation view is on. */
  let cur = $state<string | null>(null);
  let copied = $state(false);
  let input: HTMLInputElement;
  let reader = $state<HTMLDivElement>();
  let list = $state<HTMLDivElement>();

  const words = $derived(queryWords(q));
  const shows = (w: Who) => who === 'all' || (who === 'user' ? w === 'user' : w !== 'user');
  const groups = $derived(
    sessions
      .map((s) => {
        const hits = s.hits.filter((h) => shows(h.who));
        const all = who === 'all' ? s.total : hits.length;
        return { s, hits: hits.slice(0, SHOWN_PER_GROUP), more: all - Math.min(hits.length, SHOWN_PER_GROUP) };
      })
      .filter((g) => g.hits.length),
  );
  /** Across workspaces the conversations are filed under the workspace they
      belong to, workspaces in the order of their newest hit. */
  const sections = $derived.by(() => {
    if (scope !== 'all') return [{ key: 'here', ws: null as WorkspaceView | null, label: '', groups }];
    const out: { key: string; ws: WorkspaceView | null; label: string; groups: typeof groups }[] = [];
    for (const g of groups) {
      const ws = wsOf(g.s) ?? null;
      const key = ws ? `ws:${ws.id}` : `cwd:${g.s.cwd}`;
      let sec = out.find((x) => x.key === key);
      if (!sec) out.push((sec = { key, ws, label: ws?.name ?? tilde(g.s.cwd), groups: [] }));
      sec.groups.push(g);
    }
    return out;
  });
  /** Every selectable row, in the order shown, for ↑↓. */
  const rows = $derived(sections.flatMap((x) => x.groups.flatMap((g) => g.hits.map((h) => ({ s: g.s, h })))));
  const row = $derived(rows[sel]);
  // A filter can leave the selection past the end: back to the first row.
  $effect(() => {
    if (sel >= rows.length && sel !== 0) sel = 0;
  });
  const hitCount = $derived(sessions.reduce((n, s) => n + s.total, 0));

  /** In the conversation view: the messages the words are in, in order. */
  const marks = $derived(conv ? conv.filter((m) => shows(m.who) && hasWord(m.text, words)).map((m) => m.key) : []);
  const items = $derived(conv ? conversationItems(conv) : []);
  /** Rendered once per conversation and query, not on every ↑↓. */
  const html = $derived.by(() => {
    const out = new Map<string, string>();
    for (const m of conv ?? []) if (m.who !== 'tool') out.set(m.key, renderMd(m.text, words));
    return out;
  });

  // Search as you type, a beat after the last key. An answer to an older
  // query is dropped: it may land after a newer one.
  let seq = 0;
  $effect(() => {
    const text = q.trim();
    const ws = scope === 'ws' ? (here?.id ?? null) : null;
    const mine = ++seq;
    if (!text) {
      sessions = [];
      took = null;
      busy = false;
      return;
    }
    busy = true;
    const t = setTimeout(async () => {
      const r = await store.searchConversations(text, ws);
      if (mine !== seq) return;
      busy = false;
      if (!r) {
        lost = true;
        return;
      }
      lost = false;
      sessions = r.sessions;
      took = r.took_ms;
      sel = 0;
      peek = false;
    }, 150);
    return () => clearTimeout(t);
  });

  // The last three conversations read, for going back and forth between
  // results without asking again. Dropped with the dialog.
  const cache = new Map<string, ConvMsg[]>();

  async function openPeek(i = sel) {
    sel = i;
    const r = rows[i];
    if (!r) return;
    peek = true;
    copied = false;
    cur = r.h.key;
    input.blur();
    const have = cache.get(r.s.session);
    if (have) {
      cache.delete(r.s.session);
      cache.set(r.s.session, have);
      conv = have;
    } else {
      conv = null;
      const msgs = await store.conversation(r.s.session);
      if (!msgs) {
        lost = true;
        if (peek) back();
        return;
      }
      cache.set(r.s.session, msgs);
      while (cache.size > 3) cache.delete(cache.keys().next().value!);
      if (peek && row?.s.session === r.s.session) conv = msgs;
    }
    await scrollToCur();
  }

  function back() {
    peek = false;
    conv = null;
    input.focus();
    input.setSelectionRange(q.length, q.length);
  }

  /** The current message a third of the way down, so what led to it shows. */
  async function scrollToCur() {
    await tick();
    const el = reader?.querySelector<HTMLElement>(`[data-key="${CSS.escape(cur ?? '')}"]`);
    if (!reader || !el) return;
    const off = el.getBoundingClientRect().top - reader.getBoundingClientRect().top;
    reader.scrollTop += off - reader.clientHeight * 0.3;
  }

  function step(d: 1 | -1) {
    const i = marks.indexOf(cur ?? '');
    const next = marks[i < 0 ? 0 : i + d];
    if (next) {
      cur = next;
      void scrollToCur();
    }
  }

  function close() {
    store.lastSearch = q;
    onclose();
  }

  function open() {
    const r = row;
    if (!r) return;
    store.openConversation(r.s.session);
    close();
  }

  async function copy() {
    const r = row;
    if (r && (await writeClipboard(resumeCommand(r.s.cwd, r.s.session)))) copied = true;
  }

  function onkeydown(e: KeyboardEvent) {
    const atEnd = document.activeElement !== input || input.selectionStart === input.value.length;
    if (e.key === 'Escape') {
      e.preventDefault();
      if (peek) back();
      else close();
    } else if (e.key === 'Enter') {
      // In the list it reads the conversation; only from there is it opened.
      e.preventDefault();
      if (peek) open();
      else void openPeek();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const d = e.key === 'ArrowDown' ? 1 : -1;
      if (peek) step(d);
      else sel = Math.max(0, Math.min(rows.length - 1, sel + d));
    } else if (e.key === 'ArrowRight' && !peek && rows.length && atEnd) {
      e.preventDefault();
      void openPeek();
    } else if (e.key === 'ArrowLeft' && peek) {
      e.preventDefault();
      back();
    } else if (e.key === 'Tab' && here) {
      e.preventDefault();
      scope = scope === 'ws' ? 'all' : 'ws';
    } else if (e.metaKey && e.key.toLowerCase() === 'c' && peek && !getSelection()?.toString()) {
      e.preventDefault();
      void copy();
    } else if (peek && e.key.length === 1 && !e.metaKey && !e.ctrlKey) {
      // Typing in the conversation view is a new search: the key lands in
      // the field, which takes it back to the list.
      input.focus();
    }
  }

  $effect(() => {
    // Keep the selection in view as ↑↓ move it.
    void sel;
    if (!peek) void tick().then(() => list?.querySelector('.sel')?.scrollIntoView({ block: 'nearest' }));
  });

  $effect(() => {
    input.focus();
    input.select();
  });

  const tilde = (p: string) => p.replace(/^\/Users\/[^/]+/, '~');

  /** Like ⌘↑/⌘↓ in a terminal: the question before or after the one at the
      top of the view. Buttons only — the keys already step through hits. */
  function question(d: 1 | -1) {
    if (!reader) return;
    const top = reader.getBoundingClientRect().top;
    const asks = [...reader.querySelectorAll<HTMLElement>('.msg.me')].map((el) => ({
      el,
      at: el.getBoundingClientRect().top - top,
    }));
    const next = d > 0 ? asks.find((a) => a.at > 12) : asks.filter((a) => a.at < -4).pop();
    if (next) reader.scrollTop += next.at - 8;
  }

  function wsOf(s: SearchSession) {
    return store.tree.workspaces.find((x) => x.id === s.ws);
  }

  /** Where Open lands: its workspace, or the folder it ran in, which becomes one. */
  function home(s: SearchSession) {
    return wsOf(s)?.name ?? (s.cwd.split('/').filter(Boolean).pop() || s.cwd);
  }
</script>

{#snippet marked(text: string)}{#each markWords(text, words) as p}{#if p.hit}<mark>{p.text}</mark>{:else}{p.text}{/if}{/each}{/snippet}

{#snippet badge(s: SearchSession)}
  {@const w = whereIs(s)}
  <span class="badge {w}">{WHERE_LABEL[w]}</span>
{/snippet}

{#snippet place(s: SearchSession)}
  {@const w = wsOf(s)}
  <span class="place" title={s.cwd}
    >{#if w?.worktree}⑂ {/if}{w?.name ?? tilde(s.cwd)}</span
  >
{/snippet}

<svelte:window {onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="pop" class:compact={!q.trim()} class:tall={peek}>
    <div class="bar">
      <span class="ico"><Icon name="search" size={15} /></span>
      <input
        bind:this={input}
        bind:value={q}
        oninput={() => peek && back()}
        placeholder="Search past Claude conversations"
        spellcheck="false"
        autocapitalize="off"
      />
      {#if here}
        <div class="seg">
          <button class:on={scope === 'ws'} onclick={() => (scope = 'ws')}>{here.name}</button>
          <button class:on={scope === 'all'} onclick={() => (scope = 'all')}>All</button>
        </div>
      {/if}
    </div>

    {#if !q.trim()}
      <div class="hint">Words you remember — several finds any of them.</div>
    {:else if peek && row}
      {@const s = row.s}
      <div class="rdh">
        <button class="back" onclick={back}>‹ Results</button>
        <Icon name="claude" size={13} />
        <span class="name">{s.title || s.session}</span>
        {@render badge(s)}
        <span class="sub">{@render place(s)}<span>·</span><span>{span(s.first_ms, s.last_ms)}</span></span>
        <span class="nav">
          {#if marks.length}<span class="count" title="Hits — ↑↓ step through them">{marks.indexOf(cur ?? '') + 1} / {marks.length}</span>{/if}
          <button class="qnav" title="Previous question" aria-label="Previous question" onclick={() => question(-1)}
            ><Icon name="arrow-up" size={13} /></button
          >
          <button class="qnav" title="Next question" aria-label="Next question" onclick={() => question(1)}
            ><Icon name="arrow-down" size={13} /></button
          >
        </span>
      </div>
      <div class="sheet chat" bind:this={reader}>
        {#if !conv}
          <div class="empty">Loading…</div>
        {:else}
          {#each items as it (it.key)}
            {#if it.kind === 'day'}
              <div class="day">{it.label}</div>
            {:else if it.kind === 'tools'}
              <details class="tools" open={it.ms.some((m) => m.key === cur)}>
                <summary>⚙ {it.ms.length} tool {it.ms.length === 1 ? 'call' : 'calls'}</summary>
                {#each it.ms as m (m.key)}
                  <div class="tool" class:cur={m.key === cur} data-key={m.key}>{@render marked(m.text)}</div>
                {/each}
              </details>
            {:else}
              {@const m = it.m}
              <div class="msg" class:me={m.who === 'user'} data-key={m.key}>
                <div class="h">{m.who === 'user' ? clock(m.at_ms) : `Claude · ${clock(m.at_ms)}`}</div>
                <div class="bub md" class:cur={m.key === cur}>{@html html.get(m.key)}</div>
              </div>
            {/if}
          {/each}
        {/if}
      </div>
      <div class="foot">
        <span class="sp"></span>
        <button class="ghost" class:done={copied} onclick={copy} title={resumeCommand(s.cwd, s.session)}
          >{copied ? '✓ Copied' : 'Copy command'}</button
        >
        <button class="primary" onclick={open}>Open in {home(s)}</button>
      </div>
    {:else}
      <div class="sheet list" bind:this={list}>
        {#if !busy && took != null && !rows.length}
          <div class="empty">Nothing found. Try other words, several at once, or All.</div>
        {/if}
        {#each sections as sec (sec.key)}
          {#if sec.label}
            <div class="wsh" title={sec.ws?.path}>
              <Icon name="folder" size={12} /><span class="wsname">{#if sec.ws?.worktree}⑂ {/if}{sec.label}</span>
              <span class="n">{sec.groups.length}</span>
            </div>
          {/if}
          {#each sec.groups as g (g.s.session)}
            <div class="grp">
              <div class="gh">
                <Icon name="claude" size={13} /><span class="name">{g.s.title || g.s.session}</span>{@render badge(
                  g.s,
                )}
              </div>
              <div class="gsub">{#if !sec.label}{@render place(g.s)}<span>·</span>{/if}<span>{span(g.s.first_ms, g.s.last_ms)}</span></div>
              {#each g.hits as h (h.key)}
                {@const i = rows.findIndex((r) => r.h === h)}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <div class="hit" class:sel={i === sel} onclick={() => openPeek(i)} onmouseenter={() => (sel = i)}>
                  <span class="who-tag"
                    >{#if h.who === 'user'}You{:else if h.who === 'tool'}⚙{:else}<Icon name="claude" size={11} />{/if}</span
                  >
                  <span class="snip">{@render marked(h.text)}</span>
                  <span class="t">{when(h.at_ms)}</span>
                </div>
              {/each}
              {#if g.more > 0}<div class="more">{g.more} more</div>{/if}
            </div>
          {/each}
        {/each}
      </div>
      <div class="foot">
        <span
          >{#if busy}Searching…{:else if lost}Disconnected — type to search again{:else if took != null}{hitCount} hits · {sessions.length}
            {sessions.length === 1 ? 'conversation' : 'conversations'} · {(took / 1000).toFixed(2)}s{/if}</span
        >
        <span class="sp"></span>
        <div class="seg sm">
          <button class:on={who === 'all'} onclick={() => (who = 'all')}>All</button>
          <button class:on={who === 'user'} onclick={() => (who = 'user')}>You</button>
          <button class:on={who === 'ai'} onclick={() => (who = 'ai')}>Claude</button>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  /* Two layers, both from the theme: the frame in --panel-2, the sheet the
     results and the conversation sit on in --panel. No rules between. */
  .mask {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.3);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 10vh;
    z-index: 50;
  }
  .pop {
    width: min(720px, calc(100vw - 48px));
    max-height: min(640px, calc(100vh - 20vh));
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.35);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .pop.tall {
    height: min(640px, calc(100vh - 20vh));
  }
  button {
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
    cursor: pointer;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
  }
  .ico {
    color: var(--faint);
    display: flex;
  }
  input {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    outline: 0;
    color: var(--fg);
    font: 16px -apple-system, 'PingFang SC', sans-serif;
    user-select: text;
    -webkit-user-select: text;
  }
  input::placeholder {
    color: var(--faint);
  }
  .hint {
    padding: 0 16px 13px 41px;
    font-size: 12px;
    color: var(--faint);
  }
  .seg {
    display: flex;
    background: var(--panel);
    border-radius: 7px;
    padding: 2px;
    font-size: 11.5px;
    flex-shrink: 0;
  }
  .seg button {
    padding: 3px 9px;
    border-radius: 5px;
    color: var(--dim);
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .seg button.on {
    background: var(--bg);
    color: var(--fg);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
  }
  .seg.sm {
    font-size: 11px;
  }
  .seg.sm button {
    padding: 2px 8px;
  }

  .sheet {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    background: var(--panel);
    border-radius: 10px;
    margin: 0 6px;
  }
  .list {
    padding: 6px;
  }
  .empty {
    padding: 28px;
    text-align: center;
    color: var(--faint);
    font-size: 12.5px;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    font-size: 11px;
    color: var(--faint);
  }
  .foot .sp {
    flex: 1;
  }
  mark,
  .md :global(mark) {
    background: color-mix(in srgb, var(--wait) 30%, transparent);
    color: inherit;
    border-radius: 2px;
  }
  .badge {
    font-size: 10.5px;
    padding: 0 6px;
    border-radius: 9px;
    white-space: nowrap;
    border: 1px solid var(--border);
    color: var(--dim);
    flex-shrink: 0;
  }
  .badge.open {
    color: var(--run);
    border-color: color-mix(in srgb, var(--run) 45%, transparent);
  }
  .badge.freezer {
    color: var(--done);
    border-color: color-mix(in srgb, var(--done) 45%, transparent);
  }
  .badge.archive,
  .badge.later {
    color: var(--wait);
    border-color: color-mix(in srgb, var(--wait) 45%, transparent);
  }
  .place {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .grp {
    padding: 6px 0;
  }
  .wsh {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 8px 2px;
    font-size: 11px;
    font-weight: 600;
    color: var(--dim);
    letter-spacing: 0.02em;
  }
  .wsh:first-child {
    padding-top: 2px;
  }
  .wsh .wsname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wsh .n {
    font-weight: 400;
    color: var(--faint);
    background: var(--panel-2);
    border-radius: 8px;
    padding: 0 6px;
  }
  .wsh ~ .grp {
    margin-left: 10px;
  }
  .gh,
  .rdh {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    color: #d97757;
  }
  .gh {
    padding: 2px 8px;
  }
  .name {
    color: var(--fg);
    font-weight: 600;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .gsub {
    display: flex;
    gap: 4px;
    padding: 0 8px 3px 28px;
    font-size: 11px;
    color: var(--faint);
  }
  .hit {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 5px 8px 5px 26px;
    border-radius: 7px;
    font-size: 12.5px;
    cursor: pointer;
  }
  .hit.sel {
    background: var(--panel-2);
  }
  .who-tag {
    width: 26px;
    flex: 0 0 auto;
    font-size: 10.5px;
    color: var(--faint);
  }
  .snip {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--dim);
  }
  .hit.sel .snip {
    color: var(--fg);
  }
  .t {
    font-size: 11px;
    color: var(--faint);
    white-space: nowrap;
  }
  .more {
    padding: 1px 8px 0 32px;
    font-size: 11px;
    color: var(--faint);
  }

  .rdh {
    padding: 0 16px 10px;
  }
  .back {
    color: var(--dim);
    padding: 2px 6px 2px 2px;
    border-radius: 6px;
    flex-shrink: 0;
  }
  .back:hover {
    background: var(--panel);
    color: var(--fg);
  }
  .sub {
    display: flex;
    gap: 4px;
    min-width: 0;
    font-size: 11px;
    color: var(--faint);
    white-space: nowrap;
    overflow: hidden;
  }
  .nav {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: 11px;
    color: var(--dim);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .count {
    margin-right: 6px;
  }
  .qnav {
    display: flex;
    padding: 3px;
    border-radius: 5px;
    color: var(--dim);
  }
  .qnav:hover {
    background: var(--panel);
    color: var(--fg);
  }

  .chat {
    padding: 12px 14px 28px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    user-select: text;
    -webkit-user-select: text;
  }
  .day {
    align-self: center;
    font-size: 10.5px;
    color: var(--faint);
    background: var(--panel-2);
    border-radius: 9px;
    padding: 1px 9px;
  }
  .msg {
    display: flex;
    flex-direction: column;
    max-width: 84%;
    align-self: flex-start;
  }
  .msg.me {
    align-self: flex-end;
    align-items: flex-end;
  }
  .msg .h {
    font-size: 10.5px;
    color: var(--faint);
    margin: 0 4px 2px;
  }
  .bub {
    border-radius: 14px;
    border-bottom-left-radius: 4px;
    padding: 8px 12px;
    font-size: 12.5px;
    line-height: 1.6;
    overflow-wrap: anywhere;
    background: var(--bg);
    border: 1px solid var(--border);
    min-width: 0;
    max-width: 100%;
  }
  .me .bub {
    background: color-mix(in srgb, var(--accent) 16%, var(--panel));
    border-color: color-mix(in srgb, var(--accent) 28%, var(--panel));
    border-radius: 14px;
    border-bottom-right-radius: 4px;
  }
  .bub.cur {
    box-shadow: 0 0 0 2px var(--wait);
  }
  .tools {
    align-self: flex-start;
    font-size: 11px;
    color: var(--faint);
    padding: 0 6px;
    max-width: 100%;
  }
  .tools summary {
    cursor: pointer;
    list-style: none;
  }
  .tools summary::-webkit-details-marker {
    display: none;
  }
  .tool {
    font: 11px/1.5 ui-monospace, 'SF Mono', Menlo, monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    padding: 0 6px;
    border-radius: 5px;
  }
  .tool.cur {
    color: var(--fg);
    box-shadow: inset 0 0 0 1.5px var(--wait);
  }

  /* Rendered markdown. */
  .md :global(p),
  .md :global(ul),
  .md :global(ol),
  .md :global(pre),
  .md :global(table),
  .md :global(blockquote),
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4) {
    margin: 0;
  }
  .md :global(* + *) {
    margin-top: 6px;
  }
  .md :global(li + li) {
    margin-top: 2px;
  }
  .md :global(ul),
  .md :global(ol) {
    padding-left: 18px;
  }
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4) {
    font-size: 13px;
  }
  .md :global(code) {
    font: 11.5px ui-monospace, 'SF Mono', Menlo, monospace;
    background: var(--panel-2);
    border-radius: 4px;
    padding: 0 4px;
  }
  .md :global(pre) {
    background: var(--panel-2);
    border-radius: 8px;
    padding: 8px 10px;
    overflow-x: auto;
  }
  .md :global(pre code) {
    background: none;
    padding: 0;
    font-size: 11.5px;
    line-height: 1.5;
  }
  .md :global(table) {
    border-collapse: collapse;
    font-size: 12px;
    display: block;
    overflow-x: auto;
  }
  .md :global(th),
  .md :global(td) {
    border: 1px solid var(--border);
    padding: 3px 8px;
    text-align: left;
  }
  .md :global(blockquote) {
    border-left: 3px solid var(--border);
    padding-left: 10px;
    color: var(--dim);
  }
  .md :global(hr) {
    border: 0;
    border-top: 1px solid var(--border);
  }
  .md :global(.md-link) {
    text-decoration: underline;
    text-decoration-color: var(--faint);
  }

  .ghost {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 5px 12px;
    color: var(--dim);
    font-size: 12px;
  }
  .ghost:hover {
    color: var(--fg);
  }
  .ghost.done {
    color: var(--run);
    border-color: color-mix(in srgb, var(--run) 45%, transparent);
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    font-weight: 600;
    font-size: 12px;
    border-radius: 8px;
    padding: 6px 14px;
    max-width: 50%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .primary:hover {
    filter: brightness(1.08);
  }

  /* A phone: the same dialog, fitted to about 390px. */
  @media (max-width: 600px) {
    .mask {
      padding-top: 6vh;
    }
    .pop {
      width: calc(100vw - 16px);
      max-height: calc(100vh - 12vh);
    }
    .pop.tall {
      height: calc(100vh - 12vh);
    }
    .bar {
      padding: 10px 12px;
      gap: 8px;
    }
    .seg button {
      max-width: 96px;
      padding: 3px 7px;
    }
    .hint {
      padding-left: 35px;
    }
    .sheet {
      margin: 0 4px;
    }
    .gsub {
      padding-left: 26px;
    }
    .hit {
      padding: 7px 6px 7px 18px;
    }
    /* Too narrow for the matched word to be sure of a place on one line. */
    .snip {
      white-space: normal;
      display: -webkit-box;
      -webkit-line-clamp: 2;
      line-clamp: 2;
      -webkit-box-orient: vertical;
    }
    .who-tag {
      width: 22px;
    }
    .rdh {
      flex-wrap: wrap;
      row-gap: 2px;
      padding: 0 12px 8px;
    }
    .rdh .name {
      flex: 1;
    }
    .rdh .sub {
      display: none;
    }
    .chat {
      padding: 10px 10px 24px;
    }
    .msg {
      max-width: 92%;
    }
    .foot {
      padding: 8px 12px;
    }
    .primary {
      max-width: 60%;
    }
  }
</style>
