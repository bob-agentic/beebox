// Mirrors the Rust-side rules where the two overlap (rollup priority), and
// covers what only exists client-side: tooltips and per-browser read state.

import { beforeEach, describe, expect, it } from 'vitest';
import {
  agentBadge,
  isUnread,
  markRead,
  pruneRead,
  rollup,
  rollupWithUnread,
  tooltip,
} from './agent-status';
import type { AgentStatusView } from './proto';

function view(over: Partial<AgentStatusView> = {}): AgentStatusView {
  return {
    phase: 'success',
    agent: 'claude',
    revision: 1,
    at_ms: 100_000,
    started_at_ms: 28_000,
    tool_detail: null,
    summary: null,
    ...over,
  };
}

describe('rollup', () => {
  it('follows the fixed priority', () => {
    expect(rollup(['idle', 'success', 'needs_input', 'running'])).toBe('needs_input');
    expect(rollup(['idle', 'success', 'running'])).toBe('running');
    expect(rollup(['idle', 'success', 'failed'])).toBe('failed');
    expect(rollup(['idle', 'success'])).toBe('success');
    expect(rollup(['never_ran', 'idle'])).toBe('idle');
    expect(rollup([])).toBe('never_ran');
  });
});

describe('badges', () => {
  it('maps protocol enums to UI labels', () => {
    expect(agentBadge('claude')).toBe('CC');
    expect(agentBadge('codex')).toBe('CX');
    expect(agentBadge('opencode')).toBe('OC');
    expect(agentBadge(null)).toBe('SH');
  });
});

describe('tooltip', () => {
  it('shows elapsed time and tool while running', () => {
    const lines = tooltip(
      view({ phase: 'running', started_at_ms: 82_000, tool_detail: 'Edit src/auth.rs' }),
      100_000,
    );
    expect(lines).toEqual(['Running for 18s', 'Edit src/auth.rs']);
  });

  it('shows how long input has been needed', () => {
    expect(tooltip(view({ phase: 'needs_input', at_ms: 96_000 }), 100_000)).toEqual([
      'Needs input (4s ago)',
    ]);
  });

  it('shows duration and summary for a finished turn', () => {
    const lines = tooltip(
      view({ at_ms: 100_000, started_at_ms: 28_000, summary: 'Refactored the auth middleware.' }),
      200_000,
    );
    expect(lines).toEqual([
      'Claude: turn finished · 1m12s',
      'Refactored the auth middleware.',
    ]);
  });

  it('marks failed turns and other agents', () => {
    const lines = tooltip(
      view({ phase: 'failed', agent: 'codex', at_ms: 100_000, started_at_ms: 78_000 }),
      100_000,
    );
    expect(lines).toEqual(['Codex: turn had tool errors · 22s']);
  });

  it('is empty for never_ran', () => {
    expect(tooltip(view({ phase: 'never_ran' }), 0)).toEqual([]);
  });
});

describe('read state', () => {
  beforeEach(() => localStorage.clear());

  it('a completion is unread until marked, per revision', () => {
    const v = view({ revision: 5 });
    expect(isUnread(1, v)).toBe(true);
    markRead(1, v);
    expect(isUnread(1, v)).toBe(false);
    // The next turn's completion is unread again.
    expect(isUnread(1, view({ revision: 6 }))).toBe(true);
  });

  it('only completions can be unread', () => {
    expect(isUnread(1, view({ phase: 'running' }))).toBe(false);
    expect(isUnread(1, view({ phase: 'idle' }))).toBe(false);
    expect(isUnread(1, view({ phase: 'never_ran' }))).toBe(false);
  });

  it('pruning drops panes that no longer exist', () => {
    markRead(1, view());
    markRead(2, view());
    pruneRead(new Set([2]));
    const stored = JSON.parse(localStorage.getItem('beebox.agent-status-read.v1')!);
    expect(Object.keys(stored)).toEqual(['2']);
  });

  it('an unread completion keeps the aggregate dot solid', () => {
    // Two panes, both success; one read, one not: the tab dot stays unread.
    const a = { id: 1, status: view({ revision: 3 }) };
    const b = { id: 2, status: view({ revision: 5 }) };
    markRead(1, a.status);
    expect(rollupWithUnread([a, b])).toEqual({ phase: 'success', unread: true });

    markRead(2, b.status);
    expect(rollupWithUnread([a, b])).toEqual({ phase: 'success', unread: false });
  });

  it('unread only counts panes in the winning phase', () => {
    // An unread success must not make a *failed* aggregate look unread.
    const ok = { id: 1, status: view({ revision: 2 }) };
    const bad = { id: 2, status: view({ phase: 'failed' as const, revision: 4 }) };
    markRead(2, bad.status);
    expect(rollupWithUnread([ok, bad])).toEqual({ phase: 'failed', unread: false });
  });
});
