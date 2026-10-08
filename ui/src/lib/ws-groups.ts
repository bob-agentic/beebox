// How the sidebar nests worktrees under their repository.
//
// Worked out on every render from what each workspace says about itself
// (`repo`, `worktree`), never stored: open the repository and its worktrees
// move under it, close it and they stand on their own again, with nothing to
// keep in step.

import type { WorkspaceView, WsId } from './proto';

export interface WsGroup {
  ws: WorkspaceView;
  /** Linked worktrees of `ws`'s repository, in the order they were opened. */
  kids: WorkspaceView[];
}

/** Top-level rows in list order. A worktree goes under the first open
    checkout of its repository — not under another worktree — and stands on
    its own when there is none. */
export function groupWorkspaces(list: WorkspaceView[]): WsGroup[] {
  const parentOf = new Map<WsId, WsId>();
  for (const w of list) {
    if (!w.worktree || !w.repo) continue;
    const home = list.find((p) => !p.worktree && p.repo === w.repo);
    if (home) parentOf.set(w.id, home.id);
  }
  const groups = new Map<WsId, WsGroup>();
  const out: WsGroup[] = [];
  for (const w of list) {
    if (parentOf.has(w.id)) continue;
    const g = { ws: w, kids: [] };
    groups.set(w.id, g);
    out.push(g);
  }
  for (const w of list) {
    const p = parentOf.get(w.id);
    if (p !== undefined) groups.get(p)!.kids.push(w);
  }
  return out;
}

/** The full workspace order after dragging a group: each group's parent,
    then its worktrees, so they travel with it. */
export function orderFromGroups(groups: WsGroup[], top: WsId[]): WsId[] {
  const byId = new Map(groups.map((g) => [g.ws.id, g]));
  return top.flatMap((id) => {
    const g = byId.get(id);
    return g ? [g.ws.id, ...g.kids.map((k) => k.id)] : [];
  });
}

/** The full order after reordering one group's worktrees among themselves. */
export function orderWithKids(groups: WsGroup[], parent: WsId, kids: WsId[]): WsId[] {
  return groups.flatMap((g) => [g.ws.id, ...(g.ws.id === parent ? kids : g.kids.map((k) => k.id))]);
}
