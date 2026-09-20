<script lang="ts">
  // Full-path breadcrumbs. Home collapses to ~, everything else is shown —
  // eliding the middle hid exactly the segments that tell deep project
  // directories apart. CSS overflow keeps the leaf visible when space runs
  // out, and the title carries the whole path either way.
  let { path }: { path: string } = $props();

  const parts = $derived.by(() => {
    const home = path.replace(/^\/Users\/[^/]+/, '~');
    const segs = home.split('/').filter(Boolean);
    if (home.startsWith('~')) segs[0] = '~';
    else segs.unshift('/');
    return segs;
  });
</script>

<div class="crumbs" title={path}>
  {#each parts as seg, i}
    {#if i > 0}<span class="sep">›</span>{/if}
    <span class:leaf={i === parts.length - 1}>{seg}</span>
  {/each}
</div>

<style>
  .crumbs {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    display: flex;
    align-items: center;
    gap: 3px;
    white-space: nowrap;
    font-family: ui-monospace, monospace;
    color: var(--dim);
  }
  .sep {
    color: var(--faint);
    font-size: 8.5px;
  }
  .leaf {
    color: var(--fg);
    font-weight: 500;
  }
</style>
