// Drag-to-reorder for a list of rows.
//
// Deliberately not the HTML5 drag-and-drop API: that needs a drag image, fires
// `dragover` on the wrong targets in nested scroll containers, and behaves
// differently in a webview than in a browser. Pointer events do the same job
// with less to go wrong, and work with touch for free.

export interface SortOptions {
  /** Stable id of this row. */
  id: number;
  /** Ids in their current visual order. */
  order: () => number[];
  /** Called once on drop, with the new order. */
  commit: (order: number[]) => void;
  /** 'y' for a sidebar list, 'x' for a tab strip. */
  axis?: 'x' | 'y';
  /** Skips the drag when the pointer started on one of these. */
  ignore?: string;
}

export function sortable(node: HTMLElement, opts: SortOptions) {
  let current = opts;

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (current.ignore && target.closest(current.ignore)) return;
    // An input means the row is being renamed.
    if (target.closest('input, textarea')) return;

    const axis = current.axis ?? 'y';
    const startPos = axis === 'y' ? e.clientY : e.clientX;
    let dragging = false;

    const siblings = () =>
      [...(node.parentElement?.children ?? [])].filter(
        (el): el is HTMLElement => el instanceof HTMLElement && el.dataset.sortId != null,
      );

    function move(ev: PointerEvent) {
      const pos = axis === 'y' ? ev.clientY : ev.clientX;
      // A few pixels of slop, so a click is still a click.
      if (!dragging && Math.abs(pos - startPos) < 5) return;

      if (!dragging) {
        dragging = true;
        node.classList.add('dragging');
        node.setPointerCapture(ev.pointerId);
      }

      // Move to wherever the pointer is, not one place towards it: swapping
      // with a single neighbour per event leaves a fast drag stranded partway.
      const others = siblings().filter((el) => el !== node);
      const parent = node.parentElement;
      if (!parent) return;

      // The first row whose midpoint is past the pointer is the insertion
      // point; if there is none, the pointer is past the end.
      const target = others.find((el) => {
        const r = el.getBoundingClientRect();
        const mid = axis === 'y' ? r.top + r.height / 2 : r.left + r.width / 2;
        return pos < mid;
      });

      if (target) {
        if (node.nextSibling !== target) parent.insertBefore(node, target);
      } else {
        const last = others[others.length - 1];
        if (last && node.previousSibling !== last) {
          parent.insertBefore(node, last.nextSibling);
        }
      }
    }

    function up(ev: PointerEvent) {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      if (!dragging) return;

      node.classList.remove('dragging');
      if (node.hasPointerCapture(ev.pointerId)) node.releasePointerCapture(ev.pointerId);

      // Suppress the click that would otherwise follow the drop.
      const swallow = (c: Event) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener('click', swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);

      const next = siblings()
        .map((el) => Number(el.dataset.sortId))
        .filter((n) => Number.isFinite(n));
      const before = current.order();
      if (next.length === before.length && next.some((id, i) => id !== before[i])) {
        current.commit(next);
      }
    }

    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  node.addEventListener('pointerdown', down);
  node.dataset.sortId = String(opts.id);

  return {
    update(next: SortOptions) {
      current = next;
      node.dataset.sortId = String(next.id);
    },
    destroy() {
      node.removeEventListener('pointerdown', down);
    },
  };
}
