import { describe, expect, it } from 'vitest';
import type { IBuffer } from '@xterm/xterm';
import { findImageMessage, findImageTags } from './image-links';

/** A buffer of plain rows. A row starting `~` is painted (how Claude draws a
 *  sent message); `+` marks a row wrapped from the one before. */
function buffer(rows: string[]): IBuffer {
  const lines = rows.map((r) => {
    const painted = r.startsWith('~');
    const wrapped = r.startsWith('+');
    const text = painted || wrapped ? r.slice(1) : r;
    return {
      isWrapped: wrapped,
      translateToString: () => text,
      getCell: (x: number) => ({ getChars: () => text[x] ?? '', isBgDefault: () => !painted }),
    };
  });
  return {
    length: lines.length,
    baseY: 0,
    cursorY: lines.length - 1,
    getLine: (y: number) => lines[y],
  } as unknown as IBuffer;
}

describe('findImageTags', () => {
  it('finds each tag and its number', () => {
    expect(findImageTags('❯ [Image #3] and [Image #12]').map((t) => t.n)).toEqual([3, 12]);
  });
});

describe('findImageMessage', () => {
  it('stops at the agent’s own lines under the prompt', () => {
    const buf = buffer([
      '~❯ [Image #1] 看图说话',
      '~  ⎿  [Image #1]',
      '  ⎿  UserPromptSubmit says: Remember plugin hooks were not',
      '     registered for that session; run /remember:doctor.',
      '',
      '● 这张截图展示的是 Client Inquiry',
      '❯ ',
    ]);
    expect(findImageMessage(buf, 1, '  ⎿  [Image #1]')).toEqual({ row: 0, text: '看图说话' });
  });

  it('joins a wrapped question, and takes the latest message for Claude', () => {
    const buf = buffer([
      '~❯ [Image #2] first',
      '~  ⎿  [Image #2]',
      '',
      '~❯ 不是，现在哪有你说的按钮？ 你刚刚说',
      '+的那个入口 [Image #2]',
      '~  ⎿  [Image #2]',
      '',
      '❯ ',
    ]);
    expect(findImageMessage(buf, 2, '')).toEqual({ row: 3, text: '不是，现在哪有你说的按钮？ 你刚刚说的那个入口' });
  });

  it('picks the Codex message the clicked row belongs to', () => {
    const buf = buffer([
      '› [Image #1] 第一条',
      '',
      '› [Image #1] 第二条',
      '',
      '› ',
    ]);
    expect(findImageMessage(buf, 1, '› [Image #1] 第一条')?.text).toBe('第一条');
    expect(findImageMessage(buf, 1, '')?.text).toBe('第二条');
  });

  it('is null once the message has left the history', () => {
    expect(findImageMessage(buffer(['● output', '❯ ']), 4, '')).toBeNull();
  });
});
