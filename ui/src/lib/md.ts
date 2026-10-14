// Markdown for reading past conversations. What Claude writes is markdown;
// what people type is mostly plain text, which renders the same.

import DOMPurify from 'dompurify';
import { Marked } from 'marked';

const escape = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

const md = new Marked({
  gfm: true,
  // A line break typed is a line break meant.
  breaks: true,
  renderer: {
    // `<Toaster />` written outside backticks is prose, not markup: escaped,
    // it reads as written instead of being sanitised away.
    html: ({ text }) => escape(text),
  },
});

/** `text` as sanitised HTML, with any of `words` marked. Links become plain
    spans — following one would navigate the app away — and images are
    dropped rather than fetched. */
export function renderMd(text: string, words: string[]): string {
  const html = DOMPurify.sanitize(md.parse(text) as string, {
    FORBID_TAGS: ['img', 'style', 'form', 'input', 'button', 'iframe', 'svg', 'math'],
    FORBID_ATTR: ['style'],
  });
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  for (const a of tpl.content.querySelectorAll('a')) {
    const span = document.createElement('span');
    span.className = 'md-link';
    span.title = a.getAttribute('href') ?? '';
    span.append(...a.childNodes);
    a.replaceWith(span);
  }
  if (words.length) markText(tpl.content, words);
  return tpl.innerHTML;
}

function markText(root: DocumentFragment, words: string[]) {
  const re = new RegExp(words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'), 'gi');
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const texts: Text[] = [];
  while (walker.nextNode()) texts.push(walker.currentNode as Text);
  for (const t of texts) {
    const s = t.data;
    re.lastIndex = 0;
    if (!re.test(s)) continue;
    re.lastIndex = 0;
    const frag = document.createDocumentFragment();
    let last = 0;
    for (const m of s.matchAll(re)) {
      if (!m[0]) continue;
      if (m.index > last) frag.append(s.slice(last, m.index));
      const mark = document.createElement('mark');
      mark.textContent = m[0];
      frag.append(mark);
      last = m.index + m[0].length;
    }
    if (last < s.length) frag.append(s.slice(last));
    t.replaceWith(frag);
  }
}
