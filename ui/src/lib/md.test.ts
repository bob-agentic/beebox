// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { renderMd } from './md';

describe('renderMd', () => {
  it('renders markdown and marks the words outside tags', () => {
    const html = renderMd('**改了** `overflow-tooltip.tsx`\n- one Tooltip', ['tooltip']);
    expect(html).toContain('<strong>改了</strong>');
    expect(html).toContain('<code>overflow-<mark>tooltip</mark>.tsx</code>');
    expect(html).toContain('one <mark>Tooltip</mark>');
  });

  it('keeps bare angle brackets as text and runs no script', () => {
    const html = renderMd('挂在 <Toaster /> 旁边 <script>alert(1)</script><img src=x onerror=alert(1)>', []);
    expect(html).toContain('&lt;Toaster /&gt;');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('<img');
  });

  it('turns links into plain spans and keeps line breaks', () => {
    const html = renderMd('[docs](https://example.com)\nnext line', []);
    expect(html).not.toContain('<a');
    expect(html).toContain('<span class="md-link" title="https://example.com">docs</span>');
    expect(html).toContain('<br>');
  });
});
