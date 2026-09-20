<script lang="ts">
  // Opening a workspace works without the native folder chooser: Tauri v2
  // hides that plugin unless its capability is declared, and in a browser tab
  // it does not exist at all. So ⌘N always lands here, and the native picker
  // is offered as a shortcut when it happens to be available.
  import { store } from '../state.svelte';

  let { onclose }: { onclose: () => void } = $props();

  let path = $state('');
  let input: HTMLInputElement;

  const hasPicker = typeof (window as any).__BEEBOX__?.pickDirectory === 'function';

  /** The very first workspace: greet rather than restate the obvious. Once
      there is one open, this is just "add another". */
  const firstRun = $derived(store.tree.workspaces.length === 0);

  /** Recently used paths, so the common case is one click. */
  const recent = $derived(
    store.tree.workspaces.map((w) => w.path).filter(Boolean),
  );

  function submit() {
    const p = path.trim();
    if (!p) return;
    store.send({ t: 'open_workspace', path: p });
    onclose();
  }

  async function browse() {
    const picked = await store.pickDirectory();
    if (picked) {
      store.send({ t: 'open_workspace', path: picked });
      onclose();
    }
  }

  $effect(() => {
    input?.focus();
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal">
    {#if firstRun}
      <h3>Welcome to BeeBox</h3>
      <div class="sub">
        A workspace is a project folder — it keeps its own tabs and terminals.
        Choose one to get started.
      </div>
    {:else}
      <h3>Open workspace</h3>
      <div class="sub">A project folder. Each workspace keeps its own tabs.</div>
    {/if}

    <div class="field">
      <input
        bind:this={input}
        bind:value={path}
        placeholder="~/repo/my-project"
        spellcheck="false"
        autocapitalize="off"
        onkeydown={(e) => {
          if (e.key === 'Enter') submit();
          if (e.key === 'Escape') onclose();
        }}
      />
      {#if hasPicker}
        <button class="browse" onclick={browse}>Browse…</button>
      {/if}
    </div>

    {#if recent.length}
      <div class="label">Already open</div>
      <div class="recent">
        {#each recent as p (p)}
          <button class="chip" onclick={() => (path = p)} title={p}>{p}</button>
        {/each}
      </div>
    {/if}

    <div class="acts">
      <button class="g" onclick={onclose}>Cancel</button>
      <button class="p" onclick={submit} disabled={!path.trim()}>Open</button>
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
    width: 460px;
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
  .field {
    display: flex;
    gap: 8px;
    margin-bottom: 14px;
  }
  input {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 9px 11px;
    font: 12px ui-monospace, monospace;
  }
  input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .browse {
    padding: 0 13px;
    border-radius: 7px;
    background: var(--panel-2);
    color: var(--dim);
    border: 1px solid var(--border);
    font-size: 11.5px;
    flex: 0 0 auto;
  }
  .browse:hover {
    color: var(--fg);
  }
  .label {
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--faint);
    margin-bottom: 6px;
  }
  .recent {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 14px;
  }
  .chip {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 4px 9px;
    font: 11px ui-monospace, monospace;
    color: var(--dim);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip:hover {
    border-color: var(--faint);
    color: var(--fg);
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
  .acts .p {
    background: var(--accent);
    color: #0f0f13;
    font-weight: 600;
  }
  .acts .p:disabled {
    opacity: 0.4;
  }

  @media (max-width: 500px) {
    .modal { width: calc(100vw - 24px); }
  }
</style>
