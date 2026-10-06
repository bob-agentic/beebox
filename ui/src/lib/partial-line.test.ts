import { describe, expect, it } from 'vitest';
import { fixPartialLineMarkers, type LineState } from './partial-line';

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);
const fix = (s: string, cols: number, line: LineState = { text: false }) =>
  dec(fixPartialLineMarkers(enc(s), cols, line));

// Exactly what zsh 5.9 writes after a command, captured at 40 columns: the
// marker, `cols - 1` spaces, then the return and space that wipe it.
const marker = (cols: number) => `\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m${' '.repeat(cols - 1)}\r \r`;
const KEPT = '\x1b[7m%\x1b[27m\r\n';

describe('fixPartialLineMarkers', () => {
  it('leaves a marker at the start of a line alone at its own width', () => {
    const raw = `last line\r\n${marker(40)}\rP> `;
    expect(fix(raw, 40)).toBe(raw);
  });

  it('drops a marker from another width at the start of a line', () => {
    // On a phone this used to wrap and leave a % at the top of the screen.
    expect(fix(`done\r\n${marker(73)}\rnext`, 46)).toBe('done\r\n\r \r\rnext');
  });

  it('keeps the last line, its %, and a new line after output with no final newline', () => {
    // `cat` of a file without one. The last line must survive, with zsh's %
    // after it and the prompt below it — at any width, and without the spaces
    // that would reflow into blank lines when the terminal narrows.
    expect(fix(`first\r\nlast line${marker(120)}\rP> `, 80)).toBe(`first\r\nlast line${KEPT}\r \r\rP> `);
    expect(fix(`first\r\nlast line${marker(80)}\rP> `, 80)).toBe(`first\r\nlast line${KEPT}\r \r\rP> `);
  });

  it('remembers across chunks that the line has text on it', () => {
    // The file's last line and zsh's marker are two writes, and usually two
    // chunks of output.
    const line: LineState = { text: false };
    expect(fix('first\r\nlast line', 80, line)).toBe('first\r\nlast line');
    expect(line.text).toBe(true);
    expect(fix(`${marker(80)}\rP> `, 80, line)).toBe(`${KEPT}\r \r\rP> `);
    expect(line.text).toBe(true); // the prompt
  });

  it('remembers across chunks that the line is empty', () => {
    const line: LineState = { text: false };
    fix('out\r\n', 80, line);
    expect(line.text).toBe(false);
    expect(fix(marker(80), 80, line)).toBe(marker(80));
  });

  it('does not guess after the cursor was moved by an escape sequence', () => {
    // A full-screen program leaves the cursor wherever it last put it.
    const line: LineState = { text: false };
    fix('\x1b[24;1H', 80, line);
    expect(line.text).toBeNull();
    expect(fix(marker(80), 80, { ...line })).toBe(marker(80));
    expect(fix(`${marker(120)}x`, 80, { ...line })).toBe('\r \rx');
  });

  it('follows the width marks in the stream', () => {
    // Written at 40, replayed into 80: the terminal is resized to 40 before
    // it draws that stretch, so the marker there is at its own width.
    const raw = `\x1b]7788;40\x07out\r\n${marker(40)}\x1b]7788;80\x07more\r\n${marker(40)}`;
    expect(fix(raw, 80)).toBe(`\x1b]7788;40\x07out\r\n${marker(40)}\x1b]7788;80\x07more\r\n\r \r`);
  });

  it('leaves padding cut off by the end of a chunk alone', () => {
    // Spaces running to the end may continue in the next chunk, so their
    // count says nothing yet.
    const raw = `out\r\n\x1b[7m%\x1b[27m\x1b[0m${' '.repeat(10)}`;
    expect(fix(raw, 40)).toBe(raw);
  });

  it('does not count escape sequences as text on the line', () => {
    expect(fix(`out\r\n\x1b[0m\x1b[K${marker(73)}`, 46)).toBe('out\r\n\x1b[0m\x1b[K\r \r');
  });

  it('does not count private-mode sequences or controls as text', () => {
    // `ESC[>4;1m` sets a keyboard mode; BEL and backspace print nothing.
    expect(fix(`out\r\n\x1b[>4;1m\x07\x08${marker(73)}`, 46)).toBe('out\r\n\x1b[>4;1m\x07\x08\r \r');
  });

  it('does not guess after a reverse index', () => {
    const line: LineState = { text: false };
    fix('out\r\n\x1bM', 80, line);
    expect(line.text).toBeNull();
  });

  it('leaves a chunk alone that a character was cut in two at', () => {
    // The first two of the three bytes of 中: decoded alone they are U+FFFD,
    // and re-encoding would write that instead of the character.
    const bytes = new Uint8Array([...enc(`out${marker(120)}`), 0xe4, 0xb8]);
    expect(fixPartialLineMarkers(bytes, 80, { text: false })).toBe(bytes);
  });

  it('leaves a percent sign that is part of the output', () => {
    const raw = 'disk 93% full\r\n50% done\r\n';
    expect(fix(raw, 40)).toBe(raw);
  });

  it('leaves reverse video that is not the marker', () => {
    const raw = '\x1b[7m STATUS \x1b[27m ready';
    expect(fix(raw, 40)).toBe(raw);
  });

  it('returns the input itself when there is nothing to fix', () => {
    const raw = enc(`ordinary output\r\n${marker(40)}`);
    expect(fixPartialLineMarkers(raw, 40, { text: false })).toBe(raw);
  });
});
