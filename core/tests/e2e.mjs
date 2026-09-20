// End-to-end check against a running daemon: does a real browser client get a
// tree, a terminal, and its own typing echoed back?
//
// Usage: node tests/e2e.mjs [port]

import WebSocket from 'ws';
import { encode, decode } from '@msgpack/msgpack';

const port = process.argv[2] ?? 17788;
const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`);

let tree = null;
let caps = null;
const output = [];
let resyncs = 0;
let grant = null;
// Authoritative pane sizes arrive as their own frames, not in the tree: the
// server owns the size and pushes it. A real client applies these.
const size = new Map();

const send = (msg) => ws.send(encode(msg));

ws.on('open', () => console.log('connected'));

ws.on('message', (raw) => {
  const msg = decode(raw);
  switch (msg.t) {
    case 'tree':
      tree = msg.tree;
      caps = msg.caps;
      break;
    case 'resync':
      resyncs++;
      output.push(Buffer.from(msg.data).toString('utf8'));
      break;
    case 'output':
      output.push(Buffer.from(msg.data).toString('utf8'));
      break;
    case 'size':
      size.set(msg.pane, `${msg.cols}×${msg.rows}`);
      break;
    case 'grant':
      grant = msg;
      break;
  }
});

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const pane = () => tree.workspaces[0].tabs[0].panes[0].id;

async function main() {
  await wait(1200);

  console.log('\n=== 1. tree + caps ===');
  console.log('workspaces:', tree.workspaces.length);
  console.log('name      :', tree.workspaces[0].name);
  console.log('tabs      :', tree.workspaces[0].tabs.length);
  console.log('panes     :', tree.workspaces[0].tabs[0].panes.length);
  console.log('caps      :', JSON.stringify(caps));
  console.log('resyncs   :', resyncs, '(scrollback replayed on attach)');

  console.log('\n=== 2. ping ===');
  const before = Date.now();
  send({ t: 'ping' });
  await wait(300);
  console.log('pong round trip under', Date.now() - before, 'ms');

  console.log('\n=== 3. typing reaches the shell ===');
  output.length = 0;
  send({ t: 'input', pane: pane(), data: Buffer.from('echo BEEBOX_OK\r') });
  await wait(1500);
  const echoed = output.join('');
  console.log('contains BEEBOX_OK:', echoed.includes('BEEBOX_OK'));

  console.log('\n=== 4. utf-8 straight through ===');
  output.length = 0;
  send({ t: 'input', pane: pane(), data: Buffer.from('echo 中文测试 ✓ 你好\r') });
  await wait(1500);
  const cn = output.join('');
  console.log('contains 中文测试 ✓ 你好:', cn.includes('中文测试 ✓ 你好'));

  console.log('\n=== 5. owner resize ===');
  send({ t: 'viewport', pane: pane(), cols: 96, rows: 38 });
  await wait(600);
  console.log('authoritative size pushed:', size.get(pane()));

  console.log('\n=== 6. split, and the tree is re-sent ===');
  const paneCountBefore = tree.workspaces[0].tabs[0].panes.length;
  send({ t: 'split', pane: pane(), dir: 'vertical' });
  await wait(1200);
  console.log('panes', paneCountBefore, '->', tree.workspaces[0].tabs[0].panes.length);
  console.log('layout:', JSON.stringify(tree.workspaces[0].tabs[0].layout));

  console.log('\n=== 7. share grants ===');
  send({ t: 'create_grant', scope: { kind: 'pane', pane: pane() }, writable: false, pairing: false });
  await wait(500);
  console.log('pane  ->', grant.url, 'pair:', grant.pair_code ?? '(none, optional at this scope)');

  send({ t: 'create_grant', scope: { kind: 'all' }, writable: true, pairing: false });
  await wait(500);
  console.log('all   ->', grant.url, 'pair:', grant.pair_code ?? '(none)');
  console.log('pairing forced for the widest scope:', grant.pair_code !== null);

  ws.close();
  process.exit(0);
}

ws.on('error', (e) => {
  console.error('socket error:', e.message);
  process.exit(1);
});

main();
