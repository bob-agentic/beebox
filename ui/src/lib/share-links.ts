/** The links the Share dialog offers: `web` is the form Copy puts on the
    clipboard, `qr` is what the code carries. An https link serves as both —
    any camera and the app's own scanner take it as-is, so the http hop that
    a TLS tunnel answers with a 301 (and the app's WebView used to hand to
    the system browser) never occurs. An http link keeps the `beebox://`
    form in the code, which the phone's own camera hands straight to the app. */

export interface ShareLink {
  web: string;
  qr: string;
}

/** The base a share domain builds. An explicit http:// or https:// is kept
    as written — the scheme belongs to the tunnel, not the daemon. A bare
    domain means https, which is what a public tunnel speaks; write http://
    out for one that does not. A trailing slash is dropped; null when
    nothing is set. */
export function shareBase(host: string): string | null {
  const h = host.trim().replace(/\/+$/, '');
  if (!h) return null;
  return /^https?:\/\//.test(h) ? h : `https://${h}`;
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
  return plain.map((web) => ({
    web,
    // https goes into the code as itself; http keeps the beebox:// form.
    qr: web.startsWith('https://') ? web : web.replace(/^https?:\/\//, 'beebox://'),
  }));
}
