/** Removes zsh's partial-line marker from replayed history.
 *
 *  When a command's output does not end in a newline, zsh prints a
 *  reverse-video `%` and then erases it by padding with spaces to the end of
 *  the line — a count fixed at the width of the terminal that produced it. On
 *  a narrower one the padding wraps instead of erasing, and the `%` is left
 *  sitting at the top of the screen. It cannot be fixed by resizing first:
 *  the byte count was decided when the bytes were written.
 *
 *  The marker means nothing to a terminal replaying someone else's history,
 *  so it goes, padding and all. Matched tightly — reverse video, one percent
 *  sign, the attribute resets that follow it, then the run of spaces — so
 *  that a `%` in real output is untouched.
 */
export function stripPartialLineMarkers(data: Uint8Array): Uint8Array {
  const text = new TextDecoder().decode(data);
  // eslint-disable-next-line no-control-regex
  const marker = /\u001b\[7m%(?:\u001b\[[0-9;]*m)* +/g;
  if (!marker.test(text)) return data;
  marker.lastIndex = 0;
  return new TextEncoder().encode(text.replace(marker, ''));
}

