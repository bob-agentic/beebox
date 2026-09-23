import { describe, expect, it } from 'vitest';
import { stripPartialLineMarkers } from './partial-line';

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

describe('stripPartialLineMarkers', () => {
  it('drops the marker and the padding that was meant to erase it', () => {
    // What zsh emits after output with no trailing newline: reverse video, a
    // percent sign, the attributes turned back off, then spaces to the end of
    // its own line. On a narrower terminal those spaces wrap rather than
    // erase, which is what leaves the % on screen.
    const raw = `done[7m%[27m[1m[0m${' '.repeat(72)}\r\nnext`;
    expect(dec(stripPartialLineMarkers(enc(raw)))).toBe('done\r\nnext');
  });

  it('leaves a percent sign that is part of the output', () => {
    const raw = 'disk 93% full\r\n50% done\r\n';
    expect(dec(stripPartialLineMarkers(enc(raw)))).toBe(raw);
  });

  it('leaves reverse video that is not the marker', () => {
    // Reverse video is how plenty of programs draw a status line; only the
    // exact marker sequence should go.
    const raw = '[7m STATUS [27m ready';
    expect(dec(stripPartialLineMarkers(enc(raw)))).toBe(raw);
  });

  it('handles several markers in one replay', () => {
    const one = `[7m%[27m[0m${' '.repeat(40)}`;
    expect(dec(stripPartialLineMarkers(enc(`a${one}b${one}c`)))).toBe('abc');
  });

  it('returns the input untouched when there is no marker', () => {
    const raw = enc('ordinary output\r\n');
    expect(stripPartialLineMarkers(raw)).toBe(raw);
  });
});
