// The pure parts of searching past conversations: splitting a query, marking
// where it matched, and saying when and where a conversation is.

import type { SearchSession, Shelf } from './proto';

/** Space-separated words, any one of which is a hit — as the daemon reads them. */
export function queryWords(q: string): string[] {
  const out: string[] = [];
  for (const w of q.split(/\s+/)) {
    if (w && !out.some((x) => x.toLowerCase() === w.toLowerCase())) out.push(w);
  }
  return out.slice(0, 8);
}

export type Part = { text: string; hit: boolean };

/** `text` cut where any of `words` occurs, case-insensitively. */
export function markWords(text: string, words: string[]): Part[] {
  if (!words.length) return [{ text, hit: false }];
  const re = new RegExp(words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'), 'gi');
  const parts: Part[] = [];
  let last = 0;
  for (const m of text.matchAll(re)) {
    if (!m[0]) continue;
    if (m.index > last) parts.push({ text: text.slice(last, m.index), hit: false });
    parts.push({ text: m[0], hit: true });
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push({ text: text.slice(last), hit: false });
  return parts;
}

export function hasWord(text: string, words: string[]): boolean {
  const t = text.toLowerCase();
  return words.some((w) => t.includes(w.toLowerCase()));
}

/** `14:05` today, `Yesterday 18:10`, `Oct 9 11:32` this year, else with the year. */
export function when(ms: number, now = new Date()): string {
  const d = new Date(ms);
  const hm = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86_400_000);
  if (days === 0) return hm;
  if (days === 1) return `Yesterday ${hm}`;
  const opts: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' };
  if (d.getFullYear() !== now.getFullYear()) opts.year = 'numeric';
  return `${d.toLocaleDateString('en-US', opts)} ${hm}`;
}

/** `Yesterday 17:58 – 18:10`: the second half drops the day when it is the same. */
export function span(from: number, to: number, now = new Date()): string {
  const a = when(from, now);
  const b = when(to, now);
  if (a === b) return a;
  const [da, db] = [a.split(' ').slice(0, -1).join(' '), b.split(' ').slice(0, -1).join(' ')];
  if (da === db) return `${a} – ${b.split(' ').pop()}`;
  // A bare time reads as the same day as the start; today has to say so.
  return `${a} – ${db ? b : `Today ${b}`}`;
}

export type Where = 'open' | Shelf | 'closed';

export function whereIs(s: Pick<SearchSession, 'tab' | 'shelf'>): Where {
  if (s.tab == null) return 'closed';
  return s.shelf ?? 'open';
}

export const WHERE_LABEL: Record<Where, string> = {
  open: 'Open',
  freezer: 'Freezer',
  archive: 'Archive',
  later: 'Later',
  closed: 'Closed',
};

/** What Enter does with it. */
export function action(w: Where): string {
  if (w === 'open') return 'Switch to tab';
  if (w === 'closed') return 'Resume in new tab';
  return `Bring back from ${WHERE_LABEL[w]}`;
}

/** A shell word that survives any path: single-quoted, quotes inside escaped. */
export function shellQuote(s: string): string {
  return /^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
}

/** What to paste in a terminal to pick the conversation up where it ran. */
export function resumeCommand(cwd: string, session: string): string {
  return cwd ? `cd ${shellQuote(cwd)} && claude -r ${session}` : `claude -r ${session}`;
}

/** `Today`, `Yesterday`, `Oct 9`, or with the year when it is not this one. */
export function dayLabel(ms: number, now = new Date()): string {
  const w = when(ms, now);
  const parts = w.split(' ');
  return parts.length === 1 ? 'Today' : parts.slice(0, -1).join(' ');
}

export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
}

export type ConvItem<M> =
  | { kind: 'day'; key: string; label: string }
  | { kind: 'msg'; key: string; m: M }
  | { kind: 'tools'; key: string; ms: M[] };

/** A conversation as it reads: a day chip where the day changes, and each run
    of tool calls folded into one row between the messages either side. */
export function conversationItems<M extends { key: string; who: string; at_ms: number }>(
  msgs: M[],
  now = new Date(),
): ConvItem<M>[] {
  const out: ConvItem<M>[] = [];
  let day = '';
  for (const m of msgs) {
    const d = m.at_ms ? dayLabel(m.at_ms, now) : day;
    if (d && d !== day) {
      out.push({ kind: 'day', key: `day:${m.key}`, label: d });
      day = d;
    }
    const last = out[out.length - 1];
    if (m.who === 'tool') {
      if (last?.kind === 'tools') last.ms.push(m);
      else out.push({ kind: 'tools', key: `tools:${m.key}`, ms: [m] });
    } else {
      out.push({ kind: 'msg', key: m.key, m });
    }
  }
  return out;
}
