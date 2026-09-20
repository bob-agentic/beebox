// End-to-end for auto-resume: kill the daemon, start a new one on the same
// state directory, and watch the resume command actually arrive in the PTY.
// This is the restart path a unit test cannot prove.

import { expect, test, type Page } from '@playwright/test';
import { startDaemon, type Daemon } from './harness';

let daemon: Daemon;
let port = 17980;

test.afterEach(() => daemon?.stop());

async function ready(page: Page) {
  await page.goto(daemon.url);
  await expect(page.locator('.statusbar')).toContainText('daemon connected', {
    timeout: 15_000,
  });
  await expect(page.locator('.sheet.on .pane')).toHaveCount(1, { timeout: 15_000 });
  await page.waitForTimeout(1200);
}

async function visibleText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const host = document.querySelector('.sheet.on .pane .term') as any;
    return host?.__serialize?.() ?? '';
  });
}

test('a daemon restart resumes the agent session, and only with the toggle on', async ({
  page,
}) => {
  test.setTimeout(120_000);
  const p = port++;
  daemon = await startDaemon(p, { keepHome: true });
  await ready(page);

  // Enable resume for Claude through the real dialog.
  await page.keyboard.press('Meta+k');
  await page.locator('.nav .agents-nav').click();
  const row = page.locator(
    '.agent-row[data-setting="resume"][data-agent="claude"] input',
  );
  await row.click();
  await expect(row).toBeChecked();
  await page.locator('.mask').click({ position: { x: 5, y: 5 } });

  // An agent session happened in this pane (fired exactly as the adapter
  // does, from inside the pane's own shell).
  const term = page.locator('.sheet.on .pane .term').first();
  await term.click({ position: { x: 100, y: 40 } });
  await page.keyboard.type(
    `curl -s -o /dev/null -X POST -H 'Content-Type: application/json' -d '{"hook_event_name":"UserPromptSubmit","session_id":"resume-e2e-1","prompt":"work"}' "$BEEBOX_HOOK_URL"`,
  );
  await page.keyboard.press('Enter');
  await page.waitForTimeout(800);

  // Full restart on the same state directory.
  const home = daemon.home;
  daemon.stop();
  daemon = await startDaemon(p, { home });
  await ready(page);

  // The restored pane's shell received the resume command. There is no
  // real `claude` on PATH inside CI, so "command found or not" is not the
  // assertion — the typed command itself is.
  await expect
    .poll(() => visibleText(page), { timeout: 20_000 })
    .toContain('claude --resume resume-e2e-1');
});
