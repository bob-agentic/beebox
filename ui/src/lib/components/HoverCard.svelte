<script lang="ts">
  // Draws the one hover card (see hovercard.svelte.ts). Fixed to the window,
  // not nested in the row, so the sidebar's and tab strip's overflow cannot
  // clip it.

  import { card, hideCard } from '../hovercard.svelte';

  let el = $state<HTMLDivElement>();
  let pos = $state<{ left: number; top: number } | null>(null);

  // Measured after it renders: where it fits depends on how big it came out.
  $effect(() => {
    const open = card.open;
    pos = null;
    if (!open || !el) return;
    const { width: w, height: h } = el.getBoundingClientRect();
    const a = open.anchor;
    const vw = innerWidth;
    const vh = innerHeight;
    let left: number;
    let top: number;
    if (open.side === 'right') {
      left = a.right + 8;
      top = a.top - 4;
    } else {
      left = a.left;
      top = a.bottom + 6;
      if (top + h > vh - 8) top = a.top - h - 6;
    }
    left = Math.max(8, Math.min(left, vw - w - 8));
    top = Math.max(8, Math.min(top, vh - h - 8));
    pos = { left, top };
  });

  // A card opened by a hold stays until the next touch anywhere; one opened
  // by a mouse also goes when its row scrolls out from under it. Only its
  // row's scroller: a terminal scrolls with every line an agent prints.
  function scrolled(e: Event) {
    const owner = card.open?.owner;
    if (owner && e.target instanceof Node && e.target.contains(owner)) hideCard();
  }

  $effect(() => {
    if (!card.open) return;
    const opts = { capture: true, passive: true };
    addEventListener('pointerdown', hideCard, opts);
    addEventListener('wheel', scrolled, opts);
    addEventListener('scroll', scrolled, opts);
    addEventListener('keydown', hideCard, opts);
    addEventListener('blur', hideCard);
    return () => {
      removeEventListener('pointerdown', hideCard, opts);
      removeEventListener('wheel', scrolled, opts);
      removeEventListener('scroll', scrolled, opts);
      removeEventListener('keydown', hideCard, opts);
      removeEventListener('blur', hideCard);
    };
  });

  /** Paths break at their slashes, not mid-name. Not a lookbehind split:
      older WebKit cannot parse one, and that would take the whole bundle down. */
  const parts = (v: string) => v.split('/').map((p, i, all) => (i < all.length - 1 ? p + '/' : p));
</script>

{#if card.open}
  <div
    class="card"
    class:on={pos}
    bind:this={el}
    style:left="{pos?.left ?? 0}px"
    style:top="{pos?.top ?? 0}px"
    role="tooltip"
  >
    <div class="title">{card.open.content.title}</div>
    {#if card.open.content.rows.length}
      <div class="rows">
        {#each card.open.content.rows as r (r.k)}
          <span class="k">{r.k}</span>
          <span class="v" class:mono={r.mono}
            >{#each parts(r.v) as p, i (i)}{p}<wbr />{/each}</span
          >
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .card {
    position: fixed;
    z-index: 70;
    max-width: min(340px, calc(100vw - 16px));
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--panel);
    border: 1px solid var(--border);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.3);
    pointer-events: none;
    visibility: hidden;
    opacity: 0;
    transform: translateY(-2px);
  }
  .card.on {
    visibility: visible;
    opacity: 1;
    transform: none;
    transition:
      opacity 0.12s cubic-bezier(0.33, 1, 0.68, 1),
      transform 0.12s cubic-bezier(0.33, 1, 0.68, 1);
  }
  .title {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--fg);
    overflow-wrap: anywhere;
  }
  .rows {
    margin-top: 3px;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 8px;
    font-size: 11px;
    line-height: 1.45;
  }
  .k {
    color: var(--faint);
  }
  .v {
    color: var(--dim);
    overflow-wrap: anywhere;
  }
  .v.mono {
    font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  }
</style>
