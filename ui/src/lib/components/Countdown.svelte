<script lang="ts">
  // The time a passing note has left, as a line that drains. A CSS animation
  // on `transform`, so the compositor draws it and nothing runs per frame;
  // `held` pauses it with the note's own timer (see countdown.ts). Remount it
  // (`{#key}`) to start it again.
  let { ms, held = false }: { ms: number; held?: boolean } = $props();
</script>

<div class="track"><div class="left" class:held style:animation-duration="{ms}ms"></div></div>

<style>
  .track {
    height: 2px;
    border-radius: 1px;
    background: var(--border);
    overflow: hidden;
  }
  .left {
    height: 100%;
    background: var(--accent);
    transform-origin: left;
    animation: drain linear forwards;
  }
  .left.held {
    animation-play-state: paused;
  }
  @keyframes drain {
    from {
      transform: scaleX(1);
    }
    to {
      transform: scaleX(0);
    }
  }
</style>
