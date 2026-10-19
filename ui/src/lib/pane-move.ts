// Moving a pane by dragging its head: onto another pane's middle to trade
// places, onto an edge to take that side. The server decides (and caps the
// depth); the copy of its rules here only draws what a drop would do, so a
// drop it would refuse shows red instead of doing nothing.
//
// A finger has to hold the head first — a tap there only focuses the pane,
// and a quick swipe drags nothing — and the head stays lit while held, so it
// reads as picked up.

import { tick } from 'svelte';
import { store } from './state.svelte';
import { leaves, type Dir, type Node, type PaneId, type Side } from './proto';

/** Mirrors `MAX_SPLIT_DEPTH` in session.rs. */
const MAX_DEPTH = 2;
const HOLD_MS = 300;
/** Within this fraction of a side, a drop takes that side. */
const EDGE = 0.25;

const LABEL: Record<Side, string> = {
  center: 'Swap',
  left: 'Move left',
  right: 'Move right',
  up: 'Move above',
  down: 'Move below',
};

// ---- the server's rules, for the preview ----

function deepest(n: Node, d = 0): number {
  return n.kind === 'leaf' ? d : Math.max(...n.children.map((c) => deepest(c, d + 1)));
}

function split(dir: Dir, children: Node[]): Node {
  return { kind: 'split', dir, children, sizes: children.map(() => 1 / children.length) };
}

function without(n: Node, pane: PaneId): Node | null {
  if (n.kind === 'leaf') return n.pane === pane ? null : n;
  const kept = n.children.map((c) => without(c, pane)).filter((c): c is Node => c !== null);
  if (kept.length === 1) return kept[0];
  // A split left inside one running the same way merges into it.
  return split(n.dir, kept.flatMap((c) => (c.kind === 'split' && c.dir === n.dir ? c.children : [c])));
}

function beside(n: Node, target: PaneId, pane: PaneId, side: Side): Node {
  const dir = side === 'left' || side === 'right' ? 'vertical' : 'horizontal';
  const before = side === 'left' || side === 'up';
  if (n.kind === 'leaf') {
    if (n.pane !== target) return n;
    const p: Node = { kind: 'leaf', pane };
    return split(dir, before ? [p, n] : [n, p]);
  }
  const i = n.children.findIndex((c) => c.kind === 'leaf' && c.pane === target);
  if (i >= 0 && n.dir === dir) {
    const children = [...n.children];
    children.splice(before ? i : i + 1, 0, { kind: 'leaf', pane });
    return split(dir, children);
  }
  return { ...n, children: n.children.map((c) => beside(c, target, pane, side)) };
}

/** Whether the server would take this move. Trading places always fits. */
function fits(layout: Node, pane: PaneId, target: PaneId, side: Side): boolean {
  if (side === 'center') return true;
  const rest = without(layout, pane);
  return rest !== null && deepest(beside(rest, target, pane, side)) <= MAX_DEPTH;
}

// ---- sliding panes to where they went ----

function rects(): Map<PaneId, DOMRect> {
  const m = new Map<PaneId, DOMRect>();
  document.querySelectorAll<HTMLElement>('[data-pane]').forEach((el) => {
    m.set(Number(el.dataset.pane), el.getBoundingClientRect());
  });
  return m;
}

/** Each pane glides from where it was, so it is plain who went where. */
function slide(from: Map<PaneId, DOMRect>) {
  for (const [id, b] of rects()) {
    const a = from.get(id);
    if (!a || (Math.abs(a.left - b.left) < 1 && Math.abs(a.top - b.top) < 1 && Math.abs(a.width - b.width) < 1)) continue;
    document.querySelector<HTMLElement>(`[data-pane="${id}"]`)?.animate(
      [
        {
          transformOrigin: '0 0',
          transform: `translate(${a.left - b.left}px, ${a.top - b.top}px) scale(${a.width / b.width}, ${a.height / b.height})`,
        },
        { transformOrigin: '0 0', transform: 'none' },
      ],
      { duration: 220, easing: 'cubic-bezier(0.33, 1, 0.68, 1)' },
    );
  }
}

/** Trades places with a neighbour — the narrow, one-column layout's only move. */
export function swap(pane: PaneId, target: PaneId) {
  move(pane, target, 'center');
}

function move(pane: PaneId, target: PaneId, side: Side) {
  const from = rects();
  store.onTree = () => {
    store.onTree = null;
    void tick().then(() => slide(from));
  };
  store.send({ t: 'move_pane', pane, target, side });
}

// ---- the drag ----

/** From a pointerdown on a pane's head. */
export function startMove(e: PointerEvent, pane: PaneId, title: string) {
  // Stacked panes trade by ↑ ↓ instead; the head there is for scrolling.
  if (e.button !== 0 || !store.caps.writable || matchMedia('(max-width: 640px)').matches) return;
  const tab = store.tabOf(pane);
  if (!tab || leaves(tab.layout).length < 2) return;
  const head = e.currentTarget as HTMLElement;
  const touch = e.pointerType !== 'mouse';
  const x0 = e.clientX;
  const y0 = e.clientY;
  let armed = !touch;
  let ghost: HTMLElement | null = null;
  let zone: HTMLElement | null = null;
  let drop: { target: PaneId; side: Side } | null = null;

  const hold = touch
    ? setTimeout(() => {
        armed = true;
        head.classList.add('lifted');
        navigator.vibrate?.(10);
      }, HOLD_MS)
    : 0;

  const begin = () => {
    document.querySelector(`[data-pane="${pane}"]`)?.classList.add('moving');
    ghost = document.createElement('div');
    ghost.className = 'move-ghost';
    ghost.textContent = title;
    zone = document.createElement('div');
    zone.className = 'move-zone';
    document.body.append(ghost, zone);
    document.body.classList.add('pane-moving');
  };

  const onMove = (ev: PointerEvent) => {
    if (!ghost) {
      if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < 5) return;
      // Moved before the hold was up: a swipe, not a drag.
      if (!armed) return end();
      begin();
    }
    ev.preventDefault();
    ghost!.style.left = `${ev.clientX}px`;
    ghost!.style.top = `${ev.clientY}px`;
    ghost!.classList.toggle('flip', ev.clientX + ghost!.offsetWidth + 24 > innerWidth);

    const over = document
      .elementsFromPoint(ev.clientX, ev.clientY)
      .map((el) => el.closest<HTMLElement>('[data-pane]'))
      .find((el) => el && Number(el.dataset.pane) !== pane);
    const target = over ? Number(over.dataset.pane) : null;
    if (!over || target === null || store.tabOf(target)?.id !== tab.id) {
      drop = null;
      zone!.style.display = 'none';
      return;
    }
    const r = over.getBoundingClientRect();
    const fx = (ev.clientX - r.left) / r.width;
    const fy = (ev.clientY - r.top) / r.height;
    const edge = Math.min(fx, 1 - fx, fy, 1 - fy);
    const side: Side =
      edge >= EDGE ? 'center' : edge === fx ? 'left' : edge === 1 - fx ? 'right' : edge === fy ? 'up' : 'down';
    const ok = fits(tab.layout, pane, target, side);
    drop = ok ? { target, side } : null;

    let { left, top, width, height } = r;
    if (side === 'left' || side === 'right') width /= 2;
    if (side === 'right') left += width;
    if (side === 'up' || side === 'down') height /= 2;
    if (side === 'down') top += height;
    Object.assign(zone!.style, {
      display: 'flex',
      left: `${left}px`,
      top: `${top}px`,
      width: `${width}px`,
      height: `${height}px`,
    });
    zone!.classList.toggle('no', !ok);
    zone!.innerHTML = '';
    const label = document.createElement('span');
    label.textContent = ok ? LABEL[side] : 'Nests too deep';
    zone!.append(label);
  };

  const onUp = () => {
    const d = drop;
    end();
    if (d) move(pane, d.target, d.side);
  };

  // A long press would otherwise bring up the browser's own menu.
  const noMenu = (ev: Event) => armed && ev.preventDefault();

  function end() {
    clearTimeout(hold);
    head.classList.remove('lifted');
    document.querySelector(`[data-pane="${pane}"]`)?.classList.remove('moving');
    ghost?.remove();
    zone?.remove();
    document.body.classList.remove('pane-moving');
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', onUp);
    window.removeEventListener('pointercancel', end);
    window.removeEventListener('contextmenu', noMenu);
  }

  window.addEventListener('pointermove', onMove, { passive: false });
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointercancel', end);
  window.addEventListener('contextmenu', noMenu);
}
