import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  // Each test gets a fresh daemon, so they must not race for the port.
  workers: 1,
  fullyParallel: false,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [['list']],
  use: {
    viewport: { width: 1280, height: 800 },
    // A terminal is slow to prove; keep the evidence when it fails.
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
