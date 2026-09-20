<script lang="ts">
  // A right-click menu, pinned to the cursor. Deliberately tiny: the caller
  // owns "is it open and where", this only draws the items and reports a
  // choice. Closes on pick, on click-away, on Escape, and on scroll — a menu
  // left floating over content that moved under it is worse than no menu.

  export interface MenuItem {
    label: string;
    onselect: () => void;
    /** A destructive item (Close) is tinted. */
    danger?: boolean;
  }

  let {
    x,
    y,
    items,
    onclose,
  }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();

  let menu: HTMLDivElement;

  // Keep the menu on screen: if the cursor was near the right/bottom edge,
  // flip it back inside once we know its measured size. Starts at the cursor
  // and is corrected after the first measure; the effect tracks x/y so a menu
  // reused at a new position still repositions.
  let left = $state(0);
  let top = $state(0);
  // Hidden until measured, so it never flashes at the top-left before the
  // effect places it.
  let positioned = $state(false);
  $effect(() => {
    if (!menu) return;
    const r = menu.getBoundingClientRect();
    const pad = 6;
    left = Math.min(x, window.innerWidth - r.width - pad);
    top = Math.min(y, window.innerHeight - r.height - pad);
    positioned = true;
    menu.focus();
  });

  function pick(item: MenuItem) {
    item.onselect();
    onclose();
  }
</script>

<svelte:window
  onkeydown={(e) => e.key === 'Escape' && onclose()}
  onresize={onclose}
/>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onclick={onclose} oncontextmenu={(e) => { e.preventDefault(); onclose(); }} onwheel={onclose}></div>

<div
  class="menu"
  class:positioned
  bind:this={menu}
  tabindex="-1"
  style="left:{left}px; top:{top}px"
  role="menu"
>
  {#each items as item (item.label)}
    <button class="mi" class:danger={item.danger} role="menuitem" onclick={() => pick(item)}>
      {item.label}
    </button>
  {/each}
</div>

<style>
  /* Catches the click-away without dimming the page — a context menu should
     feel light, not modal. */
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .menu {
    position: fixed;
    z-index: 61;
    min-width: 148px;
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 9px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    outline: none;
    visibility: hidden;
  }
  .menu.positioned {
    visibility: visible;
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--fg);
    text-align: left;
  }
  .mi:hover {
    background: var(--panel-2);
  }
  .mi.danger {
    color: var(--err);
  }
  .mi.danger:hover {
    background: #2a1315;
  }
</style>
