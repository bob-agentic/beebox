// Where the messages you sent to Claude Code or Codex are in a terminal's
// history, so ⌘↑/⌘↓ can step between them.
//
// Both draw a message you sent as a line that opens with their prompt mark and
// a space. Colours are no help in telling it apart: they follow each tool's
// theme and the terminal's. What does:
//
// - Claude Code (❯) paints a background behind a sent message, in every one of
//   its themes, and leaves the terminal's default behind anything else that
//   starts with ❯ — its menus' pointer. Its input box follows ❯ with a
//   no-break space, not a space.
// - Codex (›) draws its input box exactly like a sent message. The cursor is
//   in the box, so the block of lines holding the cursor is left out. Codex
//   only paints a background when the terminal answers its colour query,
//   which a pane nobody has on screen does not — so the background is not
//   required of it.

import type { IBuffer, IBufferCell } from '@xterm/xterm';

/** Buffer rows where a sent message starts, top to bottom. */
export function sentRows(buf: IBuffer): number[] {
  const cursor = buf.baseY + buf.cursorY;
  let box = cursor;
  while (box > 0 && hasText(buf, box - 1)) box--;

  const rows: number[] = [];
  let cell: IBufferCell | undefined;
  for (let y = 0; y < buf.length; y++) {
    const line = buf.getLine(y);
    if (!line || line.isWrapped) continue;
    cell = line.getCell(0, cell);
    if (!cell) continue;
    const mark = cell.getChars();
    if (mark !== '❯' && mark !== '›') continue;
    // Read now: the next getCell reuses `cell`.
    const painted = !cell.isBgDefault();
    if (line.getCell(1, cell)?.getChars() !== ' ') continue;
    if (mark === '❯' ? painted : y < box) rows.push(y);
  }
  return rows;
}

function hasText(buf: IBuffer, y: number): boolean {
  return (buf.getLine(y)?.translateToString(true).trim() ?? '') !== '';
}
