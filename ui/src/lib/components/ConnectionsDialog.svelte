<script lang="ts">
  // Replaces link expiry: every device a link was opened on stays listed,
  // connected or not, until it is disconnected here. Only then does it lose
  // its way back in.
  import { store } from '../state.svelte';

  let { onclose }: { onclose: () => void } = $props();

  function ago(at: number): string {
    const secs = Math.max(0, Math.floor(Date.now() / 1000) - at);
    if (secs < 60) return 'just now';
    if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
    if (secs < 86400) return `${Math.floor(secs / 3600)} h ago`;
    return `${Math.floor(secs / 86400)} d ago`;
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal">
    <h3>Shared devices</h3>
    <div class="sub">No expiry — a device keeps its link until you disconnect it</div>

    <div class="conn-list">
      {#each store.peers as peer (peer.token)}
        <div class="conn">
          <span class="dot" class:on={peer.addr !== null}></span>
          <div class="who">
            <b>{peer.device}</b>
            <small>{peer.addr ?? 'offline'} · {peer.scope} · paired {ago(peer.paired_at)}</small>
          </div>
          <span class="badge" class:rw={peer.writable} class:ro={!peer.writable}>
            {peer.writable ? 'can type' : 'read-only'}
          </span>
          <button class="disconnect" onclick={() => store.send({ t: 'revoke', token: peer.token })}>
            Disconnect
          </button>
        </div>
      {:else}
        <div class="empty">No shared devices</div>
      {/each}
    </div>

    <div class="acts">
      <button class="g" onclick={onclose}>Close</button>
      {#if store.peers.length > 1}
        <button class="danger" onclick={() => store.send({ t: 'revoke_all' })}>
          Disconnect all {store.peers.length}
        </button>
      {/if}
    </div>
  </div>
</div>

<style>
  .mask {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }
  .modal {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 13px;
    width: 520px;
    padding: 18px;
  }
  h3 {
    font-size: 14px;
    margin-bottom: 4px;
  }
  .sub {
    font-size: 11.5px;
    color: var(--faint);
    margin-bottom: 14px;
  }
  .conn-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 14px 0;
  }
  .conn {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 11px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--faint);
    flex: 0 0 auto;
  }
  .dot.on {
    background: #86efac;
  }
  .who {
    flex: 1;
    min-width: 0;
  }
  .who b {
    font-size: 12px;
    font-weight: 600;
    display: block;
  }
  .who small {
    font-size: 10px;
    color: var(--faint);
    display: block;
    font-family: ui-monospace, monospace;
    margin-top: 1px;
  }
  .badge {
    font-size: 9px;
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 600;
    flex: 0 0 auto;
  }
  .badge.ro {
    background: #1e2a3d;
    color: #7dd3fc;
  }
  .badge.rw {
    background: #3d2416;
    color: #fbbf24;
  }
  .disconnect {
    font-size: 10.5px;
    padding: 4px 11px;
    border-radius: 6px;
    background: #3d1f1f;
    color: #f87171;
    flex: 0 0 auto;
  }
  .disconnect:hover {
    background: #5c2020;
    color: #fff;
  }
  .empty {
    font-size: 11.5px;
    color: var(--faint);
    padding: 14px;
    text-align: center;
  }
  .acts {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .acts button {
    padding: 7px 15px;
    border-radius: 7px;
    font-size: 12px;
  }
  .acts .g {
    background: var(--panel-2);
    color: var(--dim);
  }
  .acts .danger {
    background: #3d1f1f;
    color: #f87171;
    font-weight: 600;
  }

  @media (max-width: 560px) {
    .modal { width: calc(100vw - 24px); }
    .conn { align-items: flex-start; flex-wrap: wrap; }
    .who { min-width: calc(100% - 30px); }
  }
</style>
