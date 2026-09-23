// Exercises the reorder action against real DOM nodes and real pointer events.
// The interaction cannot be verified by eye from here, so the logic that
// decides the new order is pinned down instead.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { sortable } from './dragsort.svelte';

/** Lays out `n` rows of fixed height in a container, with stable ids. */
function makeList(n: number, axis: 'x' | 'y' = 'y') {
  const parent = document.createElement('div');
  document.body.replaceChildren(parent);

  const size = 40;
  const rows = Array.from({ length: n }, (_, i) => {
    const el = document.createElement('div');
    parent.appendChild(el);
    // jsdom has no layout, so the geometry the action reads is stubbed.
    const top = i * size;
    el.getBoundingClientRect = () =>
      ({
        top: axis === 'y' ? top : 0,
        bottom: axis === 'y' ? top + size : size,
        left: axis === 'x' ? top : 0,
        right: axis === 'x' ? top + size : size,
        height: size,
        width: size,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      }) as DOMRect;
    el.setPointerCapture = () => {};
    el.releasePointerCapture = () => {};
    el.hasPointerCapture = () => false;
    return el;
  });

  return { parent, rows, size };
}

function pointer(type: string, pos: number, axis: 'x' | 'y' = 'y') {
  return new PointerEvent(type, {
    bubbles: true,
    button: 0,
    pointerId: 1,
    clientX: axis === 'x' ? pos : 0,
    clientY: axis === 'y' ? pos : 0,
  });
}

/** Presses on `row`, moves to `to`, releases. */
function drag(row: HTMLElement, from: number, to: number, axis: 'x' | 'y' = 'y') {
  row.dispatchEvent(pointer('pointerdown', from, axis));
  const steps = 8;
  for (let i = 1; i <= steps; i++) {
    window.dispatchEvent(pointer('pointermove', from + ((to - from) * i) / steps, axis));
  }
  window.dispatchEvent(pointer('pointerup', to, axis));
}

describe('sortable', () => {
  beforeEach(() => {
    document.body.replaceChildren();
  });

  it('reports the new order after a downward drag', () => {
    const { rows } = makeList(3);
    const commit = vi.fn();
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit }),
    );

    // First row, dragged past the last.
    drag(rows[0], 20, 110);

    expect(commit).toHaveBeenCalledTimes(1);
    expect(commit.mock.calls[0][0]).toEqual([2, 3, 1]);
  });

  it('reports the new order after an upward drag', () => {
    const { rows } = makeList(3);
    const commit = vi.fn();
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit }),
    );

    drag(rows[2], 100, 10);

    expect(commit.mock.calls[0][0]).toEqual([3, 1, 2]);
  });

  it('carries the held row under the pointer', () => {
    // Reordering the DOM without moving anything is what made this feel
    // broken: nothing tracked the hand, so there was no sense of carrying a
    // row at all.
    const { rows } = makeList(3);
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit: vi.fn() }),
    );

    rows[0].dispatchEvent(pointer('pointerdown', 20));
    window.dispatchEvent(pointer('pointermove', 55));

    expect(rows[0].classList.contains('dragging')).toBe(true);
    expect(rows[0].style.transform).toBe('translateY(35px)');

    // And it is put back on drop, so the row lands in its new slot rather
    // than sitting 35px off it.
    window.dispatchEvent(pointer('pointerup', 55));
    expect(rows[0].style.transform).toBe('');
    expect(rows[0].classList.contains('dragging')).toBe(false);
  });

  it('slides the displaced rows aside by exactly one slot', () => {
    const { rows, size } = makeList(3);
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit: vi.fn() }),
    );

    // Hold the first row over the second.
    rows[0].dispatchEvent(pointer('pointerdown', 20));
    window.dispatchEvent(pointer('pointermove', 20 + size));

    // The row being passed steps back by one slot to open the gap; the one
    // beyond it has no reason to move.
    expect(rows[1].style.transform).toBe(`translateY(-${size}px)`);
    expect(rows[2].style.transform).toBe('');

    window.dispatchEvent(pointer('pointerup', 20 + size));
    expect(rows[1].style.transform).toBe('');
  });

  it('cleans up when the pointer is cancelled mid-drag', () => {
    // A system gesture can cancel a pointer. Without this the row keeps its
    // transform and stays stuck out of place.
    const { rows } = makeList(3);
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit: vi.fn() }),
    );

    rows[0].dispatchEvent(pointer('pointerdown', 20));
    window.dispatchEvent(pointer('pointermove', 55));
    window.dispatchEvent(pointer('pointercancel', 55));

    expect(rows[0].style.transform).toBe('');
    expect(rows[0].classList.contains('dragging')).toBe(false);
  });

  it('reports a drop on the zone instead of a reorder', () => {
    // Dropping a tab on a shelf takes it out of the list, so the lengths can
    // never match — the reorder path would discard it silently. The element
    // comes back with the id, since one selector can match several shelves
    // and the caller has to tell which one took the drop.
    const { rows } = makeList(3);
    const commit = vi.fn();
    const drop = vi.fn();

    const zone = document.createElement('div');
    zone.className = 'shelf';
    document.body.appendChild(zone);
    // jsdom resolves elementFromPoint to nothing; point it at the zone.
    const from = document.elementFromPoint;
    document.elementFromPoint = () => zone;

    try {
      rows.forEach((el, i) =>
        sortable(el, {
          id: i + 1,
          order: () => [1, 2, 3],
          commit,
          drop,
          dropTarget: '.shelf',
        }),
      );
      drag(rows[0], 20, 90);

      expect(drop).toHaveBeenCalledWith(1, zone);
      expect(commit).not.toHaveBeenCalled();
      expect(zone.classList.contains('drop-over')).toBe(false);
      // The row is left where it was: the server's next tree removes it.
      expect(rows[0].style.transform).toBe('');
    } finally {
      document.elementFromPoint = from;
      zone.remove();
    }
  });

  it('works along the x axis, for a tab strip', () => {
    const { rows } = makeList(3, 'x');
    const commit = vi.fn();
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, axis: 'x', order: () => [1, 2, 3], commit }),
    );

    drag(rows[0], 20, 110, 'x');

    expect(commit.mock.calls[0][0]).toEqual([2, 3, 1]);
  });

  it('does not fire on a click', () => {
    // A few pixels of movement is a click, not a drag — otherwise selecting a
    // row would reorder the list.
    const { rows } = makeList(3);
    const commit = vi.fn();
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit }),
    );

    drag(rows[0], 20, 22);

    expect(commit).not.toHaveBeenCalled();
  });

  it('does not fire when the order is unchanged', () => {
    const { rows } = makeList(3);
    const commit = vi.fn();
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit }),
    );

    // Moved far enough to count as a drag, but dropped back in place.
    rows[1].dispatchEvent(pointer('pointerdown', 60));
    window.dispatchEvent(pointer('pointermove', 75));
    window.dispatchEvent(pointer('pointermove', 60));
    window.dispatchEvent(pointer('pointerup', 60));

    expect(commit).not.toHaveBeenCalled();
  });

  it('ignores a drag that starts on an excluded control', () => {
    // The close button must stay clickable.
    const { rows } = makeList(3);
    const commit = vi.fn();
    const x = document.createElement('span');
    x.className = 'x';
    rows[0].appendChild(x);
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, ignore: '.x', order: () => [1, 2, 3], commit }),
    );

    x.dispatchEvent(pointer('pointerdown', 20));
    window.dispatchEvent(pointer('pointermove', 110));
    window.dispatchEvent(pointer('pointerup', 110));

    expect(commit).not.toHaveBeenCalled();
  });

  it('ignores a drag that starts in a rename field', () => {
    const { rows } = makeList(3);
    const commit = vi.fn();
    const input = document.createElement('input');
    rows[0].appendChild(input);
    rows.forEach((el, i) =>
      sortable(el, { id: i + 1, order: () => [1, 2, 3], commit }),
    );

    input.dispatchEvent(pointer('pointerdown', 20));
    window.dispatchEvent(pointer('pointermove', 110));
    window.dispatchEvent(pointer('pointerup', 110));

    expect(commit).not.toHaveBeenCalled();
  });
});
