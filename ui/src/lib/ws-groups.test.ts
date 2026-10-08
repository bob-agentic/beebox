import { describe, expect, it } from 'vitest';
import { groupWorkspaces, orderFromGroups, orderWithKids } from './ws-groups';
import type { WorkspaceView } from './proto';

function ws(id: number, repo: string | null, worktree = false): WorkspaceView {
  return { id, name: `w${id}`, path: `/p/${id}`, branch: '', worktree, repo, tabs: [] };
}

const ids = (gs: ReturnType<typeof groupWorkspaces>) => gs.map((g) => [g.ws.id, g.kids.map((k) => k.id)]);

describe('groupWorkspaces', () => {
  it('puts worktrees under their repository, wherever they were opened', () => {
    const list = [ws(1, '/a/.git'), ws(2, null), ws(3, '/a/.git', true), ws(4, '/a/.git', true)];
    expect(ids(groupWorkspaces(list))).toEqual([[1, [3, 4]], [2, []]]);
  });

  it('leaves a worktree on its own while its repository is not open', () => {
    expect(ids(groupWorkspaces([ws(3, '/a/.git', true), ws(2, '/b/.git')]))).toEqual([[3, []], [2, []]]);
  });

  it('never nests a worktree under another worktree', () => {
    const list = [ws(3, '/a/.git', true), ws(4, '/a/.git', true)];
    expect(ids(groupWorkspaces(list))).toEqual([[3, []], [4, []]]);
  });

  it('uses the first checkout when the repository is open twice', () => {
    const list = [ws(1, '/a/.git'), ws(2, '/a/.git'), ws(3, '/a/.git', true)];
    expect(ids(groupWorkspaces(list))).toEqual([[1, [3]], [2, []]]);
  });
});

describe('ordering', () => {
  const groups = groupWorkspaces([ws(1, '/a/.git'), ws(2, null), ws(3, '/a/.git', true), ws(4, '/a/.git', true)]);

  it('carries worktrees with their group', () => {
    expect(orderFromGroups(groups, [2, 1])).toEqual([2, 1, 3, 4]);
  });

  it('reorders worktrees within their group only', () => {
    expect(orderWithKids(groups, 1, [4, 3])).toEqual([1, 4, 3, 2]);
  });
});
