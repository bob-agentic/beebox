<script lang="ts">
  // A pane owns its Terminal. Earlier this lived in the store, keyed by pane
  // id, so that a re-render could reuse it — but an xterm Terminal is bound to
  // the element it was opened on, and a split re-parents panes, which left a
  // terminal drawing into a node that was no longer on screen. Ownership here
  // makes that impossible: the element and the Terminal are created and
  // destroyed together.
  //
  // Nothing is lost by it. Scrollback comes back from the server's ring via
  // `Resync`, which exists for exactly this.

  import { onMount } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { Unicode11Addon } from '@xterm/addon-unicode11';
  import { store } from '../state.svelte';
  import { settings } from '../settings.svelte';
  import type { PaneView } from '../proto';
  import { agentBadge, isUnread } from '../agent-status';
  import Crumbs from './Crumbs.svelte';
  import Icon from './Icon.svelte';
  import StatusIcon from './StatusIcon.svelte';

  let { pane }: { pane: PaneView } = $props();

  let host: HTMLDivElement;
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
    const cfg = settings.current;
    const term = new Terminal({
      fontFamily: cfg.fontFamily,
      fontSize: cfg.fontSize,
      lineHeight: cfg.lineHeight,
      cursorBlink: cfg.cursorBlink,
      allowProposedApi: true,
      // Matched to the server ring, so scrolling back never hits a hole the
      // server could have filled.
      scrollback: 200_000,
      theme: settings.xterm,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);

    // Lets the end-to-end tests read what the terminal is actually showing.
    // WebGL draws to a canvas, so there is nothing in the DOM to assert on.
    void import('@xterm/addon-serialize').then(({ SerializeAddon }) => {
      const ser = new SerializeAddon();
      term.loadAddon(ser);
      (host as any).__serialize = () => ser.serialize();
    });
    // Lets the shell's self-test drive this pane without depending on focus.
    (host as any).__type = (text: string) => term.input(text);
    term.loadAddon(new Unicode11Addon());
    // Correct widths for CJK and emoji — verified on a real device.
    term.unicode.activeVersion = '11';

    term.open(host);

    // WebKit drops the first Chinese full-width punctuation mark: `？` needs
    // two presses, while Han characters are fine.
    //
    // xterm only accepts an `insertText` when `_keyDownSeen` is false, assuming
    // the `input` for a keystroke arrives after its `keydown`. WebKit reverses
    // that pair for IME direct-commit, and these marks need Shift — whose own
    // keydown set the flag and whose keyup has not run yet — so the character
    // is discarded. Han characters go through compositionend instead, which
    // never consults the flag. Upstream: xtermjs/xterm.js#6144, still open.
    //
    // Clearing the flag before xterm's own capture-phase listener runs lets the
    // character through its normal path. Nothing extra is sent, and the
    // duplicate-suppression xterm already does is untouched — so this cannot
    // double up. Composition is left strictly alone.
    if ('__BEEBOX__' in window) {
      host.addEventListener(
        'beforeinput',
        (ev: Event) => {
          const e = ev as InputEvent;
          if (e.inputType !== 'insertText' || !e.data) return;
          const core = (term as any)._core;
          if (core && !core._compositionHelper?.isComposing) {
            core._keyDownSeen = false;
          }
        },
        true,
      );
    }

    // WKWebView can parse output into the buffer without invalidating xterm's
    // compositing layer. Coalesce an explicit refresh to the next frame so a
    // busy agent still causes at most one extra paint per display frame.
    let refreshFrame = 0;
    const refresh = () => {
      if (refreshFrame) return;
      refreshFrame = requestAnimationFrame(() => {
        refreshFrame = 0;
        if (term.rows > 0) term.refresh(0, term.rows - 1);
      });
    };
    const writeParsed = term.onWriteParsed(refresh);

    // WebKit currently accepts the WebGL context but composites it as a blank
    // layer. Browsers keep the accelerated renderer; the native shell uses
    // xterm's Canvas2D addon, which WebKit snapshots and composites reliably.
    if ('__BEEBOX__' in window) {
      void import('@xterm/addon-canvas').then(({ CanvasAddon }) => {
        try {
          term.loadAddon(new CanvasAddon());
          refresh();
        } catch {
          // The built-in DOM renderer remains active.
        }
      });
    } else {
      void import('@xterm/addon-webgl').then(({ WebglAddon }) => {
        try {
          const gl = new WebglAddon();
          gl.onContextLoss(() => gl.dispose());
          term.loadAddon(gl);
          refresh();
        } catch {
          // The built-in renderer remains active.
        }
      });
    }

    const send = (data: Uint8Array) => store.send({ t: 'input', pane: pane.id, data });
    term.onData((s) => send(new TextEncoder().encode(s)));
    term.onBinary((s) => {
      const bytes = new Uint8Array(s.length);
      for (let i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i) & 0xff;
      send(bytes);
    });

    // Only report when the grid actually changed. A ResizeObserver fires for
    // any pixel movement, and every redundant report makes the shell repaint
    // its prompt — which is what left rows of stray `%` markers on screen.
    let lastCols = 0;
    let lastRows = 0;
    const report = () => {
      try {
        fit.fit();
      } catch {
        return;
      }
      refresh();
      if (term.cols === lastCols && term.rows === lastRows) return;
      lastCols = term.cols;
      lastRows = term.rows;
      // Advisory: the server decides, and the owner's viewport wins.
      store.send({ t: 'viewport', pane: pane.id, cols: term.cols, rows: term.rows });
    };

    store.register(pane.id, term, report);
    report();

    const ro = new ResizeObserver(report);
    ro.observe(host);

    // Appearance changes apply to terminals that already exist, so you can see
    // a font or theme land without restarting anything.
    const stopWatching = settings.onChange(() => {
      const c = settings.current;
      term.options.fontFamily = c.fontFamily;
      term.options.fontSize = c.fontSize;
      term.options.lineHeight = c.lineHeight;
      term.options.cursorBlink = c.cursorBlink;
      term.options.theme = settings.xterm;
      // Glyph size changed, so the column count did too.
      report();
    });

    return () => {
      stopWatching();
      writeParsed.dispose();
      if (refreshFrame) cancelAnimationFrame(refreshFrame);
      ro.disconnect();
      store.unregister(pane.id);
      term.dispose();
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
