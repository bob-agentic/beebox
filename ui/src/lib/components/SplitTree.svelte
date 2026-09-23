<script lang="ts">
  // Renders a layout node. Recursive, matching the protocol: the UI caps
  // nesting at two levels but the shape does not, so lifting the cap later is
  // a server-side constant.
  import { store } from '../state.svelte';
  import { leaves, type Node, type TabId } from '../proto';
  import Pane from './Pane.svelte';
  import SplitTree from './SplitTree.svelte';

  let {
    node,
    tab,
    path = [],
  }: { node: Node; tab: TabId; path?: number[] } = $props();

  /** Drag a gutter: resolve to fractions, then let the server normalise. */
  function startDrag(e: PointerEvent, index: number) {
    if (!store.caps.host || node.kind !== 'split') return;
    e.preventDefault();

    const container = (e.currentTarget as HTMLElement).parentElement!;
    const vertical = node.dir === 'vertical';
    const rect = container.getBoundingClientRect();
    const total = vertical ? rect.width : rect.height;
    const sizes = [...node.sizes];
    const start = vertical ? e.clientX : e.clientY;
    const a = sizes[index];
    const b = sizes[index + 1];

    const move = (ev: PointerEvent) => {
      const delta = ((vertical ? ev.clientX : ev.clientY) - start) / total;
      // Keep both neighbours visible; a zero-width pane is never wanted.
      const min = 0.08;
      const shift = Math.max(-a + min, Math.min(b - min, delta));
      sizes[index] = a + shift;
      sizes[index + 1] = b - shift;
      if (node.kind === 'split') node.sizes = [...sizes];
    };

    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      store.send({ t: 'set_sizes', tab, path, sizes });
    };

    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  const pane = $derived(node.kind === 'leaf' ? store.pane(node.pane) : undefined);

  /** Unique per child. A leaf is identified by its pane; a nested split by its
      position plus the panes it holds, so two sibling splits can never collide
      — a duplicate key silently breaks rendering. */
  function childKey(child: Node, i: number): string {
    return child.kind === 'leaf' ? `l:${child.pane}` : `s:${i}:${leaves(child).join('.')}`;
  }
</script>

{#if node.kind === 'leaf'}
  {#if pane}
    <Pane {pane} />
  {/if}
{:else}
  <div class="split" class:h={node.dir === 'horizontal'}>
    {#each node.children as child, i (childKey(child, i))}
      {#if i > 0}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="gutter"
          class:disabled={!store.caps.host}
          onpointerdown={(e) => startDrag(e, i - 1)}
        ></div>
      {/if}
      <div class="cell" style="flex: {node.sizes[i] ?? 1 / node.children.length}">
        <SplitTree node={child} {tab} path={[...path, i]} />
      </div>
    {/each}
  </div>
{/if}

<style>
  .split {
    display: flex;
    flex-direction: row;
    gap: 6px;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .split.h {
    flex-direction: column;
  }
  .cell {
    display: flex;
    min-width: 0;
    min-height: 0;
  }
  .gutter {
    flex: 0 0 3px;
    background: transparent;
    cursor: col-resize;
    border-radius: 2px;
  }
  .gutter:hover {
    background: var(--border);
  }
  .split.h > .gutter {
    cursor: row-resize;
  }
  .gutter.disabled {
    cursor: default;
    pointer-events: none;
  }

  @media (max-width: 640px) {
    .split, .split.h { flex-direction: column; }
    .gutter, .split.h > .gutter {
      cursor: default;
      pointer-events: none;
    }
  }
</style>
