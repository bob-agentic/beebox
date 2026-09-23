// Drag-to-reorder for a list of rows.
//
// Deliberately not the HTML5 drag-and-drop API: that needs a drag image, fires
// `dragover` on the wrong targets in nested scroll containers, and behaves
// differently in a webview than in a browser. Pointer events do the same job
// with less to go wrong, and work with touch for free.
//
// The row you are holding follows the pointer under a transform, and the rows
// it displaces slide out of its way. Reordering the DOM alone — which is what
// this used to do — is correct and reads as broken: nothing tracks your hand,
// so there is no sense of carrying anything, and every row it passes jumps
// rather than moves.

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
  /** Selector for somewhere outside the list a row can be dropped instead. */
  dropTarget?: string;
  /** Called instead of `commit` when the drop landed on `dropTarget`. The
      element is passed too, since one selector can match several places to
      drop — two shelves, say — and the caller has to tell them apart. */
  drop?: (id: number, on: HTMLElement | null) => void;
}

/** Long enough that a sloppy click is not a drag, short enough to feel direct. */
const SLOP = 4;
/** One frame under 200ms reads as instant while still being followable. */
const SLIDE_MS = 180;

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

    // Measured once the drag starts, so the arithmetic below is not fighting
    // the transforms it is applying.
    let slot = 0;
    let home = 0;
    let index = 0;
    let others: { el: HTMLElement; home: number; shift: number }[] = [];
    /** The drop target currently under the pointer, if any. */
    let overZone: Element | null = null;

    const rows = () =>
      [...(node.parentElement?.children ?? [])].filter(
        (el): el is HTMLElement => el instanceof HTMLElement && el.dataset.sortId != null,
      );

    const offsetOf = (el: HTMLElement) => {
      const r = el.getBoundingClientRect();
      return axis === 'y' ? r.top : r.left;
    };
    const sizeOf = (el: HTMLElement) => {
      const r = el.getBoundingClientRect();
      return axis === 'y' ? r.height : r.width;
    };

    function begin(ev: PointerEvent) {
      dragging = true;
      const all = rows();
      index = all.indexOf(node);
      home = offsetOf(node);
      // The gap a row leaves behind is its own size plus whatever the list puts
      // between rows; reading it from the next row keeps CSS `gap` out of here.
      const next = all[index + 1];
      slot = next ? offsetOf(next) - home : sizeOf(node);

      others = all
        .filter((el) => el !== node)
        .map((el) => ({ el, home: offsetOf(el), shift: 0 }));

      node.classList.add('dragging');
      // Above its neighbours while it is in hand, or it slides underneath them.
      node.style.zIndex = '5';
      node.style.position = 'relative';
      node.setPointerCapture(ev.pointerId);
      for (const o of others) o.el.style.transition = `transform ${SLIDE_MS}ms ease`;
    }

    function move(ev: PointerEvent) {
      const pos = axis === 'y' ? ev.clientY : ev.clientX;
      if (!dragging && Math.abs(pos - startPos) < SLOP) return;
      if (!dragging) begin(ev);

      const delta = pos - startPos;

      // Over the drop target? Then this is not a reorder, and the rows must
      // not shuffle as though it were.
      const zone = current.dropTarget
        ? document
            .elementFromPoint(ev.clientX, ev.clientY)
            ?.closest(current.dropTarget) ?? null
        : null;
      if (zone !== overZone) {
        overZone?.classList.remove('drop-over');
        zone?.classList.add('drop-over');
        overZone = zone;
      }
      if (zone) {
        node.style.transform =
          axis === 'y' ? `translateY(${delta}px)` : `translateX(${delta}px)`;
        for (const o of others) {
          if (o.shift !== 0) {
            o.shift = 0;
            o.el.style.transform = '';
          }
        }
        return;
      }
      // The held row tracks the pointer exactly. No transition on this one:
      // easing the thing under your finger is what makes a drag feel laggy.
      node.style.transform =
        axis === 'y' ? `translateY(${delta}px)` : `translateX(${delta}px)`;

      // How many slots it has travelled, by where its own leading edge now sits.
      const moved = Math.round(delta / slot);
      const dest = Math.max(0, Math.min(others.length, index + moved));

      // Everything between the row's old index and its new one steps aside by
      // exactly one slot — the space the held row will occupy.
      for (let i = 0; i < others.length; i++) {
        const o = others[i];
        // Index of this row in the list as it looks with `node` taken out.
        const before = i < index;
        let shift = 0;
        if (before && i >= dest) shift = slot;
        else if (!before && i < dest) shift = -slot;
        if (o.shift !== shift) {
          o.shift = shift;
          o.el.style.transform = shift
            ? axis === 'y'
              ? `translateY(${shift}px)`
              : `translateX(${shift}px)`
            : '';
        }
      }
      (node as any).__dest = dest;
    }

    function up(ev: PointerEvent) {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
      if (!dragging) return;

      const dest: number = (node as any).__dest ?? index;
      delete (node as any).__dest;

      // Drop the transforms in the same frame the DOM order changes, so the
      // row lands where it already appears to be instead of flashing home.
      for (const o of others) {
        o.el.style.transition = '';
        o.el.style.transform = '';
      }
      node.classList.remove('dragging');
      node.style.transform = '';
      node.style.zIndex = '';
      node.style.position = '';
      if (node.hasPointerCapture(ev.pointerId)) node.releasePointerCapture(ev.pointerId);

      // Suppress the click that would otherwise follow the drop. Registered
      // before the early return below, because dropping on the zone would
      // otherwise activate the very row it just took away.
      const swallow = (c: Event) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener('click', swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);

      if (overZone) {
        const landed = overZone;
        landed.classList.remove('drop-over');
        overZone = null;
        // The DOM is left alone: the row is about to leave this list, and the
        // server's next tree is what removes it.
        current.drop?.(current.id, landed);
        return;
      }

      const parent = node.parentElement;
      if (parent) {
        const rest = rows().filter((el) => el !== node);
        const anchor = rest[dest] ?? null;
        parent.insertBefore(node, anchor);
      }

      const next = rows()
        .map((el) => Number(el.dataset.sortId))
        .filter((n) => Number.isFinite(n));
      const before = current.order();
      if (next.length === before.length && next.some((id, i) => id !== before[i])) {
        current.commit(next);
      }
    }

    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    // A cancelled pointer (a system gesture, a lost capture) must not leave the
    // row stuck mid-drag with a transform on it.
    window.addEventListener('pointercancel', up);
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
