<script lang="ts">
  // The image behind an agent's `[Image #N]`, over everything. The browser
  // decodes it and the compositor moves it: zoom and pan are a transform on
  // the <img>, never a re-layout.
  //
  // Over everything below the app's title bar, which stays as it was. Two
  // small pills float at fixed places — above, the image's tag and close;
  // below, what was asked with it, which (or L) closes the viewer on that
  // message in the terminal — and the rest is the half-dimmed terminal.
  // Nothing moves as you step.
  //
  // Mouse: wheel zooms at the pointer, drag pans, double-click toggles 1×/2.5×,
  // ←/→ step, Esc or a click outside the image closes. Touch: pinch, drag when
  // zoomed, double-tap, swipe sideways to step, swipe down to close.
  import { fade } from 'svelte/transition';
  import { store } from '../state.svelte';

  const img = $derived(store.image);

  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  /** The swipe in progress at 1×, drawn as it goes. */
  let drag = $state({ x: 0, y: 0 });
  /** A finger or button is down: the image follows it with no easing. */
  let pressing = $state(false);
  let stage: HTMLDivElement | undefined = $state();
  /** Where the image is centred, unscaled: the zoom's origin. */
  let frame: HTMLDivElement | undefined = $state();
  /** Set for a moment when a step finds no image that way. */
  let edge = $state<-1 | 1 | null>(null);
  let edgeTimer = 0;

  async function step(dir: -1 | 1) {
    clearTimeout(edgeTimer);
    if (await store.stepImage(dir)) {
      edge = null;
      return;
    }
    edge = dir;
    edgeTimer = window.setTimeout(() => (edge = null), 1400);
  }

  // A new image starts unzoomed.
  $effect(() => {
    void img?.n;
    reset();
  });

  function reset() {
    scale = 1;
    tx = 0;
    ty = 0;
    drag = { x: 0, y: 0 };
  }

  /** Zooms to `to`, keeping the point under (x, y) where it is. */
  function zoomAt(to: number, x: number, y: number) {
    const next = Math.min(8, Math.max(1, to));
    if (!frame) return;
    const r = frame.getBoundingClientRect();
    const px = x - (r.left + r.width / 2);
    const py = y - (r.top + r.height / 2);
    const k = next / scale;
    tx = px - (px - tx) * k;
    ty = py - (py - ty) * k;
    scale = next;
    if (scale === 1) tx = ty = 0;
  }

  function onkey(e: KeyboardEvent) {
    // ⌘ shortcuts stay the app's.
    if (!img || e.metaKey) return;
    const act =
      e.key === 'Escape'
        ? () => store.closeImage()
        : e.key === 'ArrowLeft'
          ? () => step(-1)
          : e.key === 'ArrowRight'
            ? () => step(1)
            : e.key === 'l' || e.key === 'L'
              ? () => store.locateImage()
              : null;
    // Taken before the terminal, which has the keyboard and would type it.
    e.preventDefault();
    e.stopPropagation();
    act?.();
  }

  function onwheel(e: WheelEvent) {
    e.preventDefault();
    zoomAt(scale * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002)), e.clientX, e.clientY);
  }

  // Pointers down on the stage, for pinch and drag.
  const down = new Map<number, { x: number; y: number }>();
  let gesture: {
    x: number;
    y: number;
    tx: number;
    ty: number;
    dist: number;
    scale: number;
    moved: boolean;
    at: number;
  } | null = null;
  let lastTap = { at: 0, x: 0, y: 0 };

  function begin() {
    const pts = [...down.values()];
    const mid = {
      x: pts.reduce((a, p) => a + p.x, 0) / pts.length,
      y: pts.reduce((a, p) => a + p.y, 0) / pts.length,
    };
    const dist = pts.length > 1 ? Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y) : 0;
    gesture = { ...mid, tx, ty, dist, scale, moved: gesture?.moved ?? false, at: gesture?.at ?? performance.now() };
  }

  function onpointerdown(e: PointerEvent) {
    // The strip and the question are buttons, not part of the picture.
    if (e.button !== 0 || (e.target as Element).closest('button')) return;
    stage?.setPointerCapture(e.pointerId);
    down.set(e.pointerId, { x: e.clientX, y: e.clientY });
    pressing = true;
    if (down.size === 1) gesture = null;
    begin();
  }

  function onpointermove(e: PointerEvent) {
    if (!down.has(e.pointerId) || !gesture) return;
    down.set(e.pointerId, { x: e.clientX, y: e.clientY });
    const pts = [...down.values()];
    const mx = pts.reduce((a, p) => a + p.x, 0) / pts.length;
    const my = pts.reduce((a, p) => a + p.y, 0) / pts.length;
    const dx = mx - gesture.x;
    const dy = my - gesture.y;
    if (Math.hypot(dx, dy) > 8) gesture.moved = true;
    if (pts.length > 1 && gesture.dist > 0) {
      const d = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
      scale = gesture.scale;
      tx = gesture.tx + dx;
      ty = gesture.ty + dy;
      zoomAt(gesture.scale * (d / gesture.dist), mx, my);
    } else if (scale > 1) {
      tx = gesture.tx + dx;
      ty = gesture.ty + dy;
    } else if (e.pointerType !== 'mouse') {
      drag = { x: dx, y: dy };
    }
  }

  function onpointerup(e: PointerEvent) {
    if (!down.delete(e.pointerId) || !gesture) return;
    if (down.size) {
      // One finger of a pinch lifted: carry on from here with the other.
      begin();
      return;
    }
    const g = gesture;
    gesture = null;
    pressing = false;
    const { x: dx, y: dy } = drag;
    drag = { x: 0, y: 0 };
    if (g.moved) {
      if (scale === 1 && Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(dy)) void step(dx < 0 ? 1 : -1);
      else if (scale === 1 && dy > 90 && dy > Math.abs(dx)) store.closeImage();
      return;
    }
    // A tap. Two in quick succession zoom, by mouse or finger alike.
    const now = performance.now();
    if (now - lastTap.at < 300 && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 30) {
      lastTap.at = 0;
      if (scale > 1) reset();
      else zoomAt(2.5, e.clientX, e.clientY);
      return;
    }
    lastTap = { at: now, x: e.clientX, y: e.clientY };
    // A click beside the image closes, as lightboxes do. Not a finger's: too
    // easy to land there while reaching for the picture.
    if (e.pointerType === 'mouse' && !(e.target as Element).closest('img, .pill, button')) {
      const at = now;
      setTimeout(() => {
        if (lastTap.at === at) store.closeImage();
      }, 300);
    }
  }
</script>

<svelte:window onkeydowncapture={onkey} />

{#if img}
  <div
    class="viewer"
    role="dialog"
    tabindex="-1"
    aria-label="Image {img.n}"
    bind:this={stage}
    {onwheel}
    {onpointerdown}
    {onpointermove}
    {onpointerup}
    onpointercancel={onpointerup}
  >
    <!-- Where the image may be: clear of the pills and the side arrows. Its
         centre is the image's, and the zoom's origin. -->
    <div class="stage" bind:this={frame}>
      {#if img.url}
        <img
          src={img.url}
          alt="Image {img.n}"
          draggable="false"
          decoding="async"
          style:transform="translate({tx + drag.x}px, {ty + Math.max(0, drag.y)}px) scale({scale})"
          style:opacity={1 - Math.min(0.6, Math.max(0, drag.y) / 300)}
          class:moving={pressing}
        />
      {:else}
        <p class="note pill">{img.missing ? 'This image isn’t in the session’s transcript.' : 'Loading…'}</p>
      {/if}
    </div>

    <!-- Desktop: beside everything, where they stay put. Phone: in the pill. -->
    <button class="side prev" aria-label="Previous image" disabled={img.n <= 1} onclick={() => step(-1)}>‹</button>
    <button class="side next" aria-label="Next image" onclick={() => step(1)}>›</button>

    <div class="top pill">
      <button class="nav" aria-label="Previous image" disabled={img.n <= 1} onclick={() => step(-1)}>‹</button>
      <span class="tag">[Image #{img.n}]</span>
      <button class="nav" aria-label="Next image" onclick={() => step(1)}>›</button>
      <button class="close" aria-label="Close" onclick={() => store.closeImage()}>✕</button>
    </div>
    <!-- Under the pill, not in it: the pill keeps its width, ✕ its place. -->
    {#if edge}
      <span class="edge pill" transition:fade={{ duration: 120 }}>{edge < 0 ? 'First image' : 'Last image'}</span>
    {/if}

    <!-- Always there, at one size, whether or not the message still is. -->
    {#if img.asked}
      <button class="bottom pill asked" onclick={() => store.locateImage()} title="Go to this message in the terminal">
        <span class="q"><i>❯</i> {img.asked}</span>
        <span class="go">⤶ Go to message <kbd>L</kbd></span>
      </button>
    {:else}
      <div class="bottom pill asked gone">The message this was sent with is no longer in the terminal’s history.</div>
    {/if}
  </div>
{/if}

<style>
  /* Below the app's title bar: its window buttons and controls stay put. */
  .viewer {
    position: fixed;
    inset: 38px 0 0 0;
    z-index: 90;
    background: rgba(6, 6, 9, 0.5);
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
  }
  .pill {
    position: absolute;
    left: 50%;
    z-index: 2;
    transform: translateX(-50%);
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    border: 1px solid var(--border);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  }
  .top {
    top: 14px;
    display: flex;
    align-items: center;
    gap: 2px;
    height: 36px;
    padding: 0 4px 0 12px;
    border-radius: 999px;
    white-space: nowrap;
  }
  /* Wide enough for `[Image #999]`, so the pill is one width throughout. */
  .tag {
    min-width: 12ch;
    color: var(--fg);
    font: 12px ui-monospace, Menlo, monospace;
    text-align: center;
  }
  .edge {
    top: 58px;
    padding: 3px 10px;
    border-radius: 999px;
    color: var(--dim);
    font-size: 11px;
    white-space: nowrap;
  }
  .nav,
  .close {
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--dim);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
  }
  .close {
    margin-left: 6px;
  }
  .nav {
    display: none;
    font-size: 20px;
  }
  .nav:hover:not(:disabled),
  .close:hover {
    background: var(--panel-2);
    color: var(--fg);
  }
  .nav:disabled {
    opacity: 0.3;
    cursor: default;
  }
  .side {
    position: absolute;
    top: 50%;
    z-index: 2;
    width: 40px;
    height: 40px;
    margin-top: -20px;
    padding-bottom: 3px;
    border: 1px solid var(--border);
    border-radius: 50%;
    background: var(--panel);
    color: var(--fg);
    font-size: 22px;
    line-height: 1;
    cursor: pointer;
  }
  .side:hover:not(:disabled) {
    background: var(--panel-2);
  }
  .side:disabled {
    opacity: 0.3;
    cursor: default;
  }
  .prev {
    left: max(16px, env(safe-area-inset-left));
  }
  .next {
    right: max(16px, env(safe-area-inset-right));
  }
  /* Room for the pills above and below, the arrows either side. */
  .stage {
    position: absolute;
    inset: 64px 72px 86px;
  }
  img {
    position: absolute;
    inset: 0;
    margin: auto;
    max-width: 100%;
    max-height: 100%;
    border-radius: 4px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.9);
    transition: transform 0.15s ease-out, opacity 0.15s ease-out;
    will-change: transform;
    -webkit-user-drag: none;
  }
  img.moving {
    transition: none;
  }
  .note {
    top: 50%;
    margin: 0;
    padding: 8px 14px;
    border-radius: 8px;
    transform: translate(-50%, -50%);
    color: var(--dim);
    font-size: 13px;
  }
  /* One size, two lines of question, however many it has. */
  .bottom {
    bottom: max(16px, env(safe-area-inset-bottom));
    display: flex;
    align-items: flex-start;
    gap: 10px;
    width: min(720px, calc(100% - 32px));
    height: calc(2 * 1.55 * 12.5px + 18px);
    padding: 9px 14px;
    border-radius: 14px;
    color: var(--fg);
    text-align: left;
    font: inherit;
  }
  button.asked {
    cursor: pointer;
  }
  .gone {
    align-items: center;
    color: var(--faint);
    font-size: 12px;
  }
  .asked:hover .go {
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .q {
    flex: 1;
    min-width: 0;
    font: 12.5px/1.55 ui-monospace, Menlo, monospace;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
  }
  .q i {
    font-style: normal;
    color: var(--accent);
  }
  .go {
    flex: none;
    padding-top: 1px;
    color: var(--accent);
    font-size: 11.5px;
    white-space: nowrap;
  }
  kbd {
    margin-left: 4px;
    padding: 0 4px;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--dim);
    font: 10.5px ui-monospace, Menlo, monospace;
  }
  /* Phones: arrows in the pill, swipes for the rest; bigger targets; the
     pills nearly solid, as the key bar's labels would show through. */
  @media (pointer: coarse) {
    kbd,
    .side {
      display: none;
    }
    .nav {
      display: inline-block;
      width: 36px;
      height: 36px;
    }
    .close {
      width: 36px;
      height: 36px;
    }
    .top {
      height: 44px;
      padding-left: 4px;
    }
    .edge {
      top: 66px;
    }
    .pill {
      background: color-mix(in srgb, var(--panel) 97%, transparent);
    }
    .stage {
      inset: 70px 8px 88px;
    }
  }
</style>
