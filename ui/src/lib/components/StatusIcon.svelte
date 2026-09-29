<script lang="ts">
  // One agent-status dot. Used at pane, tab, and workspace level; the caller
  // passes an already-rolled-up phase (and unread flag) for aggregates.
  //
  // "never_ran" renders nothing at all — a column of ghost dots for plain
  // shells would just be noise.
  //
  // The tooltip is a custom layer, not the native `title` attribute: native
  // tooltips appear after a platform-defined delay and collapse multi-line
  // content inconsistently across browsers, and this dot is the product's
  // main surface — it deserves mux0-quality hover feedback.

  import type { AgentPhase, AgentStatusView } from '../proto';
  import { tooltip } from '../agent-status';

  let {
    phase,
    view = null,
    unread = false,
  }: {
    phase: AgentPhase;
    /** Full view when this dot describes a single pane; enables the tooltip. */
    view?: AgentStatusView | null;
    unread?: boolean;
  } = $props();

  let hover = $state(false);
  // Recomputed on each hover so "Running for 18s" is current when shown.
  let lines = $state<string[]>([]);
  function enter() {
    if (!view) return;
    lines = tooltip(view, Date.now());
    hover = lines.length > 0;
  }
</script>

{#if phase !== 'never_ran'}
  <span
    class="ast-wrap"
    role="status"
    onmouseenter={enter}
    onmouseleave={() => (hover = false)}
  >
    <span
      class="ast {phase}"
      class:read={!unread && (phase === 'success' || phase === 'failed')}
    ></span>
    {#if hover}
      <span class="tip">
        {#each lines as line, i (i)}
          <span class="tip-line" class:head={i === 0}>{line}</span>
        {/each}
      </span>
    {/if}
  </span>
{/if}

<style>
  .ast-wrap {
    position: relative;
    display: inline-flex;
    flex: 0 0 auto;
  }
  .ast {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex: 0 0 auto;
  }
  /* A turning arc, not a pulse: a pulsing green dot was hard to tell from a
     steady one (a finished run) at this size, and several pulsing at once
     tire the eye. Shape tells them apart, and an even turn does not pull at
     the eye the way fading in and out does. Three pixels larger than the other
     dots to hold a ring, with the margin giving it back so rows do not move. */
  .ast.running {
    width: 12px;
    height: 12px;
    margin: -1.5px;
    border: 2.5px solid color-mix(in srgb, var(--run) 30%, transparent);
    border-top-color: var(--run);
    animation: spin 1.1s linear infinite;
    will-change: transform;
  }
  /* Each animated dot gets its own compositor layer (`will-change`): the
     sidebar and tab strip repaint around them constantly, and a dot that
     shares those repaints stutters. */
  .ast.needs_input {
    background: var(--wait);
    animation: blink 1.2s ease-in-out infinite;
    will-change: opacity;
  }
  /* Respect a system-level request for less motion: colour and shape still
     carry the state, so the animation is the only thing lost. */
  @media (prefers-reduced-motion: reduce) {
    .ast.running,
    .ast.needs_input {
      animation: none;
    }
  }
  .ast.idle {
    background: transparent;
    border: 1.5px solid var(--idle);
  }
  .ast.success {
    background: var(--run);
  }
  .ast.failed {
    background: var(--err);
  }
  /* A read completion keeps its colour but hollows out, so "there is a fresh
     result here" and "you already saw this" stay distinguishable. */
  .ast.success.read {
    background: transparent;
    border: 1.5px solid var(--run);
  }
  .ast.failed.read {
    background: transparent;
    border: 1.5px solid var(--err);
  }

  .tip {
    position: absolute;
    top: calc(100% + 7px);
    left: -4px;
    z-index: 40;
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 140px;
    max-width: 300px;
    padding: 7px 10px;
    border-radius: 7px;
    background: var(--panel-2, #1c1c24);
    border: 1px solid var(--border);
    box-shadow: 0 6px 22px rgba(0, 0, 0, 0.45);
    font-size: 10.5px;
    line-height: 1.45;
    color: var(--dim, #b9b9c6);
    white-space: normal;
    pointer-events: none;
  }
  .tip-line.head {
    color: var(--fg, #e8e8ef);
    font-weight: 600;
  }
</style>
