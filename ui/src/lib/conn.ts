// WebSocket transport. MessagePack in both directions, automatic reconnect,
// and a heartbeat — a phone's socket over a VPN dies silently, and a peer list
// that lies is worse than no peer list.

import { decode, encode } from '@msgpack/msgpack';
import type { In, Out } from './proto';

const PING_INTERVAL = 20_000;
const RECONNECT_MIN = 500;
const RECONNECT_MAX = 5_000;

export class Conn {
  private ws: WebSocket | null = null;
  private ping: ReturnType<typeof setInterval> | null = null;
  private backoff = RECONNECT_MIN;
  private closed = false;
  /** Set when the server turns us away. Reconnecting then would only be
      turned away again. */
  private refused = false;
  /** True once any connection succeeded. A socket that has *never* opened is
      either a daemon still starting or credentials refused before the
      upgrade — retry a few times for the first, then stop instead of
      hammering the server with the same wrong credentials forever. */
  private everOpened = false;
  private coldTries = 0;

  constructor(
    private url: string,
    private onFrame: (msg: Out) => void,
    private onStatus: (up: boolean) => void,
  ) {
    this.open();
  }

  private open() {
    const ws = new WebSocket(this.url);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;

    ws.onopen = () => {
      this.everOpened = true;
      this.backoff = RECONNECT_MIN;
      this.onStatus(true);
      this.ping = setInterval(() => this.send({ t: 'ping' }), PING_INTERVAL);
    };

    ws.onmessage = (ev) => {
      if (!(ev.data instanceof ArrayBuffer)) return;
      const msg = decode(new Uint8Array(ev.data)) as Out;
      if (msg.t === 'closed') this.refused = true;
      this.onFrame(msg);
    };

    ws.onclose = () => {
      if (this.ping) clearInterval(this.ping);
      this.ping = null;
      this.onStatus(false);
      if (this.closed || this.refused) return;
      if (!this.everOpened && ++this.coldTries > 3) return;
      // Backoff, because a daemon that is down stays down for a while.
      setTimeout(() => this.open(), this.backoff);
      this.backoff = Math.min(this.backoff * 2, RECONNECT_MAX);
    };

    ws.onerror = () => ws.close();
  }

  send(msg: In) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(encode(msg));
    }
  }

  dispose() {
    this.closed = true;
    this.ws?.close();
  }
}
