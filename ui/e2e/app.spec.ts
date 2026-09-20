// End-to-end: a real browser, a real daemon, real PTYs, real mouse and
// keyboard. These cover the flows a unit test cannot — whether opening a tab
// actually gives you a working shell, whether ⌘N creates a workspace, whether
// switching tabs keeps the terminals alive.

import { expect, test, type Page } from '@playwright/test';
import { startDaemon, type Daemon } from './harness';

let daemon: Daemon;
let port = 17850;

// A fresh daemon per test: state persists to disk, so a shared one would let
// each test see the previous one's workspaces.
test.beforeEach(async () => {
  daemon = await startDaemon(port++);
});

test.afterEach(() => daemon?.stop());

/** The UI is ready once the socket is up and the first pane has painted. */
async function ready(page: Page) {
  await page.goto(daemon.url);
  await expect(page.locator('.statusbar')).toContainText('daemon connected', {
    timeout: 15_000,
  });
  await expect(page.locator('.sheet.on .pane')).toHaveCount(1, { timeout: 15_000 });
  // The shell needs a moment to draw its prompt.
  await page.waitForTimeout(1200);
}

/** Runs a command in the focused pane and waits for its output. */
async function runInTerminal(page: Page, cmd: string, expected: string) {
  const term = page.locator('.sheet.on .pane .term').first();
  await term.click({ position: { x: 100, y: 40 } });
  await page.keyboard.type(cmd);
  await page.keyboard.press('Enter');

  await expect
    .poll(() => visibleText(page), {
      timeout: 10_000,
      message: `terminal never printed ${expected}`,
    })
    .toContain(expected);
}

/** What the visible terminal is actually showing. WebGL draws to a canvas, so
    the DOM holds nothing to assert on; the app exposes xterm's own serializer
    for this. */
async function visibleText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const host = document.querySelector('.sheet.on .pane .term') as any;
    return host?.__serialize?.() ?? '';
  });
}

test('the first pane is a working shell', async ({ page }) => {
  await ready(page);
  await runInTerminal(page, 'echo E2E_FIRST_PANE', 'E2E_FIRST_PANE');
});

test('⌘T opens a tab with a working shell, and no dialog', async ({ page }) => {
  await ready(page);

  await page.keyboard.press('Meta+t');
  await expect(page.locator('.tab')).toHaveCount(2, { timeout: 10_000 });
  // Asking for anything here would be wrong: the folder is already known.
  await expect(page.locator('.modal')).toHaveCount(0);

  // The new tab is the visible one, and it must be usable.
  await expect(page.locator('.sheet.on .pane')).toHaveCount(1);
  await runInTerminal(page, 'echo E2E_NEW_TAB', 'E2E_NEW_TAB');
});

test('⌘N creates a workspace on the same folder, with no dialog', async ({ page }) => {
  await ready(page);

  await page.keyboard.press('Meta+n');
  await expect(page.locator('.ws')).toHaveCount(2, { timeout: 10_000 });
  await expect(page.locator('.modal')).toHaveCount(0);

  // Same folder, distinguishable names.
  const names = await page.locator('.ws .name').allTextContents();
  expect(names[1]).toMatch(/ 2$/);

  await runInTerminal(page, 'echo E2E_NEW_WS', 'E2E_NEW_WS');
});

test('⌘N keeps adding workspaces, not folding them into one', async ({ page }) => {
  // Two was not enough coverage: the bug that got through showed up only
  // after several presses, as one workspace with a growing pane count.
  await ready(page);

  for (let i = 0; i < 8; i++) {
    await page.keyboard.press('Meta+n');
    await expect(page.locator('.ws')).toHaveCount(i + 2, { timeout: 10_000 });
  }

  expect(await page.locator('.ws .name').allTextContents()).toEqual([
    'bee-box', 'bee-box 2', 'bee-box 3', 'bee-box 4',
    'bee-box 5', 'bee-box 6', 'bee-box 7', 'bee-box 8', 'bee-box 9',
  ]);
  // Each is its own workspace with one tab — not nine tabs in one workspace.
  await expect(page.locator('.tab')).toHaveCount(1);
});

test('⌘T keeps adding tabs to the workspace you are in', async ({ page }) => {
  await ready(page);

  for (let i = 0; i < 5; i++) {
    await page.keyboard.press('Meta+t');
    await expect(page.locator('.tab')).toHaveCount(i + 2, { timeout: 10_000 });
  }
  await expect(page.locator('.ws')).toHaveCount(1);
});

test('⌘N follows the workspace you are looking at', async ({ page }) => {
  // Picking the *newest* workspace's folder is wrong once you have switched
  // back to an earlier one.
  await ready(page);
  await page.keyboard.press('Meta+n');
  await expect(page.locator('.ws')).toHaveCount(2, { timeout: 10_000 });

  await page.locator('.ws').first().click();
  await page.waitForTimeout(300);
  await page.keyboard.press('Meta+n');
  await expect(page.locator('.ws')).toHaveCount(3, { timeout: 10_000 });
  await runInTerminal(page, 'echo E2E_FROM_FIRST', 'E2E_FROM_FIRST');
});

test('⇧⌘[ and ⇧⌘] move between tabs', async ({ page }) => {
  await ready(page);
  await page.keyboard.press('Meta+t');
  await expect(page.locator('.tab')).toHaveCount(2);
  await page.keyboard.press('Meta+t');
  await expect(page.locator('.tab')).toHaveCount(3);

  const active = () => page.locator('.tab.active .label').textContent();
  const third = await active();

  await page.keyboard.press('Shift+Meta+BracketLeft');
  await expect.poll(active).not.toBe(third);
  const second = await active();

  await page.keyboard.press('Shift+Meta+BracketRight');
  await expect.poll(active).toBe(third);
  expect(second).not.toBe(third);
});

test('⌘D and ⇧⌘D split the pane', async ({ page }) => {
  await ready(page);

  await page.keyboard.press('Meta+d');
  await expect(page.locator('.sheet.on .pane')).toHaveCount(2, { timeout: 10_000 });

  await page.keyboard.press('Shift+Meta+d');
  await expect(page.locator('.sheet.on .pane')).toHaveCount(3, { timeout: 10_000 });

  // Every split must be a live shell, not an empty box.
  await expect(page.locator('.sheet.on .pane canvas').first()).toBeVisible();
});

test('switching workspaces keeps each one’s terminals alive', async ({ page }) => {
  await ready(page);
  await runInTerminal(page, 'echo WS_ONE_MARKER', 'WS_ONE_MARKER');

  await page.keyboard.press('Meta+n');
  await expect(page.locator('.ws')).toHaveCount(2, { timeout: 10_000 });
  await runInTerminal(page, 'echo WS_TWO_MARKER', 'WS_TWO_MARKER');

  // Back to the first: its scrollback must still be there. Losing it is the
  // bug that made workspaces feel "shared".
  await page.locator('.ws').first().click();
  await page.waitForTimeout(1000);
  await expect.poll(() => visibleText(page)).toContain('WS_ONE_MARKER');
});

test('a workspace can be closed', async ({ page }) => {
  await ready(page);
  await page.keyboard.press('Meta+n');
  await expect(page.locator('.ws')).toHaveCount(2, { timeout: 10_000 });

  page.on('dialog', (d) => d.accept());
  await page.locator('.ws').nth(1).hover();
  await page.locator('.ws').nth(1).locator('.x').click();

  await expect(page.locator('.ws')).toHaveCount(1, { timeout: 10_000 });
});

test('a tab can be closed', async ({ page }) => {
  await ready(page);
  await page.keyboard.press('Meta+t');
  await expect(page.locator('.tab')).toHaveCount(2);

  // Closing now asks; the test is about the close, not the question.
  page.on('dialog', (d) => d.accept());
  await page.locator('.tab').nth(1).locator('.x').click();
  await expect(page.locator('.tab')).toHaveCount(1, { timeout: 10_000 });
});

test('double-click renames a tab', async ({ page }) => {
  await ready(page);
  await page.locator('.tab .label').first().dblclick();
  const field = page.locator('.tab .rename');
  await expect(field).toBeVisible();

  await field.fill('build pipeline');
  await field.press('Enter');
  await expect(page.locator('.tab .label').first()).toHaveText('build pipeline', {
    timeout: 10_000,
  });
});

test('tabs can be dragged into a new order, and it sticks', async ({ page }) => {
  await ready(page);
  await page.keyboard.press('Meta+t');
  await page.keyboard.press('Meta+t');
  await expect(page.locator('.tab')).toHaveCount(3, { timeout: 10_000 });

  const ids = () =>
    page.evaluate(() => [...document.querySelectorAll('.tab')].map((t) => (t as HTMLElement).dataset.sortId));
  const before = await ids();

  const first = page.locator('.tab').first();
  const last = page.locator('.tab').nth(2);
  const a = (await first.boundingBox())!;
  const b = (await last.boundingBox())!;

  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  for (let i = 1; i <= 10; i++) {
    await page.mouse.move(a.x + ((b.x + b.width - a.x) * i) / 10, a.y + a.height / 2);
    await page.waitForTimeout(20);
  }
  await page.mouse.up();
  await page.waitForTimeout(1200);

  const after = await ids();
  expect(after).not.toEqual(before);
  expect(after).toEqual([before[1], before[2], before[0]]);

  // Reordering is only useful if the server remembers it.
  await page.reload();
  await expect(page.locator('.tab')).toHaveCount(3, { timeout: 15_000 });
  expect(await ids()).toEqual(after);
});

test('the chosen theme applies and survives a reload', async ({ page }) => {
  await ready(page);

  await page.getByRole('button', { name: 'Settings' }).click();
  await page.locator('.search').fill('iTerm2 Solarized Light');
  await page.locator('.theme').first().click();
  await page.waitForTimeout(500);

  const panel = () =>
    page.evaluate(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--panel').trim(),
    );
  expect(await panel()).toBe('#fdf6e3');

  await page.getByRole('button', { name: 'Done' }).click();
  await page.reload();
  await expect(page.locator('.statusbar')).toContainText('daemon connected', {
    timeout: 15_000,
  });
  expect(await panel()).toBe('#fdf6e3');
});

test('the layout survives a reload', async ({ page }) => {
  await ready(page);
  await page.keyboard.press('Meta+d');
  await expect(page.locator('.sheet.on .pane')).toHaveCount(2, { timeout: 10_000 });

  await page.reload();
  await expect(page.locator('.statusbar')).toContainText('daemon connected', {
    timeout: 15_000,
  });
  await expect(page.locator('.sheet.on .pane')).toHaveCount(2, { timeout: 15_000 });
});

test('the desktop chrome keeps the prototype geometry', async ({ page }) => {
  await ready(page);

  const boxes = await page.evaluate(() => {
    const box = (selector: string) => {
      const r = document.querySelector(selector)!.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height };
    };
    return {
      title: box('.titlebar'),
      sidebar: box('.sidebar'),
      tabs: box('.tabbar'),
      status: box('.statusbar'),
      pane: box('.sheet.on .pane'),
    };
  });

  expect(boxes.title).toEqual({ x: 0, y: 0, width: 1280, height: 38 });
  expect(boxes.sidebar.width).toBe(206);
  expect(boxes.sidebar.y).toBe(38);
  expect(boxes.tabs).toEqual({ x: 206, y: 38, width: 1074, height: 34 });
  expect(boxes.status.height).toBe(26);
  expect(boxes.status.y).toBe(774);
  expect(boxes.pane.x).toBe(212);
  expect(boxes.pane.y).toBe(78);
});

test('the mobile layout stacks panes without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await ready(page);
  await page.keyboard.press('Meta+d');
  await expect(page.locator('.sheet.on .pane')).toHaveCount(2, { timeout: 10_000 });

  const layout = await page.evaluate(() => {
    const panes = [...document.querySelectorAll('.sheet.on .pane')].map((el) => {
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, right: r.right };
    });
    return {
      sidebar: getComputedStyle(document.querySelector('.sidebar')!).display,
      panes,
      viewport: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
    };
  });

  expect(layout.sidebar).toBe('none');
  expect(layout.scrollWidth).toBe(layout.viewport);
  expect(layout.panes[0].x).toBe(layout.panes[1].x);
  expect(layout.panes[1].y).toBeGreaterThan(layout.panes[0].y);
  expect(layout.panes.every((pane) => pane.right <= layout.viewport)).toBe(true);
});

test('⌘T with no workspace open creates one on $HOME', async ({ page }) => {
  await ready(page);

  // Empty the tree first.
  page.on('dialog', (d) => d.accept());
  await page.locator('.ws').first().hover();
  await page.locator('.ws').first().locator('.x').click();
  await expect(page.locator('.ws')).toHaveCount(0, { timeout: 10_000 });

  // ⌘T must still mean "give me a terminal" — no picker, no dead key.
  await page.keyboard.press('Meta+t');
  await expect(page.locator('.ws')).toHaveCount(1, { timeout: 10_000 });
  await expect(page.locator('.sheet.on .pane')).toHaveCount(1);
  await expect(page.locator('.modal')).toHaveCount(0);
});
