// Real-pointer layout checks, run by layout-e2e.mjs in Chromium against the harness. V1's navigation checks, and the
// download list's (#59). The details panel's are #61's, and the Settings screen's #30's.
const width = async (loc) => Math.round((await loc.boundingBox())?.width ?? -1);
const center = async (loc) => {
  const b = await loc.boundingBox();
  if (!b) throw new Error('element not visible');
  return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
};

export async function runChecks({ open, shot, row, newPage, check, BASE }) {
  const { ctx, page } = await newPage(1366, 768);
  await open(page, 'fresh=1&theme=light');
  const nav = page.getByRole('navigation', { name: 'Primary' });
  const navSep = page.getByRole('separator', { name: 'Resize navigation' });
  const grid = page.getByRole('grid', { name: 'Downloads' });
  const sizeHeader = page.getByRole('columnheader', { name: 'Size' });
  const sizeSep = page.getByRole('separator', { name: 'Resize Size column' });
  const saved = () => page.evaluate(() => JSON.parse(localStorage.getItem('velocity-layout-harness-settings') ?? 'null')?.appearance ?? null);
  const saves = () => page.evaluate(() => window.appearanceSaves);

  check('first run: navigation 232 px at 1366 wide', (await width(nav)) === 232, `${String(await width(nav))} px`);

  // Select a row first; no drag may change the selection.
  await row(page, 'archive.zip').click();

  // The navigation: a live preview, then one save on release.
  let c = await center(navSep);
  await page.mouse.move(c.x, c.y);
  await page.mouse.down();
  await page.mouse.move(c.x + 30, c.y, { steps: 3 });
  await page.mouse.move(c.x + 60, c.y, { steps: 3 });
  await page.waitForTimeout(50);
  const navMid = await width(nav);
  await page.mouse.up();
  check('navigation drag: previews while dragging', navMid === 292, `${String(navMid)} px mid-drag`);
  check('navigation drag: 292 px after release', (await width(nav)) === 292);
  check('navigation drag: row selection kept', (await row(page, 'archive.zip').getAttribute('aria-selected')) === 'true');
  await page.waitForTimeout(400);
  check('navigation drag: width saved', (await saved())?.layout.sidebarWidth === 292, JSON.stringify((await saved())?.layout));

  // A column edge (#59): a live preview, nothing saved while dragging, one save on release.
  const before = await width(sizeHeader);
  const savesBefore = await saves();
  c = await center(sizeSep);
  await page.mouse.move(c.x, c.y);
  await page.mouse.down();
  for (const dx of [10, 20, 30, 40, 50, 60]) await page.mouse.move(c.x + dx, c.y);
  await page.waitForTimeout(50);
  const sizeMid = await width(sizeHeader);
  const savesMid = await saves();
  await page.mouse.up();
  await page.waitForTimeout(300);
  const after = await width(sizeHeader);
  check('column drag: previews while dragging', sizeMid === before + 60, `${String(before)} → ${String(sizeMid)} px mid-drag`);
  check('column drag: nothing saved while dragging', savesMid === savesBefore, `${String(savesMid - savesBefore)} saves`);
  check('column drag: saved once on release', (await saves()) === savesBefore + 1, `${String((await saves()) - savesBefore)} saves`);
  check('column drag: the saved width is the one shown', (await saved())?.columnWidths.size === after, `${String((await saved())?.columnWidths.size)} vs ${String(after)} px`);
  check('column drag: row selection kept', (await row(page, 'archive.zip').getAttribute('aria-selected')) === 'true');

  // The column chooser: Destination, then the menu closes.
  await page.getByRole('button', { name: 'Choose columns' }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Destination' }).click();
  await page.keyboard.press('Escape');
  await page.getByRole('columnheader', { name: 'Destination' }).waitFor();
  await page.waitForTimeout(300);
  check('columns: Destination shown and saved', (await saved())?.visibleColumns.includes('destination') === true);

  // A "restart": the same harness without fresh=1 reads what was saved.
  await page.goto(BASE);
  await grid.waitFor();
  await page.getByText('archive.zip').first().waitFor();
  check('restart: navigation width restored', (await width(nav)) === 292, `${String(await width(nav))} px`);
  check('restart: column width restored', (await width(sizeHeader)) === after, `${String(await width(sizeHeader))} px`);
  check('restart: chosen columns restored', (await page.getByRole('columnheader', { name: 'Destination' }).count()) === 1);
  check('retrying: counts down to the engine’s time', /^Retrying in 1m \d\ds$/.test((await row(page, 'flaky-server-file.bin').getByText(/^Retrying/).textContent()) ?? ''));

  // The smallest window: nothing overflows it; the list scrolls sideways in its own area.
  await page.setViewportSize({ width: 960, height: 600 });
  await page.waitForTimeout(150);
  const overflow = await page.evaluate(() => {
    const toolbar = document.querySelector('[role="toolbar"][aria-label="Download actions"]');
    return { doc: document.documentElement.scrollWidth, body: document.body.scrollWidth, toolbar: toolbar?.scrollWidth ?? -1, toolbarClient: toolbar?.clientWidth ?? 0 };
  });
  check('960 px: no horizontal overflow of the window', overflow.doc <= 960 && overflow.body <= 960, JSON.stringify(overflow));
  check('960 px: the toolbar fits', overflow.toolbar <= overflow.toolbarClient, JSON.stringify(overflow));
  check('960 px: the list keeps a usable width', (await width(grid)) >= 440, `${String(await width(grid))} px`);
  await shot(page, 'min-960x600-light-resized');
  await page.setViewportSize({ width: 1366, height: 768 });
  await page.waitForTimeout(100);

  // Sorting and search still work after all of that.
  await page.getByRole('button', { name: 'Name' }).click();
  check('table: sorting by name says so', (await page.getByRole('columnheader', { name: 'Name' }).getAttribute('aria-sort')) === 'ascending');
  await page.getByRole('searchbox', { name: 'Search downloads' }).fill('dataset-part-01');
  await page.waitForTimeout(100);
  const rows = await grid.getByRole('row').count();
  check('table: search filters after resizing', rows === 10, `${String(rows)} rows`);
  await ctx.close();

  // Dark theme, smallest window, collapsed navigation.
  const dark = await newPage(960, 600);
  await open(dark.page, 'fresh=1&theme=dark');
  await dark.page.getByRole('button', { name: 'Collapse navigation' }).click();
  await shot(dark.page, 'min-960x600-dark-nav-collapsed');
  const darkOverflow = await dark.page.evaluate(() => document.documentElement.scrollWidth);
  check('dark 960 px, collapsed navigation: no window overflow', darkOverflow <= 960, `${String(darkOverflow)} px`);
  await dark.ctx.close();
}
