/** Copies text, and says whether it worked.
 *
 *  `navigator.clipboard` needs a secure context. The desktop shell loads from
 *  `http://localhost:<port>`, which counts as one — but a share link opened on
 *  another machine is plain `http://192.168.x.x:<port>`, which does not, and
 *  there the API is simply missing. So the deprecated `execCommand` path is not
 *  legacy baggage: it is the only one that works for a remote viewer.
 *
 *  The check is deliberately synchronous. Awaiting the modern API first and
 *  falling back afterwards would run `execCommand` outside the user gesture
 *  that started it, and browsers reject a copy made there.
 */
export async function writeClipboard(text: string): Promise<boolean> {
  if (navigator.clipboard) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Permission denied, or a context stricter than feature detection can
      // see. Fall through rather than report a copy that never happened.
    }
  }
  return legacyCopy(text);
}

/** Selection-based copy, for contexts without the clipboard API. */
function legacyCopy(text: string): boolean {
  const el = document.createElement('textarea');
  el.value = text;
  // Off-screen rather than hidden: `display:none` cannot hold a selection.
  el.style.cssText = 'position:fixed; top:0; left:-9999px; opacity:0';
  el.setAttribute('readonly', '');
  document.body.appendChild(el);
  try {
    el.select();
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    el.remove();
  }
}
