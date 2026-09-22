<script lang="ts">
  import { FONTS, LINE_HEIGHTS, SIZES, THEMES, settings } from '../settings.svelte';
  import { store } from '../state.svelte';
  import type { AgentKind, AgentSetting } from '../proto';

  let { onclose }: { onclose: () => void } = $props();

  const s = $derived(settings.current);

  // Two-pane layout: sections on the left, one section's controls on the
  // right. Agents only exists for the owner — a shared page gets Appearance
  // alone and no section list ceremony for a single entry.
  type Section = 'appearance' | 'agents';
  let section = $state<Section>('appearance');
  const showAgents = $derived(store.caps.owner && store.agentSettings !== null);

  // 600 themes need finding, not scrolling.
  let query = $state('');
  let mode = $state<'all' | 'dark' | 'light'>('all');

  const matches = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return THEMES.filter(
      (t) =>
        (mode === 'all' || (mode === 'dark') === t.dark) &&
        (!q || t.name.toLowerCase().includes(q)),
    );
  });

  /** Keeps the chosen theme in view when the dialog opens.
   *
   *  Scrolls the list by hand rather than with `scrollIntoView`, which walks
   *  up and scrolls *every* scrollable ancestor — including the panel itself,
   *  which it dragged to the bottom, hiding the heading and opening the dialog
   *  somewhere the user never asked to be. */
  function scrollToCurrent(el: HTMLElement) {
    const on = el.querySelector<HTMLElement>('.theme.on');
    if (!on) return;
    el.scrollTop = on.offsetTop - el.clientHeight / 2 + on.offsetHeight / 2;
  }

  // The Agents toggles are daemon-owned and owner-only. OpenCode is not part
  // of this product's scope, so it gets no row — the daemon still knows the
  // key, the UI just never offers it.
  const AGENT_ROWS: { agent: AgentKind; label: string }[] = [
    { agent: 'claude', label: 'Claude Code' },
    { agent: 'codex', label: 'Codex' },
  ];
  function agentToggle(agent: AgentKind, setting: AgentSetting, on: boolean) {
    store.send({ t: 'set_agent_setting', agent, setting, on });
  }
  const ag = $derived(store.agentSettings);
  const codexBadge = $derived(
    store.codexHooks === 'legacy' ? 'BETA' : store.codexHooks === 'missing' ? 'NOT FOUND' : '',
  );
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="mask" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal">
    <nav class="nav">
      <div class="nav-title">Settings</div>
      <button class:on={section === 'appearance'} onclick={() => (section = 'appearance')}>
        Appearance
      </button>
      {#if showAgents}
        <button class="agents-nav" class:on={section === 'agents'} onclick={() => (section = 'agents')}>
          Agents
        </button>
      {/if}
      <div class="nav-spacer"></div>
      <button class="done" onclick={onclose}>Done</button>
    </nav>

    <div class="body">
      {#if section === 'appearance'}
        <h3>Appearance</h3>
        <div class="sub">Stored in this browser — your phone can differ from your desk</div>

        <label class="row">
          <span class="lbl">Font</span>
          <select
            value={s.fontFamily}
            onchange={(e) => settings.update({ fontFamily: e.currentTarget.value })}
          >
            {#each FONTS as f (f.stack)}
              <option value={f.stack}>{f.name}</option>
            {/each}
          </select>
        </label>

        <label class="row">
          <span class="lbl">Size</span>
          <select
            value={String(s.fontSize)}
            onchange={(e) => settings.update({ fontSize: +e.currentTarget.value })}
          >
            {#each SIZES as n (n)}
              <option value={String(n)}>{n} px</option>
            {/each}
          </select>
        </label>

        <label class="row">
          <span class="lbl">Line height</span>
          <select
            value={s.lineHeight.toFixed(2)}
            onchange={(e) => settings.update({ lineHeight: +e.currentTarget.value })}
          >
            {#each LINE_HEIGHTS as n (n)}
              <option value={n.toFixed(2)}>{n.toFixed(2)}</option>
            {/each}
          </select>
        </label>

        <label class="row">
          <span class="lbl">Cursor blink</span>
          <input
            type="checkbox"
            checked={s.cursorBlink}
            onchange={(e) => settings.update({ cursorBlink: e.currentTarget.checked })}
          />
        </label>

        <div class="theme-head">
          <span class="lbl">Theme</span>
          <input
            class="search"
            type="search"
            placeholder="Search {THEMES.length} themes…"
            bind:value={query}
          />
          <div class="seg">
            <button class:on={mode === 'all'} onclick={() => (mode = 'all')}>All</button>
            <button class:on={mode === 'dark'} onclick={() => (mode = 'dark')}>Dark</button>
            <button class:on={mode === 'light'} onclick={() => (mode = 'light')}>Light</button>
          </div>
        </div>

        <div class="themes" use:scrollToCurrent>
          {#each matches as t (t.name)}
            <button
              class="theme"
              class:on={t.name === s.theme}
              onclick={() => settings.update({ theme: t.name })}
              title={t.name}
            >
              <span class="swatch" style="background:{t.background}">
                {#each t.palette.slice(1, 7) as c, i (i)}
                  <i style="background:{c}"></i>
                {/each}
              </span>
              <span class="tname">{t.name}</span>
            </button>
          {:else}
            <div class="none">No theme matches “{query}”</div>
          {/each}
        </div>

        <div
          class="preview"
          style="background:{settings.theme.background};color:{settings.theme.foreground};
                 font-family:{s.fontFamily};font-size:{s.fontSize}px;line-height:{s.lineHeight}"
        >
          <span style="color:{settings.theme.cursor}">❯</span>
          <span style="color:{settings.theme.palette[2]}">git</span>
          <span style="color:{settings.theme.palette[4]}">status</span>
          <span style="color:{settings.theme.palette[3]}">─ ╭──╮</span>
          <span style="color:{settings.theme.palette[1]}">✗</span>
          <span style="color:{settings.theme.palette[6]}">✓ ⏺ ⎇ 中文</span>
        </div>

        <div class="acts">
          <button class="g" onclick={() => settings.reset()}>Reset appearance</button>
        </div>
      {:else if ag}
        <h3>Agents</h3>
        <div class="sub">Stored by the daemon — applies to every device</div>

        <div class="grp">Notifications</div>
        <div class="grp-hint">
          Show live status dots (running, needs input, finished) for this agent's sessions.
        </div>
        {#each AGENT_ROWS as r (r.agent)}
          <label class="row agent-row" data-setting="status" data-agent={r.agent}>
            <span class="lbl wide"
              >{r.label}
              {#if r.agent === 'codex' && codexBadge}<span class="badge">{codexBadge}</span
                >{/if}</span
            >
            <input
              type="checkbox"
              checked={ag[`status_${r.agent}`]}
              onchange={(e) => agentToggle(r.agent, 'status', e.currentTarget.checked)}
            />
          </label>
        {/each}

        <div class="grp">Resume on Launch</div>
        <div class="grp-hint">
          Reopen this agent's last session automatically when BeeBox restarts.
        </div>
        {#each AGENT_ROWS as r (r.agent)}
          <label class="row agent-row" data-setting="resume" data-agent={r.agent}>
            <span class="lbl wide">{r.label}</span>
            <input
              type="checkbox"
              checked={ag[`resume_${r.agent}`]}
              onchange={(e) => agentToggle(r.agent, 'resume', e.currentTarget.checked)}
            />
          </label>
        {/each}

        <div class="acts">
          <button class="g reset-agents" onclick={() => store.send({ t: 'reset_agent_settings' })}>
            Restore defaults
          </button>
        </div>
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
    display: flex;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 13px;
    width: 720px;
    max-width: calc(100vw - 24px);
    height: 560px;
    max-height: calc(100vh - 48px);
    overflow: hidden;
  }

  /* Left rail: first-level sections. */
  .nav {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 160px;
    flex: 0 0 160px;
    padding: 14px 10px;
    background: var(--panel-2);
    border-right: 1px solid var(--border);
  }
  .nav-title {
    font-size: 13px;
    font-weight: 700;
    padding: 2px 8px 10px;
  }
  .nav button {
    text-align: left;
    padding: 7px 10px;
    border-radius: 7px;
    font-size: 12px;
    color: var(--dim);
  }
  .nav button:hover {
    background: var(--panel);
  }
  .nav button.on {
    background: var(--bg);
    color: var(--fg);
  }
  .nav-spacer {
    flex: 1;
  }
  .nav .done {
    text-align: center;
    background: var(--accent);
    /* The theme background always contrasts with its own accent. */
    color: var(--bg);
    font-weight: 600;
  }
  .nav .done:hover {
    background: var(--accent);
    filter: brightness(1.1);
  }

  /* Right pane: the selected section's controls. */
  .body {
    flex: 1;
    min-width: 0;
    padding: 18px 20px;
    overflow-y: auto;
  }
  h3 {
    font-size: 14px;
    margin-bottom: 4px;
  }
  .sub {
    font-size: 11.5px;
    color: var(--faint);
    margin-bottom: 16px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 11px;
  }
  .lbl {
    font-size: 11.5px;
    color: var(--dim);
    width: 84px;
    flex: 0 0 84px;
  }
  .lbl.wide {
    width: 140px;
    flex-basis: 140px;
    color: var(--fg);
  }
  /* Flat, theme-matched controls: the platform's 3D chrome clashes with the
     rest of the panel. `appearance: none` needs its own arrow. */
  select {
    flex: 0 1 260px;
    appearance: none;
    -webkit-appearance: none;
    background:
      url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='8' height='5'><path d='M0 0l4 5 4-5z' fill='%23707080'/></svg>")
        no-repeat right 9px center,
      var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 5px 24px 5px 8px;
    font-size: 12px;
  }
  select:focus {
    outline: none;
    border-color: var(--accent);
  }
  input[type='checkbox'] {
    accent-color: var(--accent);
    width: 14px;
    height: 14px;
  }

  .theme-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 16px 0 8px;
  }
  .search {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 5px 9px;
    font-size: 11.5px;
  }
  .search:focus {
    outline: none;
    border-color: var(--accent);
  }
  .seg {
    display: flex;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 2px;
    gap: 2px;
    flex: 0 0 auto;
  }
  .seg button {
    padding: 3px 9px;
    border-radius: 6px;
    font-size: 10.5px;
    color: var(--dim);
  }
  .seg button.on {
    background: var(--panel-2);
    color: var(--fg);
  }

  .themes {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    /* Fixed row height: with two matches left, `1fr` rows would stretch each
       card to fill the box. */
    grid-auto-rows: 32px;
    align-content: start;
    gap: 5px;
    margin-bottom: 14px;
    height: 200px;
    overflow-y: auto;
    padding-right: 4px;
    /* The offset parent for the cards, so centring the chosen one is a
       subtraction against this box rather than against the page. */
    position: relative;
  }
  .theme {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--bg);
    font-size: 11px;
    color: var(--dim);
    text-align: left;
    min-width: 0;
  }
  .theme:hover {
    border-color: var(--faint);
  }
  .theme.on {
    border-color: var(--accent);
    color: var(--fg);
  }
  .tname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Shows the ANSI colours, which is what a theme actually changes. */
  .swatch {
    width: 34px;
    height: 18px;
    border-radius: 5px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1px;
    flex: 0 0 auto;
    border: 1px solid rgba(128, 128, 128, 0.3);
  }
  .swatch i {
    width: 3px;
    height: 9px;
    border-radius: 1px;
  }
  .none {
    grid-column: 1 / -1;
    color: var(--faint);
    font-size: 11.5px;
    padding: 20px 0;
    text-align: center;
  }

  .preview {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    display: flex;
    gap: 7px;
    margin-bottom: 16px;
    white-space: nowrap;
    overflow: hidden;
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
  .g {
    background: var(--panel-2);
    color: var(--dim);
    border: 1px solid var(--border);
  }
  .g:hover {
    color: var(--fg);
  }

  .grp {
    margin: 14px 0 2px;
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--faint);
  }
  .grp-hint {
    font-size: 11px;
    color: var(--faint);
    margin-bottom: 8px;
  }
  .badge {
    margin-left: 6px;
    font-size: 8.5px;
    padding: 1px 4px;
    border-radius: 3px;
    background: #3d331f;
    color: #fbbf24;
    font-weight: 700;
  }

  @media (max-width: 760px) {
    .modal {
      flex-direction: column;
      width: calc(100vw - 24px);
      height: auto;
    }
    .nav {
      flex-direction: row;
      width: auto;
      flex: 0 0 auto;
      border-right: none;
      border-bottom: 1px solid var(--border);
      align-items: center;
    }
    .nav-title {
      padding: 0 8px;
    }
    .themes {
      grid-template-columns: 1fr;
    }
  }
</style>
