// Agent status display model: aggregation, tooltips, badges, and the
// per-browser read state. Mirrors core/src/agent.rs where the two overlap;
// the wire view arrives fully sanitized, so nothing here re-validates text.

import type { AgentKind, AgentPhase, AgentStatusView, PaneId } from './proto';

/** Fixed by the spec: most actionable state wins a roll-up. */
export const PHASE_PRIORITY: AgentPhase[] = [
  'needs_input',
  'running',
  'failed',
  'success',
  'idle',
  'never_ran',
];

export function rollup(phases: AgentPhase[]): AgentPhase {
  const present = new Set(phases);
  return PHASE_PRIORITY.find((p) => present.has(p)) ?? 'never_ran';
}

/** Roll-up that also carries unread: among panes sharing the winning phase,
    one unread completion makes the aggregate dot solid. The spec's
    "unread beats read within the same completion state". */
export function rollupWithUnread(
  panes: { id: PaneId; status: AgentStatusView }[],
): { phase: AgentPhase; unread: boolean } {
  const phase = rollup(panes.map((p) => p.status.phase));
  const unread = panes.some(
    (p) => p.status.phase === phase && isUnread(p.id, p.status),
  );
  return { phase, unread };
}

/** UI badge for a pane. Protocol enums only reach the user as these. */
export function agentBadge(agent: AgentKind | null): string {
  switch (agent) {
    case 'claude':
      return 'CC';
    case 'codex':
      return 'CX';
    case 'opencode':
      return 'OC';
    default:
      return 'SH';
  }
}

export function agentName(agent: AgentKind | null): string {
  switch (agent) {
    case 'claude':
      return 'Claude';
    case 'codex':
      return 'Codex';
    case 'opencode':
      return 'OpenCode';
    default:
      return 'Agent';
  }
}

function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  return `${m}m${(s % 60).toString().padStart(2, '0')}s`;
}

/** Tooltip lines for one pane's status. `now` is a parameter so tests do not
    race the clock. */
export function tooltip(v: AgentStatusView, now: number): string[] {
  switch (v.phase) {
    case 'running': {
      const lines = [
        v.started_at_ms !== null
          ? `Running for ${fmtDuration(now - v.started_at_ms)}`
          : 'Running',
      ];
      if (v.tool_detail) lines.push(v.tool_detail);
      return lines;
    }
    case 'needs_input':
      return [`Needs input (${fmtDuration(now - v.at_ms)} ago)`];
    case 'success':
    case 'failed': {
      const who = agentName(v.agent);
      const took =
        v.started_at_ms !== null ? ` · ${fmtDuration(v.at_ms - v.started_at_ms)}` : '';
      const head =
        v.phase === 'success'
          ? `${who}: turn finished${took}`
          : `${who}: turn had tool errors${took}`;
      return v.summary ? [head, v.summary] : [head];
    }
    case 'idle':
      return [`${agentName(v.agent)}: waiting for a prompt`];
    default:
      return [];
  }
}

// ---- per-browser read state ------------------------------------------------
//
// Read/unread is client-local by design: BeeBox has many browsers on one
// daemon, and looking at a result on the desktop must not mark it read on the
// phone. The stored value is the last *seen* revision per pane; a completion
// is unread while its revision is newer than that.

const READ_KEY = 'beebox.agent-status-read.v1';

type ReadMap = Record<string, number>;

function load(): ReadMap {
  try {
    return JSON.parse(localStorage.getItem(READ_KEY) ?? '{}') as ReadMap;
  } catch {
    return {};
  }
}

export function isUnread(pane: PaneId, v: AgentStatusView): boolean {
  if (v.phase !== 'success' && v.phase !== 'failed') return false;
  return (load()[String(pane)] ?? -1) < v.revision;
}

/** Records the completion as seen. Returns true only when something actually
    changed — callers use this to avoid reactive feedback loops. */
export function markRead(pane: PaneId, v: AgentStatusView): boolean {
  const m = load();
  if ((m[String(pane)] ?? -1) >= v.revision) return false;
  m[String(pane)] = v.revision;
  try {
    localStorage.setItem(READ_KEY, JSON.stringify(m));
  } catch {
    // Storage full or denied; unread state degrades gracefully.
  }
  return true;
}

/** Drops read entries for panes that no longer exist, so the map cannot grow
    without bound. Called on every tree update. */
export function pruneRead(livePanes: Set<PaneId>): void {
  const m = load();
  let changed = false;
  for (const k of Object.keys(m)) {
    if (!livePanes.has(Number(k))) {
      delete m[k];
      changed = true;
    }
  }
  if (changed) {
    try {
      localStorage.setItem(READ_KEY, JSON.stringify(m));
    } catch {
      // Best effort.
    }
  }
}
