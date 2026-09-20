<script lang="ts">
  // Replaces link expiry: see who is connected and cut them off. Visible
  // present state beats a guessed future one.
  import { store } from '../state.svelte';

  let { onclose }: { onclose: () => void } = $props();

  function duration(secs: number): string {
    if (secs < 60) return `${secs}s`;
    if (secs < 3600) return `${Math.floor(secs / 60)} min`;
    return `${Math.floor(secs / 3600)} h`;
  }

  const COLOURS = ['#7dd3fc', '#fbbf24', '#86efac', '#c4a5e8', '#f87171'];

  const others = $derived(store.peers.filter((p) => !p.is_you).length);

  function initials(label: string): string {
    return label
      .split(/\s+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? '')
      .join('') || '?';
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal">
    <h3>Connected clients</h3>
    <div class="sub">No expiry — disconnect anyone at any time</div>

    <div class="conn-list">
      {#each store.peers as peer, i (peer.session)}
        <div class="conn">
          <span class="av" style="background:{COLOURS[i % COLOURS.length]}">
            {initials(peer.label)}
          </span>
          <div class="who">
            <b>{peer.label} · {peer.device}</b>
            <small>{peer.addr} · {peer.scope} · {duration(peer.since_secs)}</small>
          </div>
          <span class="badge" class:rw={peer.writable} class:ro={!peer.writable}>
            {peer.writable ? 'can type' : 'read-only'}
          </span>
          {#if peer.is_you}
            <!-- Disconnecting yourself would just close this window. -->
            <span class="badge you">this window</span>
          {:else}
            <button class="kick" onclick={() => store.send({ t: 'kick', session: peer.session })}>
              Disconnect
            </button>
          {/if}
        </div>
      {:else}
        <div class="empty">Nothing connected</div>
      {/each}
    </div>

    <div class="acts">
      <button class="g" onclick={onclose}>Close</button>
      {#if others > 0}
        <button class="danger" onclick={() => store.send({ t: 'kick_all' })}>
          Disconnect {others} other{others === 1 ? '' : 's'}
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
  .av {
    width: 19px;
    height: 19px;
    border-radius: 50%;
    font-size: 9px;
    font-weight: 700;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #0f0f13;
    flex: 0 0 auto;
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
  .badge.you {
    background: var(--panel-2);
    color: var(--faint);
  }
  .kick {
    font-size: 10.5px;
    padding: 4px 11px;
    border-radius: 6px;
    background: #3d1f1f;
    color: #f87171;
    flex: 0 0 auto;
  }
  .kick:hover {
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
