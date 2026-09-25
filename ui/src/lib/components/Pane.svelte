<script lang="ts">
  // A pane's view. The Terminal itself belongs to the store (see
  // pane-term.ts): this component comes and goes whenever the layout around
  // the pane changes, and the history must not go with it. What is kept here
  // is only what belongs to this place on screen — measuring it, and watching
  // it change size.

  import { onMount } from 'svelte';
  import { store } from '../state.svelte';
  import { settings } from '../settings.svelte';
  import type { PaneView } from '../proto';
  import type { PaneTerm } from '../pane-term';
  import { agentBadge, isUnread } from '../agent-status';
  import Crumbs from './Crumbs.svelte';
  import Icon from './Icon.svelte';
  import StatusIcon from './StatusIcon.svelte';

  let { pane }: { pane: PaneView } = $props();

  let host: HTMLDivElement;
  let reportSize: (() => void) | null = null;
  let pt: PaneTerm | null = null;

  /** On the tab in front. Every tab stays mounted, so a window resize or a
      sidebar fold reached every terminal on the machine at once — each one
      re-laying out its scrollback and making its shell repaint. A pane out
      of sight only notes that it owes a fit, and settles it when shown. */
  const shown = $derived(store.visiblePanes().some((p) => p.id === pane.id));
  let owesFit = false;
  $effect(() => {
    if (shown && owesFit) reportSize?.();
  });

  $effect(() => {
    // Read before the call: `pt?.show(shown)` skips its argument while the
    // terminal is not attached yet, and then the effect never tracks `shown`.
    const on = shown;
    pt?.show(on);
  });

  // The PTY's width, as the server last said. Someone else resizing it has to
  // reach a client that draws at that width.
  $effect(() => {
    void pane.cols;
    reportSize?.();
  });
  const focused = $derived(store.focused === pane.id);

  /** Same reasoning as the tab bar: "bob@Bobs-MacBook-Pro:~/x" is noise. */
  const paneTitle = $derived.by(() => {
    if (pane.pty === null) return 'exited';
    const t = pane.title ?? '';
    if (t && !/^[\w.-]+@[\w.-]+[:\s]/.test(t)) return t;
    return pane.cwd.split('/').filter(Boolean).pop() || 'shell';
  });

  const AGENT_STYLE: Record<string, string> = {
    OC: 'background:#1f2f3d;color:#7dd3fc',
    SH: 'background:#2b2b35;color:#8b8b9a',
  };

  // Claude and Codex get their own marks — recognisable at a glance in a way
  // that "CC" and "CX" never were, and in their own colours, since a mark
  // recoloured to match the chrome stops being the thing people recognise.
  // OpenAI's blossom is monochrome by design (black on light, white on dark),
  // so it follows the theme's foreground; a fixed value would disappear
  // against half of the 600-odd themes. Anything else keeps its badge.
  const AGENT_MARK: Record<string, { icon: 'claude' | 'codex'; color: string }> = {
    CC: { icon: 'claude', color: '#D97757' },
    CX: { icon: 'codex', color: 'var(--fg)' },
  };

  const badge = $derived(agentBadge(pane.agent));
  const mark = $derived(AGENT_MARK[badge]);
  // Reading is a local act: focusing the pane marks the completion seen in
  // this browser only. localStorage is not reactive, so the flag is mirrored
  // into local state and re-derived whenever the status or focus changes.
  let unread = $state(false);
  $effect(() => {
    const done = pane.status.phase === 'success' || pane.status.phase === 'failed';
    // Through the store so tab/workspace aggregate dots re-derive too.
    if (focused && done) store.markStatusRead(pane.id, pane.status);
    void store.readRev;
    unread = isUnread(pane.id, pane.status);
  });

  onMount(() => {
    const t = store.terminal(pane.id);
    const term = t.term;
    pt = t;

    // Lets the end-to-end tests read what the terminal is actually showing,
    // and the shell's self-test drive this pane without depending on focus.
    (host as any).__serialize = () => t.serialize();
    (host as any).__type = (text: string) => term.input(text);

    // Only report when the grid actually changed. A ResizeObserver fires for
    // any pixel movement, and every redundant report makes the shell repaint
    // its prompt — which is what left rows of stray `%` markers on screen.
    // Per mount, so a pane that moved reports its new place at least once.
    let lastCols = 0;
    let lastRows = 0;
    // `force` is for the re-fit button. The usual caller is a ResizeObserver
    // firing constantly, so an unchanged size is not worth a message — but a
    // session carried to another device may measure the same here while the
    // server holds a size some other client set, and then silence is wrong.
    // Asking explicitly has to mean asking.
    const report = (force = false) => {
      // Always the first time: the replay needs a real width to land in.
      if (!shown && !force && lastCols !== 0) {
        owesFit = true;
        return;
      }
      owesFit = false;
      let dims;
      try {
        dims = t.fit.proposeDimensions();
      } catch {
        return;
      }
      if (!dims || !(dims.cols > 0) || !(dims.rows > 0)) return;
      // Every client draws at the width the PTY really has; its own is only a
      // request, and another window's may win. Fitted to its own window
      // instead, output laid out for the other width lands wrong — lines wrap,
      // and a TUI that redraws by moving up N rows erases the wrong ones and
      // leaves its old frame behind as a duplicate. A narrower window loses
      // the right edge; nothing is mis-drawn. The server's answer arrives as
      // `pane.cols`, which runs this again.
      const cols = pane.cols || dims.cols;
      if (term.cols !== cols || term.rows !== dims.rows) term.resize(cols, dims.rows);
      t.refresh();
      if (!force && dims.cols === lastCols && dims.rows === lastRows) return;
      lastCols = dims.cols;
      lastRows = dims.rows;
      // Advisory: the server decides, and the owner's viewport wins.
      store.send({ t: 'viewport', pane: pane.id, cols: dims.cols, rows: dims.rows });
    };
    reportSize = report;

    store.attach(pane.id, host, report);
    t.show(shown);
    report();

    // Not while the sidebar is moving: a terminal re-laid out on every frame
    // of the slide is what made it stutter. It fits once, when it lands.
    const ro = new ResizeObserver(() => {
      if (!store.holdFit) report();
    });
    ro.observe(host);

    // Only this mount's part. The terminal stays with the store until the
    // pane leaves the tree.
    return () => {
      ro.disconnect();
      reportSize = null;
      t.detach(host);
    };
  });
</script>

<!-- Background comes from the theme, not a constant: on a light theme a
     hardcoded dark panel shows up as a black border round the terminal. -->
<div
  class="pane"
  class:focus={focused}
  role="group"
  style="background:{settings.theme.background}"
  aria-label="Terminal {pane.title || badge}"
  onpointerdown={() => store.focus(pane.id)}
>
  <div class="pane-head">
    <StatusIcon phase={pane.status.phase} view={pane.status} {unread} />
    {#if mark}
      <span class="mark" style="color:{mark.color}" title={badge}>
        <Icon name={mark.icon} size={13} />
      </span>
    {:else}
      <span class="agent" style={AGENT_STYLE[badge] ?? AGENT_STYLE.SH}>{badge}</span>
    {/if}
    <span class="ttl">{paneTitle}</span>
    {#if pane.pty === null}
      <button class="respawn" onclick={() => store.send({ t: 'respawn', pane: pane.id })}>
        Restart
      </button>
    {/if}
    <span class="dims">{pane.cols}×{pane.rows}</span>
  </div>

  <div class="term" bind:this={host}></div>

  <!-- Path belongs to the pane, not the workspace: two panes in one tab can
       sit in different directories. -->
  <div class="pane-foot">
    <Crumbs path={pane.cwd} />
    {#if pane.git}
      <span class="git">
        ⎇ {pane.git.branch}
        {#if pane.git.added || pane.git.modified}
          <span class="dirty">+{pane.git.added} ~{pane.git.modified}</span>
        {/if}
      </span>
    {/if}
  </div>
</div>

<style>
  .pane {
    flex: 1;
    min-width: 0;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
  }
  /* The same accent that marks the selected tab and workspace, so the three
     read as one selection rather than three unrelated highlights. Mixed with
     the panel colour instead of a fixed green: the accent comes from the
     theme, and a hardcoded one clashed with every palette but the default. */
  .pane.focus {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .pane-head {
    height: 22px;
    flex: 0 0 22px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    font-size: 10.5px;
    color: var(--faint);
  }
  .agent {
    font-size: 8.5px;
    padding: 0 4px;
    border-radius: 3px;
    font-weight: 700;
  }
  /* No pill behind a brand mark: the shape is the identifier, and a coloured
     box around it only competes with the status dot beside it. */
  .mark {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
  }
  .ttl {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dims {
    font-family: ui-monospace, monospace;
  }
  .respawn {
    font-size: 9.5px;
    padding: 1px 7px;
    border-radius: 5px;
    background: var(--panel-2);
    color: var(--dim);
    border: 1px solid var(--border);
  }
  .respawn:hover {
    color: var(--fg);
  }

  .term {
    flex: 1;
    min-height: 0;
    padding: 7px 9px;
    overflow: hidden;
  }

  /* xterm paints its own background; the padding around it must match or a
     rim of the wrong colour shows through. */
  .term :global(.xterm-viewport) {
    background: transparent !important;
    /* Momentum, so a flick keeps going the way it does in a browser. Without
       this the viewport stops dead with the finger, which on a phone reads as
       the terminal being slow rather than as a scrolling model. */
    -webkit-overflow-scrolling: touch;
    overscroll-behavior: contain;
  }

  .pane-foot {
    height: 21px;
    flex: 0 0 21px;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 0 8px;
    background: var(--panel);
    border-top: 1px solid var(--border);
    font-size: 10px;
    color: var(--faint);
    overflow: hidden;
  }
  .git {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 0 0 auto;
    font-family: ui-monospace, monospace;
  }
  .dirty {
    color: var(--wait);
  }
</style>
