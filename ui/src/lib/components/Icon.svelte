<script lang="ts">
  // One line-icon component for all UI chrome. Paths are Lucide's (ISC),
  // drawn on a 24×24 grid with a 2px round stroke in currentColor — so an
  // icon takes the colour and hover treatment of whatever button holds it,
  // exactly like the character glyphs it replaces did.
  //
  // Status dots stay in StatusIcon.svelte; those carry agent state, not chrome.

  type Name =
    | 'plus'
    | 'x'
    | 'settings'
    | 'share'
    | 'split-v'
    | 'split-h'
    | 'terminal'
    | 'pencil'
    | 'folder';

  let {
    name,
    size = 16,
    stroke = 2,
  }: { name: Name; size?: number; stroke?: number } = $props();

  // Each entry is the inner markup of a 24×24 viewBox.
  const PATHS: Record<Name, string> = {
    plus: '<path d="M12 5v14M5 12h14"/>',
    x: '<path d="M18 6 6 18M6 6l12 12"/>',
    settings:
      '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>',
    share:
      '<circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><path d="m8.59 13.51 6.83 3.98M15.41 6.51l-6.82 3.98"/>',
    'split-v': '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M12 3v18"/>',
    'split-h': '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 12h18"/>',
    terminal: '<path d="m4 17 6-6-6-6M12 19h8"/>',
    pencil: '<path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4Z"/>',
    folder:
      '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <!-- Static, in-repo markup keyed by a typed union — no user input reaches
       {@html}, so it is safe here. -->
  {@html PATHS[name]}
</svg>

<style>
  svg {
    display: block;
    flex: 0 0 auto;
  }
</style>
