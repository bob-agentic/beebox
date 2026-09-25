<script lang="ts">
  // The keys a phone keyboard does not have, in Termux's layout. Each one is
  // typed into the pane on screen as if it came from a keyboard, so the
  // terminal's own modes — application cursor keys in vim or less — apply.
  import { store } from '../state.svelte';

  type Key = { label: string; seq?: string; app?: string; mod?: 'ctrl' | 'alt'; step?: -1 | 1 };

  const ROWS: Key[][] = [
    [
      { label: 'ESC', seq: '\x1b' },
      { label: '/', seq: '/' },
      { label: '-', seq: '-' },
      { label: 'HOME', seq: '\x1b[H', app: '\x1bOH' },
      { label: '↑', seq: '\x1b[A', app: '\x1bOA' },
      { label: 'END', seq: '\x1b[F', app: '\x1bOF' },
      { label: 'PGUP', seq: '\x1b[5~' },
    ],
    [
      { label: 'TAB', seq: '\t' },
      { label: 'CTRL', mod: 'ctrl' },
      { label: 'ALT', mod: 'alt' },
      { label: '←', seq: '\x1b[D', app: '\x1bOD' },
      { label: '↓', seq: '\x1b[B', app: '\x1bOB' },
      { label: '→', seq: '\x1b[C', app: '\x1bOC' },
      { label: 'PGDN', seq: '\x1b[6~' },
    ],
  ];

  // ⌘↑/⌘↓: the message you sent before or after. Only where Claude Code or
  // Codex runs, the one place there are sent messages to find — and there
  // they take the place of PGUP/PGDN, which the agents make no use of.
  const STEPS: Key[] = [
    { label: 'MSG↑', step: -1 },
    { label: 'MSG↓', step: 1 },
  ];
  const rows = $derived(
    store.isAgent(store.targetPane()) ? ROWS.map((row, i) => [...row.slice(0, -1), STEPS[i]]) : ROWS,
  );

  function press(k: Key) {
    if (k.step) store.stepSent(k.step);
    else if (k.mod) store.mods[k.mod] = !store.mods[k.mod];
    else store.typeKey(k.seq!, k.app);
  }
</script>

<!-- pointerdown, not click, and default prevented: a tap must not move focus
     off the terminal, or the soft keyboard closes under the finger. -->
<div class="keybar">
  {#each rows as row}
    <div class="row">
      {#each row as k}
        <button
          class:on={k.mod && store.mods[k.mod]}
          onpointerdown={(e) => {
            e.preventDefault();
            press(k);
          }}>{k.label}</button
        >
      {/each}
    </div>
  {/each}
</div>

<style>
  .keybar {
    flex: 0 0 auto;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }
  .row {
    display: flex;
  }
  button {
    flex: 1;
    height: 36px;
    background: none;
    border: none;
    color: var(--fg);
    font: 12px ui-monospace, monospace;
    touch-action: manipulation;
  }
  button:active,
  button.on {
    color: var(--accent);
  }
</style>
