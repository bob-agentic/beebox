import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  // Compiles runes in `.svelte.ts` modules, as the app build does.
  plugins: [svelte()],
  resolve: { conditions: ['browser'] },
  test: { environment: 'jsdom', include: ['src/**/*.test.ts'] },
});
