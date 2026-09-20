// End-to-end for the M3 agent-status slice: hook events fired from *inside*
// the pane's own shell (using the injected $BEEBOX_HOOK_URL, exactly as the
// adapters do) must light the pane, tab, and workspace dots in a real
// browser, carry the tooltip, and auto-title the tab.

import { expect, test, type Page } from '@playwright/test';
import { startDaemon, type Daemon } from './harness';

let daemon: Daemon;
let port = 17950;

test.beforeEach(async () => {
  daemon = await startDaemon(port++);
});

test.afterEach(() => daemon?.stop());

async function ready(page: Page) {
  await page.goto(daemon.url);
  await expect(page.locator('.statusbar')).toContainText('daemon connected', {
    timeout: 15_000,
  });
  await expect(page.locator('.sheet.on .pane')).toHaveCount(1, { timeout: 15_000 });
  await page.waitForTimeout(1200);
}

/** Flips one Agents toggle through the real settings dialog — the same path
    a user takes, which also covers the dialog itself. */
async function setAgentToggle(
  page: Page,
  setting: 'status' | 'resume',
  agent: string,
  on: boolean,
) {
  await page.keyboard.press('Meta+k');
  await page.locator('.nav .agents-nav').click();
  const row = page.locator(
    `.agent-row[data-setting="${setting}"][data-agent="${agent}"] input`,
  );
  await expect(row).toBeVisible({ timeout: 10_000 });
  if ((await row.isChecked()) !== on) await row.click();
  // The daemon echoes the snapshot; wait for the checkbox to settle.
  await expect(row).toBeChecked({ checked: on });
  await page.locator('.mask').click({ position: { x: 5, y: 5 } });
  await expect(page.locator('.modal')).toHaveCount(0);
}

/** Fires one hook event from inside the focused pane's shell — the same
    trust path the real adapters use. */
async function fireHook(page: Page, json: string) {
  const term = page.locator('.sheet.on .pane .term').first();
  await term.click({ position: { x: 100, y: 40 } });
  // Single quotes keep the JSON intact through zsh.
  await page.keyboard.type(
    `curl -s -o /dev/null -X POST -H 'Content-Type: application/json' -d '${json}' "$BEEBOX_HOOK_URL"`,
  );
  await page.keyboard.press('Enter');
}

test('hook events drive the pane, tab and workspace dots', async ({ page }) => {
  await ready(page);
  await setAgentToggle(page, 'status', 'claude', true);

  // Before any agent event there is no dot at all.
  await expect(page.locator('.pane-head .ast')).toHaveCount(0);

  await fireHook(
    page,
    '{"hook_event_name":"UserPromptSubmit","session_id":"e2e-1","prompt":"fix the login flow"}',
  );
  await expect(page.locator('.pane-head .ast.running')).toHaveCount(1, {
    timeout: 10_000,
  });
  // The dot aggregates upward without any further events.
  await expect(page.locator('.tab .ast.running')).toHaveCount(1);
  await expect(page.locator('.ws .ast.running')).toHaveCount(1);
  // And the badge switched from SH to the agent.
  await expect(page.locator('.pane-head .agent')).toHaveText('CC');

  await fireHook(
    page,
    '{"hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"/repo/src/auth.rs"}}',
  );
  // Tool detail reaches the custom hover tooltip.
  await expect(page.locator('.pane-head .ast.running')).toHaveCount(1, {
    timeout: 10_000,
  });
  await page.locator('.pane-head .ast-wrap').hover();
  await expect(page.locator('.pane-head .tip')).toContainText('Edit repo/src/auth.rs', {
    timeout: 10_000,
  });
  await page.mouse.move(400, 400);

  await fireHook(page, '{"hook_event_name":"Stop"}');
  await expect(page.locator('.pane-head .ast.success')).toHaveCount(1, {
    timeout: 10_000,
  });
  // This browser has the pane focused (we typed in it), so the completion is
  // immediately read: hollow dot.
  await expect(page.locator('.pane-head .ast.success.read')).toHaveCount(1);
});

test('a failing tool turns the dot red, and the next prompt clears it', async ({
  page,
}) => {
  await ready(page);
  await setAgentToggle(page, 'status', 'claude', true);

  await fireHook(page, '{"hook_event_name":"UserPromptSubmit","prompt":"run the tests"}');
  await fireHook(
    page,
    '{"hook_event_name":"PostToolUse","tool_response":{"is_error":true}}',
  );
  await fireHook(page, '{"hook_event_name":"Stop"}');
  await expect(page.locator('.pane-head .ast.failed')).toHaveCount(1, {
    timeout: 10_000,
  });

  await fireHook(page, '{"hook_event_name":"UserPromptSubmit","prompt":"try again"}');
  await expect(page.locator('.pane-head .ast.running')).toHaveCount(1, {
    timeout: 10_000,
  });
  await fireHook(page, '{"hook_event_name":"Stop"}');
  await expect(page.locator('.pane-head .ast.success')).toHaveCount(1, {
    timeout: 10_000,
  });
});

test('the first prompt becomes the tab title until renamed', async ({ page }) => {
  await ready(page);
  await setAgentToggle(page, 'status', 'claude', true);

  await fireHook(
    page,
    '{"hook_event_name":"UserPromptSubmit","session_id":"e2e-t","prompt":"refactor the auth middleware"}',
  );
  await expect(page.locator('.tab .label')).toContainText(
    'refactor the auth middleware',
    { timeout: 10_000 },
  );

  // A manual rename wins over the session title, permanently.
  await page.locator('.tab .label').dblclick();
  await page.locator('.tab input.rename').fill('my tab');
  await page.keyboard.press('Enter');
  await expect(page.locator('.tab .label')).toHaveText('my tab');

  await fireHook(
    page,
    '{"hook_event_name":"UserPromptSubmit","prompt":"another prompt entirely"}',
  );
  await page.waitForTimeout(500);
  await expect(page.locator('.tab .label')).toHaveText('my tab');
});

test('with Notifications off the dot never appears; toggling off hides it at once', async ({
  page,
}) => {
  await ready(page);

  // Default is OFF: a hook event must not paint anything.
  await fireHook(page, '{"hook_event_name":"UserPromptSubmit","prompt":"quiet"}');
  await page.waitForTimeout(800);
  await expect(page.locator('.pane-head .ast')).toHaveCount(0);

  // ON: the next event paints.
  await setAgentToggle(page, 'status', 'claude', true);
  await fireHook(page, '{"hook_event_name":"UserPromptSubmit","prompt":"loud"}');
  await expect(page.locator('.pane-head .ast.running')).toHaveCount(1, {
    timeout: 10_000,
  });

  // OFF again: the dot disappears immediately, no stale dot.
  await setAgentToggle(page, 'status', 'claude', false);
  await expect(page.locator('.pane-head .ast')).toHaveCount(0, { timeout: 10_000 });
});

test('reset turns all toggles off', async ({ page }) => {
  await ready(page);
  await setAgentToggle(page, 'status', 'claude', true);
  await setAgentToggle(page, 'resume', 'codex', true);

  await page.keyboard.press('Meta+k');
  await page.locator('.nav .agents-nav').click();
  await page.locator('.reset-agents').click();
  for (const sel of [
    '.agent-row[data-setting="status"][data-agent="claude"] input',
    '.agent-row[data-setting="resume"][data-agent="codex"] input',
  ]) {
    await expect(page.locator(sel)).toBeChecked({ checked: false });
  }
  await page.locator('.mask').click({ position: { x: 5, y: 5 } });
});

test('right-click resets a manual tab title back to auto', async ({ page }) => {
  await ready(page);
  await setAgentToggle(page, 'status', 'claude', true);

  await fireHook(
    page,
    '{"hook_event_name":"UserPromptSubmit","prompt":"the auto title"}',
  );
  await expect(page.locator('.tab .label')).toContainText('the auto title', {
    timeout: 10_000,
  });

  await page.locator('.tab .label').dblclick();
  await page.locator('.tab input.rename').fill('manual name');
  await page.keyboard.press('Enter');
  await expect(page.locator('.tab .label')).toHaveText('manual name');

  await page.locator('.tab .label').click({ button: 'right' });
  await expect(page.locator('.tab .label')).toContainText('the auto title');
});

test('an unread completion in another pane keeps the tab dot solid', async ({
  page,
}) => {
  await ready(page);
  await setAgentToggle(page, 'status', 'claude', true);

  // Split: two panes in the tab.
  await page.keyboard.press('Meta+d');
  await expect(page.locator('.sheet.on .pane')).toHaveCount(2, { timeout: 10_000 });
  await page.waitForTimeout(800);

  // Run a whole turn in the SECOND pane's shell (so both events hit that
  // pane's hook URL), then move focus to the first pane before the Stop
  // lands — the completion arrives while its pane is unfocused → unread.
  const panes = page.locator('.sheet.on .pane');
  const typeInSecond = (cmd: string) =>
    page.evaluate((c) => {
      const host = document.querySelectorAll('.sheet.on .pane .term')[1] as any;
      host.__type(c + '\n');
    }, cmd);

  const hook = (json: string) =>
    `curl -s -o /dev/null -X POST -H 'Content-Type: application/json' -d '${json}' "$BEEBOX_HOOK_URL"`;

  await typeInSecond(hook('{"hook_event_name":"UserPromptSubmit","prompt":"bg work"}'));
  await page.waitForTimeout(600);
  await panes.nth(0).locator('.term').click({ position: { x: 50, y: 30 } });
  await typeInSecond(hook('{"hook_event_name":"Stop"}'));

  // Unread completion: pane dot solid, and the tab dot solid too.
  await expect(
    page.locator('.sheet.on .pane .ast.success:not(.read)'),
  ).toHaveCount(1, { timeout: 10_000 });
  await expect(page.locator('.tab .ast.success:not(.read)')).toHaveCount(1);

  // Focusing the pane reads it; pane and tab dots hollow together.
  await panes.nth(1).locator('.term').click({ position: { x: 50, y: 30 } });
  await expect(page.locator('.sheet.on .pane .ast.success.read')).toHaveCount(1, {
    timeout: 10_000,
  });
  await expect(page.locator('.tab .ast.success.read')).toHaveCount(1);
});
