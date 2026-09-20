// Appearance settings. Local to the browser, not the daemon: your phone and
// your desktop want different sizes, and a shared setting would fight itself.

const KEY = 'beebox.settings';

export type { Theme } from './themes';
import { THEMES as BUILT_IN_THEMES, type Theme } from './themes';

/** The prototype's own palette. It is kept outside the generated catalogue so
 * rebuilding the imported Ghostty themes cannot silently change the product's
 * default appearance. */
const BEEBOX_THEME: Theme = {
  name: 'BeeBox',
  dark: true,
  background: '#101015',
  foreground: '#d4d4dc',
  cursor: '#4ade80',
  selectionBackground: '#30483a',
  palette: [
    '#101015', '#f87171', '#4ade80', '#fbbf24',
    '#7dd3fc', '#c4a5e8', '#67e8f9', '#d4d4dc',
    '#5a5a68', '#fca5a5', '#86efac', '#fde68a',
    '#38bdf8', '#d8b4fe', '#a5f3fc', '#e8e8ee',
  ],
  chrome: {
    bg: '#0f0f13',
    panel: '#17171c',
    panel2: '#1d1d24',
    border: '#2b2b35',
    fg: '#e8e8ee',
    dim: '#8b8b9a',
    faint: '#5a5a68',
  },
};

export const THEMES: Theme[] = [BEEBOX_THEME, ...BUILT_IN_THEMES];


/** Discrete steps, not a slider: a terminal only looks right at whole pixel
    sizes, and a dropdown is easier to hit than a thumb. */
export const SIZES = [10, 11, 12, 13, 14, 15, 16, 18, 20, 22];
export const LINE_HEIGHTS = [1.0, 1.1, 1.15, 1.2, 1.3, 1.4, 1.5];

/** The embedded Nerd Font first — it is the only one guaranteed to have the
    box-drawing and Powerline glyphs an agent TUI draws. */
export const FONTS = [
  { name: 'MesloLGS NF', stack: '"MesloLGS NF", ui-monospace, Menlo, monospace' },
  { name: 'SF Mono', stack: '"SF Mono", ui-monospace, Menlo, monospace' },
  { name: 'Menlo', stack: 'Menlo, ui-monospace, monospace' },
  { name: 'Monaco', stack: 'Monaco, ui-monospace, monospace' },
  { name: 'JetBrains Mono', stack: '"JetBrains Mono", ui-monospace, Menlo, monospace' },
  { name: 'Fira Code', stack: '"Fira Code", ui-monospace, Menlo, monospace' },
];

export interface Settings {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  theme: string;
  cursorBlink: boolean;
}

/** A sensible starting point out of 600 — and the fallback if a saved theme
    name no longer exists. */
const DEFAULT_THEME = BEEBOX_THEME;

const DEFAULTS: Settings = {
  fontFamily: FONTS[0].stack,
  fontSize: 12,
  lineHeight: 1.2,
  theme: DEFAULT_THEME.name,
  cursorBlink: true,
};

function load(): Settings {
  try {
    const raw = localStorage.getItem(KEY);
    // Merge, so a setting added in a later version gets its default rather
    // than `undefined`.
    return raw ? { ...DEFAULTS, ...JSON.parse(raw) } : { ...DEFAULTS };
  } catch {
    return { ...DEFAULTS };
  }
}

class SettingsStore {
  current = $state<Settings>(load());

  get theme(): Theme {
    return THEMES.find((t) => t.name === this.current.theme) ?? DEFAULT_THEME;
  }

  /** The full xterm theme, including ANSI 0-15. Passing only background and
      foreground would leave every coloured command output on xterm's own
      defaults, which is what makes a theme look half-applied. */
  get xterm() {
    const t = this.theme;
    const [
      black, red, green, yellow, blue, magenta, cyan, white,
      brightBlack, brightRed, brightGreen, brightYellow,
      brightBlue, brightMagenta, brightCyan, brightWhite,
    ] = t.palette;

    return {
      background: t.background,
      foreground: t.foreground,
      cursor: t.cursor,
      cursorAccent: t.background,
      selectionBackground: t.selectionBackground,
      black, red, green, yellow, blue, magenta, cyan, white,
      brightBlack, brightRed, brightGreen, brightYellow,
      brightBlue, brightMagenta, brightCyan, brightWhite,
    };
  }

  update(patch: Partial<Settings>) {
    this.current = { ...this.current, ...patch };
    try {
      localStorage.setItem(KEY, JSON.stringify(this.current));
    } catch {
      // Private browsing. The setting still applies for this session.
    }
    this.applyChrome();
    for (const fn of this.listeners) fn();
  }

  reset() {
    this.update({ ...DEFAULTS });
  }

  /** Repaints the app chrome to match the terminal theme. */
  applyChrome() {
    const c = this.theme.chrome;
    const root = document.documentElement.style;
    root.setProperty('--bg', c.bg);
    root.setProperty('--panel', c.panel);
    root.setProperty('--panel-2', c.panel2);
    root.setProperty('--border', c.border);
    root.setProperty('--fg', c.fg);
    root.setProperty('--dim', c.dim);
    root.setProperty('--faint', c.faint);
    // Accent follows the theme's ANSI green: selection highlights, focus
    // rings and primary buttons then match whatever palette is active instead
    // of a hardcoded green that clashes with light themes.
    root.setProperty('--accent', this.theme.palette[2]);
  }

  /** Panes subscribe so a change applies to terminals that already exist. */
  private listeners = new Set<() => void>();

  onChange(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
}

export const settings = new SettingsStore();
settings.applyChrome();
