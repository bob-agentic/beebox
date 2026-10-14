import { describe, expect, it } from 'vitest';
import { conversationItems, markWords, queryWords, resumeCommand, span, when, whereIs, action } from './conv-search';

describe('conversation search', () => {
  it('splits a query into distinct words', () => {
    expect(queryWords('  Tooltip tooltip  悬停 ')).toEqual(['Tooltip', '悬停']);
  });

  it('marks every word, whatever its case', () => {
    expect(markWords('Use tooltips, not HOVER', ['tooltip', 'hover'])).toEqual([
      { text: 'Use ', hit: false },
      { text: 'tooltip', hit: true },
      { text: 's, not ', hit: false },
      { text: 'HOVER', hit: true },
    ]);
    expect(markWords('a.b', ['.'])).toEqual([
      { text: 'a', hit: false },
      { text: '.', hit: true },
      { text: 'b', hit: false },
    ]);
  });

  it('says when, relative to today', () => {
    const now = new Date(2026, 9, 14, 16, 0);
    expect(when(new Date(2026, 9, 14, 14, 5).getTime(), now)).toBe('14:05');
    expect(when(new Date(2026, 9, 13, 18, 10).getTime(), now)).toBe('Yesterday 18:10');
    expect(when(new Date(2026, 9, 9, 11, 32).getTime(), now)).toBe('Oct 9 11:32');
    expect(span(new Date(2026, 9, 13, 17, 58).getTime(), new Date(2026, 9, 13, 18, 10).getTime(), now)).toBe(
      'Yesterday 17:58 – 18:10',
    );
    expect(span(new Date(2026, 9, 12, 23, 42).getTime(), new Date(2026, 9, 14, 15, 53).getTime(), now)).toBe(
      'Oct 12 23:42 – Today 15:53',
    );
  });

  it('says where a conversation is and what opening it does', () => {
    expect(whereIs({ tab: null, shelf: null })).toBe('closed');
    expect(whereIs({ tab: 3, shelf: null })).toBe('open');
    expect(action(whereIs({ tab: 3, shelf: 'freezer' }))).toBe('Bring back from Freezer');
    expect(action('closed')).toBe('Resume in new tab');
  });

  it('builds a resume command that survives any folder name', () => {
    expect(resumeCommand('/Users/me/repo/app', 'abc-1')).toBe('cd /Users/me/repo/app && claude -r abc-1');
    expect(resumeCommand("/tmp/it's here", 'x')).toBe("cd '/tmp/it'\\''s here' && claude -r x");
  });

  it('folds runs of tool calls and marks where the day changes', () => {
    const now = new Date(2026, 9, 14, 16, 0);
    const at = (d: number, h: number) => new Date(2026, 9, d, h).getTime();
    const items = conversationItems(
      [
        { key: 'u1', who: 'user', at_ms: at(13, 17) },
        { key: 't1', who: 'tool', at_ms: at(13, 17) },
        { key: 't2', who: 'tool', at_ms: at(13, 17) },
        { key: 'a1', who: 'ai', at_ms: at(13, 18) },
        { key: 'u2', who: 'user', at_ms: at(14, 9) },
      ],
      now,
    );
    expect(items.map((i) => (i.kind === 'tools' ? `tools(${i.ms.length})` : i.kind === 'day' ? i.label : i.key))).toEqual([
      'Yesterday',
      'u1',
      'tools(2)',
      'a1',
      'Today',
      'u2',
    ]);
  });
});
