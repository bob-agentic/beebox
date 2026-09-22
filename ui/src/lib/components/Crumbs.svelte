<script lang="ts">
  // Full-path breadcrumbs. Home collapses to ~, everything else is shown —
  // eliding the middle hid exactly the segments that tell deep project
  // directories apart. CSS overflow keeps the leaf visible when space runs
  // out, and the title carries the whole path either way.
  import Icon from './Icon.svelte';
  import { writeClipboard } from '../clipboard';

  let { path }: { path: string } = $props();

  const parts = $derived.by(() => {
    const home = path.replace(/^\/Users\/[^/]+/, '~');
    const segs = home.split('/').filter(Boolean);
    if (home.startsWith('~')) segs[0] = '~';
    else segs.unshift('/');
    return segs;
  });

  // The button answers, or the user assumes nothing happened. Same dwell as
  // the share dialog's Copy, so the two feel like one gesture.
  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function copy() {
    // `path`, not the rendered segments: what is on screen has $HOME folded to
    // `~`, which is not a path anything else can open.
    if (!(await writeClipboard(path))) return;
    copied = true;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => (copied = false), 1600);
  }

  // Closing a pane unmounts this mid-dwell; the pending timer would then write
  // to state that no longer has a component.
  $effect(() => () => {
    if (timer) clearTimeout(timer);
  });
</script>

<div class="crumbs-row">
  <div class="crumbs" title={path}>
    {#each parts as seg, i}
      {#if i > 0}<span class="sep">›</span>{/if}
      <span class:leaf={i === parts.length - 1}>{seg}</span>
    {/each}
  </div>
  <button
    class="copy"
    class:did={copied}
    title={copied ? 'Copied' : 'Copy path'}
    aria-label={copied ? 'Path copied' : 'Copy path'}
    onclick={copy}
  >
    <Icon name={copied ? 'check' : 'copy'} size={11} />
  </button>
</div>

<style>
  /* The row spans the footer, but the path only takes the width it needs —
     `flex: 1` here would stretch it and strand the button at the far right,
     far from the text it acts on. The button sits right after the last
     segment instead, and `justify-content` keeps the pair left-aligned. */
  .crumbs-row {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 5px;
  }
  .crumbs {
    flex: 0 1 auto;
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
  /* Quiet until reached for, like the close affordances elsewhere. */
  .copy {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    color: var(--faint);
    opacity: 0;
    padding: 0 1px;
  }
  .crumbs-row:hover .copy {
    opacity: 1;
  }
  .copy:hover {
    color: var(--fg);
  }
  /* Confirmation outranks hover: it must be visible after the pointer leaves. */
  .copy.did {
    opacity: 1;
    color: var(--accent);
  }
</style>
