// The app's own shortcuts, per platform. A Mac keeps ⌘. Elsewhere they are
// Ctrl+Shift, as in Windows Terminal and GNOME Terminal: plain Ctrl belongs
// to the terminal (Ctrl+C interrupts, Ctrl+W deletes a word), and the Windows
// or Super key to the system, which takes Win+D, Win+T and Win+N for itself.

export type AppCommand = 'settings' | 'search_conversations' | 'split_v' | 'prev_tab' | 'next_tab';

export const isMac = /Macintosh|Mac OS X|iPhone|iPad/.test(globalThis.navigator?.userAgent ?? '');

/** The command a keystroke means off a Mac, or null. Only these four: the
    rest have no free chord in a browser tab (Ctrl+Shift+N/T/W are the
    browser's own) and are reached by their buttons. */
export function otherCommand(e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey'>): AppCommand | null {
  if (!e.ctrlKey || e.altKey || e.metaKey) return null;
  if (!e.shiftKey) return e.key === ',' ? 'settings' : null;
  switch (e.key.toLowerCase()) {
    case 'f':
      return 'search_conversations';
    case 'd':
      return 'split_v';
    // Shift turns [ and ] into { and } on most layouts.
    case '[':
    case '{':
      return 'prev_tab';
    case ']':
    case '}':
      return 'next_tab';
    default:
      return null;
  }
}

/** A tooltip with the shortcut as this platform writes it, or just the label
    where it has none. */
export function withChord(label: string, mac: string, other: string | null): string {
  const chord = isMac ? mac : other;
  return chord ? `${label}  ${chord}` : label;
}
