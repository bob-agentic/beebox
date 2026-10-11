/** The links the Share dialog offers: the plain web form Copy puts on the
    clipboard, and the `beebox://` form the QR code carries — the phone's own
    camera hands that to the app, which knows it is a phone and takes the
    terminal's size accordingly, so nothing needs to say so here. */

export interface ShareLink {
  http: string;
  app: string;
}

/** The base a share domain builds. `example.com`, a pasted scheme, a
    trailing slash — all end up as `http://example.com`: the daemon speaks
    plain http and so does the phone app, so whatever was typed, http is the
    scheme the link has to have. Null when nothing is set. */
export function shareBase(host: string): string | null {
  const h = host.trim().replace(/^https?:\/\//, '').replace(/\/+$/, '');
  return h ? `http://${h}` : null;
}

/**
 * Every URL a share path is reachable at.
 *
 * With a share domain set (a tunnel or reverse proxy in front of this
 * machine), that one address replaces the rest: LAN addresses are no use to a
 * phone on another network, and the point of the setting is that the code
 * scans as-is, wherever the phone is. Otherwise this is a dev server's
 * banner — what the daemon enumerated from its interfaces, with this page's
 * own origin first when it is not loopback (that one is known to work: the
 * page came over it).
 */
export function shareLinks(opts: {
  /** The token path the daemon handed back, e.g. `/t/<token>`. */
  path: string;
  /** Base URLs the daemon enumerated from its own interfaces. */
  hosts: string[];
  /** This page's origin. */
  origin: string;
  /** The Share domain setting; empty for the local-address list. */
  shareHost: string;
}): ShareLink[] {
  const custom = shareBase(opts.shareHost);
  let plain: string[];
  if (custom) {
    plain = [`${custom}${opts.path}`];
  } else {
    plain = opts.hosts.map((h) => `${h}${opts.path}`);
    if (!/^https?:\/\/(localhost|127\.)/.test(opts.origin)) {
      const own = `${opts.origin}${opts.path}`;
      if (!plain.includes(own)) plain.unshift(own);
    }
    if (!plain.length) plain = [`${opts.origin}${opts.path}`];
  }
  return plain.map((http) => ({ http, app: http.replace(/^https?:\/\//, 'beebox://') }));
}
