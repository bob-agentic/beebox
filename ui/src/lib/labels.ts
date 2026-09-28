import type { PaneView, TabView } from './proto';

/** A shell's OSC title is "user@host:/path", which tells you nothing you
    did not already know. Priority: the user's own name for the tab, then
    the focused agent session's title, then the running command, then the
    folder — never the hostname. */
export function tabLabel(tab: TabView): string {
  if (tab.title) return tab.title;
  const session = tab.panes.find((p) => p.session_title)?.session_title;
  if (session) return session;
  const p = tab.panes[0];
  return p ? paneLabel(p) : 'shell';
}

/** The same order for one pane, minus the tab's own name. */
export function paneLabel(p: PaneView): string {
  if (p.session_title) return p.session_title;
  const t = p.title ?? '';
  if (t && !/^[\w.-]+@[\w.-]+[:\s]/.test(t)) return t;
  return p.cwd.split('/').filter(Boolean).pop() || 'shell';
}
