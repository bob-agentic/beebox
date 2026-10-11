import { describe, expect, it } from 'vitest';
import { shareLinks } from './share-links';

const local = {
  path: '/t/tok',
  hosts: ['http://192.168.2.22:17788'],
  origin: 'http://localhost:17788',
  shareHost: '',
};

describe('shareLinks', () => {
  it('lists the daemon hosts, adding the origin only when it is not loopback', () => {
    expect(shareLinks(local).map((l) => l.web)).toEqual(['http://192.168.2.22:17788/t/tok']);
  });

  it('puts a non-loopback origin first', () => {
    const links = shareLinks({ ...local, origin: 'http://192.168.2.9:17788' });
    expect(links.map((l) => l.web)).toEqual([
      'http://192.168.2.9:17788/t/tok',
      'http://192.168.2.22:17788/t/tok',
    ]);
  });

  it('falls back to the origin when the daemon enumerated nothing', () => {
    const links = shareLinks({ ...local, hosts: [] });
    expect(links.map((l) => l.web)).toEqual(['http://localhost:17788/t/tok']);
  });

  it('wraps an http link in beebox:// for the code', () => {
    expect(shareLinks(local)[0].qr).toBe('beebox://192.168.2.22:17788/t/tok');
  });

  it('replaces every address with the share domain, https by default', () => {
    const links = shareLinks({ ...local, shareHost: 'example.com' });
    expect(links).toEqual([{ web: 'https://example.com/t/tok', qr: 'https://example.com/t/tok' }]);
  });

  it('keeps the scheme a domain is written with', () => {
    const https = shareLinks({ ...local, shareHost: ' https://example.com/ ' });
    expect(https).toEqual([{ web: 'https://example.com/t/tok', qr: 'https://example.com/t/tok' }]);

    const http = shareLinks({ ...local, shareHost: 'http://example.com' });
    expect(http).toEqual([{ web: 'http://example.com/t/tok', qr: 'beebox://example.com/t/tok' }]);
  });

  it('treats whitespace as unset', () => {
    const links = shareLinks({ ...local, shareHost: '   ' });
    expect(links.map((l) => l.web)).toEqual(['http://192.168.2.22:17788/t/tok']);
  });
});
