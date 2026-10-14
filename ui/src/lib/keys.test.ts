import { describe, expect, it } from 'vitest';
import { otherCommand } from './keys';

const k = (key: string, mods: Partial<Record<'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey', boolean>> = {}) => ({
  key,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe('shortcuts off a Mac', () => {
  it('maps the four Ctrl+Shift chords and Ctrl+,', () => {
    expect(otherCommand(k('F', { ctrlKey: true, shiftKey: true }))).toBe('search_conversations');
    expect(otherCommand(k('D', { ctrlKey: true, shiftKey: true }))).toBe('split_v');
    expect(otherCommand(k('{', { ctrlKey: true, shiftKey: true }))).toBe('prev_tab');
    expect(otherCommand(k('}', { ctrlKey: true, shiftKey: true }))).toBe('next_tab');
    expect(otherCommand(k(',', { ctrlKey: true }))).toBe('settings');
  });

  it('leaves the terminal its own keys', () => {
    for (const key of ['c', 'd', 'w', 'f']) expect(otherCommand(k(key, { ctrlKey: true }))).toBeNull();
    expect(otherCommand(k('T', { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(otherCommand(k('F', { metaKey: true, shiftKey: true }))).toBeNull();
  });
});
