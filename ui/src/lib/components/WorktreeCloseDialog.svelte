<script lang="ts">
  // Closing a worktree's workspace deletes the worktree — that is what closing
  // one means here, so it is never one click away. You type its name, having
  // been told what goes with it: the terminals, the folder, and whatever in it
  // was never committed. The branch stays; that is git's business.
  import { store, type WorktreeInfo } from '../state.svelte';
  import type { WorkspaceView } from '../proto';

  let { ws, onclose }: { ws: WorkspaceView; onclose: () => void } = $props();

  let info = $state<WorktreeInfo | null>(null);
  let typed = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  const word = $derived(info?.branch || ws.branch || ws.name);
  const terminals = $derived(ws.tabs.reduce((n, t) => n + t.panes.length, 0));
  const ok = $derived(info !== null && typed.trim() === word && !busy);
  const home = $derived(info?.path.replace(/^\/Users\/[^/]+/, '~') ?? ws.path);

  // Asked again after a reconnect, if the first answer was lost with the old
  // connection: the count is what the confirmation is for.
  let asking = false;
  $effect(() => {
    if (info !== null || asking || !store.connected) return;
    asking = true;
    store.worktreeInfo(ws.id).then((i) => {
      asking = false;
      info = i;
    });
  });

  async function remove() {
    if (!ok) return;
    busy = true;
    error = await store.removeWorktree(ws.id);
    busy = false;
    if (error === null) onclose();
  }

  function focus(el: HTMLInputElement) {
    el.focus();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && !busy && onclose()}>
  <div class="modal">
    <h3>Close worktree “{word}”?</h3>
    <div class="sub">{home}</div>

    {#if error}
      <!-- The workspace is already gone; the folder may not be. -->
      <div class="failed">
        <b>The folder may still be on disk.</b>
        <span>{error}</span>
      </div>
      <div class="acts">
        <button class="g" onclick={onclose}>Close</button>
      </div>
    {:else}
      <div class="facts">
        <div><span class="k">Terminals</span><span>{terminals} will be closed</span></div>
        <div><span class="k">Worktree</span><span>folder will be deleted from disk</span></div>
        <div>
          <span class="k">Changes</span>
          {#if info === null}
            <span class="faint">{store.connected ? 'checking…' : 'waiting for BeeBox to reconnect…'}</span>
          {:else if info.dirty > 0}
            <span class="warn">{info.dirty} uncommitted file{info.dirty === 1 ? '' : 's'} will be lost</span>
          {:else}
            <span>nothing uncommitted</span>
          {/if}
        </div>
        <div><span class="k">Branch</span><span class="ok">{word} is kept — merge or delete it in git</span></div>
      </div>

      <div class="ask">Type <code>{word}</code> to confirm</div>
      <input
        bind:value={typed}
        use:focus
        spellcheck="false"
        autocomplete="off"
        onkeydown={(e) => {
          if (e.key === 'Enter') remove();
          if (e.key === 'Escape' && !busy) onclose();
        }}
      />
      <div class="acts">
        <button class="g" disabled={busy} onclick={onclose}>Cancel</button>
        <button class="danger" disabled={!ok} onclick={remove}>
          {busy ? 'Deleting…' : 'Delete worktree'}
        </button>
      </div>
    {/if}
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
    width: 440px;
    padding: 18px;
  }
  h3 {
    font-size: 14px;
    margin-bottom: 4px;
    overflow-wrap: anywhere;
  }
  .sub {
    font-size: 11px;
    color: var(--faint);
    margin-bottom: 14px;
    font-family: ui-monospace, monospace;
    overflow-wrap: anywhere;
  }
  .facts {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
  }
  .facts div {
    display: flex;
    gap: 10px;
    padding: 7px 11px;
    font-size: 12px;
    border-top: 1px solid var(--border);
  }
  .facts div:first-child {
    border-top: 0;
  }
  .k {
    width: 80px;
    flex: 0 0 auto;
    color: var(--faint);
  }
  .warn {
    color: var(--wait);
  }
  .ok {
    color: var(--run);
  }
  .faint {
    color: var(--faint);
  }
  .ask {
    margin: 14px 0 6px;
    font-size: 12px;
    color: var(--dim);
  }
  code {
    font-family: ui-monospace, monospace;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
  }
  input {
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 7px 10px;
    font: 12.5px ui-monospace, monospace;
    color: var(--fg);
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }
  input:focus {
    border-color: var(--accent);
  }
  .failed {
    border: 1px solid #5c2020;
    background: #2a1717;
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 14px;
  }
  .failed b {
    color: #f87171;
    font-weight: 600;
  }
  .failed span {
    color: var(--dim);
    font-family: ui-monospace, monospace;
    font-size: 11px;
    overflow-wrap: anywhere;
  }
  .acts {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 16px;
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
  .acts .danger:not(:disabled):hover {
    background: #5c2020;
    color: #fff;
  }
  .acts button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  @media (max-width: 560px) {
    .modal {
      width: calc(100vw - 24px);
    }
  }
</style>
