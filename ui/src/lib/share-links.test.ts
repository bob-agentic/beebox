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
    expect(shareLinks(local).map((l) => l.http)).toEqual(['http://192.168.2.22:17788/t/tok']);
  });

  it('puts a non-loopback origin first', () => {
    const links = shareLinks({ ...local, origin: 'http://192.168.2.9:17788' });
    expect(links.map((l) => l.http)).toEqual([
      'http://192.168.2.9:17788/t/tok',
      'http://192.168.2.22:17788/t/tok',
    ]);
  });

  it('falls back to the origin when the daemon enumerated nothing', () => {
    const links = shareLinks({ ...local, hosts: [] });
    expect(links.map((l) => l.http)).toEqual(['http://localhost:17788/t/tok']);
  });

  it('offers the app form of every web link', () => {
    expect(shareLinks(local)[0].app).toBe('beebox://192.168.2.22:17788/t/tok');
  });

  it('replaces every address with the share domain', () => {
    const links = shareLinks({ ...local, shareHost: 'example.com' });
    expect(links).toEqual([
      {
        http: 'http://example.com/t/tok',
        app: 'beebox://example.com/t/tok',
      },
    ]);
  });

  it('normalizes a domain typed with a scheme or trailing slash', () => {
    const links = shareLinks({ ...local, shareHost: ' https://example.com/ ' });
    expect(links.map((l) => l.http)).toEqual(['http://example.com/t/tok']);
  });

  it('treats whitespace as unset', () => {
    const links = shareLinks({ ...local, shareHost: '   ' });
    expect(links.map((l) => l.http)).toEqual(['http://192.168.2.22:17788/t/tok']);
  });
});
