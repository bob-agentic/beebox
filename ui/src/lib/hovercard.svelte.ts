// The card that shows a row's full name and where it lives — sidebar
// workspaces and tabs both truncate, and a native `title` was slow to appear
// and could only say one thing.
//
// A mouse rests on the row; a finger holds it, since a phone has no hover.
// One card on screen at a time, drawn by HoverCard.svelte.

export interface CardRow {
  k: string;
  v: string;
  /** Paths and branches, which read better in a fixed width. */
  mono?: boolean;
}

export interface CardContent {
  title: string;
  rows: CardRow[];
}

export interface CardOptions {
  /** Read when the card opens, so it says what is true then. */
  content: () => CardContent | null;
  /** Where it opens for a mouse. A finger always gets it below: on a phone
      the sidebar is most of the screen and there is no room to its right. */
  side: 'right' | 'below';
  /** Parts of the row that have their own tooltip or job, like a close button. */
  ignore?: string;
  /** Whether a long-press opens the row's own menu right now, which then
      takes the hold instead of the card. */
  menu?: () => boolean;
}

export interface OpenCard {
  content: CardContent;
  anchor: DOMRect;
  side: 'right' | 'below';
  owner: HTMLElement;
}

/** Long enough that sweeping across the sidebar does not flash cards. */
const REST_MS = 350;
/** A fallback for browsers without a long-press `contextmenu` (iOS Safari).
    Longer than Android's own long-press, which is the real trigger there. */
const HOLD_MS = 650;
/** A finger that moves this far is scrolling or dragging, not holding. The
    same distance at which dragsort starts a drag, so a hold never turns into
    a card in the middle of one. */
const SLOP = 4;

export const card = $state<{ open: OpenCard | null }>({ open: null });

export function hideCard() {
  card.open = null;
}

export function hovercard(node: HTMLElement, opts: CardOptions) {
  let current = opts;
  let timer = 0;
  let start = { x: 0, y: 0 };
  let holding = false;
  /** Clicked: the row has been dealt with, so no card until the mouse leaves. */
  let clicked = false;

  const ignored = (e: Event) =>
    !!current.ignore && !!(e.target as HTMLElement).closest(current.ignore);

  function cancel() {
    clearTimeout(timer);
    timer = 0;
  }

  function lift() {
    cancel();
    holding = false;
  }

  // Lifting the finger after a hold would otherwise click the row and switch
  // to it. Only that click: one a moment later is the next tap.
  let swallowing = false;
  function swallow(e: MouseEvent) {
    e.stopPropagation();
    e.preventDefault();
  }
  function startSwallow() {
    if (swallowing) return;
    swallowing = true;
    addEventListener('click', swallow, { capture: true });
  }
  function endSwallow() {
    if (!swallowing) return;
    setTimeout(() => {
      swallowing = false;
      removeEventListener('click', swallow, { capture: true });
    }, 300);
  }

  function holdOpen() {
    cancel();
    show('below');
    if (card.open?.owner === node) startSwallow();
  }

  // A hold is Android's long-press, which arrives as `contextmenu`. A row with
  // a menu of its own (the owner's Rename / Close) takes it and gets no card;
  // otherwise the card is what a hold means. Asked of the row rather than read
  // off `defaultPrevented`: the app cancels every `contextmenu` on its chrome.
  function longPress() {
    if (!holding) return;
    if (current.menu?.()) {
      lift();
      if (card.open?.owner === node) hideCard();
    } else {
      holdOpen();
    }
  }

  function show(side: 'right' | 'below') {
    timer = 0;
    const content = current.content();
    if (!content) return;
    card.open = { content, anchor: node.getBoundingClientRect(), side, owner: node };
  }

  function move(e: PointerEvent) {
    if (e.pointerType === 'mouse') {
      if (ignored(e) || e.buttons || clicked) {
        cancel();
        if (card.open?.owner === node) hideCard();
      } else if (!timer && card.open?.owner !== node) {
        timer = setTimeout(() => show(current.side), REST_MS);
      }
    } else if (holding && Math.hypot(e.clientX - start.x, e.clientY - start.y) > SLOP) {
      lift();
    }
  }

  function leave(e: PointerEvent) {
    if (e.pointerType !== 'mouse') return;
    cancel();
    clicked = false;
    if (card.open?.owner === node) hideCard();
  }

  function down(e: PointerEvent) {
    cancel();
    if (e.pointerType === 'mouse') {
      clicked = true;
      hideCard();
      return;
    }
    if (ignored(e) || (e.target as HTMLElement).closest('input')) return;
    start = { x: e.clientX, y: e.clientY };
    holding = true;
    timer = setTimeout(holdOpen, HOLD_MS);
  }

  function up() {
    lift();
    endSwallow();
  }

  node.addEventListener('pointermove', move);
  node.addEventListener('pointerleave', leave);
  node.addEventListener('pointerdown', down);
  node.addEventListener('pointerup', up);
  node.addEventListener('pointercancel', up);
  node.addEventListener('contextmenu', longPress);

  return {
    update(next: CardOptions) {
      current = next;
    },
    destroy() {
      cancel();
      if (card.open?.owner === node) hideCard();
      node.removeEventListener('pointermove', move);
      node.removeEventListener('pointerleave', leave);
      node.removeEventListener('pointerdown', down);
      node.removeEventListener('pointerup', up);
      node.removeEventListener('pointercancel', up);
      node.removeEventListener('contextmenu', longPress);
      if (swallowing) removeEventListener('click', swallow, { capture: true });
    },
  };
}

/** `/Users/bob/x` → `~/x`. The home folder is the same on every row, so it
    is just width. */
export function tildify(path: string): string {
  const m = /^\/(Users|home)\/[^/]+/.exec(path);
  return m ? '~' + path.slice(m[0].length) : path;
}
