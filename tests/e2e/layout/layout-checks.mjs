// Real-pointer layout checks, run by layout-e2e.mjs in Chromium against the harness. V1's navigation and details-panel
// checks, and the download list's (#59) and the details panel's (#61). The Settings screen's are #30's.
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
  const details = page.getByRole('complementary', { name: 'Download details' });
  const detailsSep = page.getByRole('separator', { name: 'Resize details panel' });
  const saved = () => page.evaluate(() => JSON.parse(localStorage.getItem('velocity-layout-harness-settings') ?? 'null')?.appearance ?? null);
  const saves = () => page.evaluate(() => window.appearanceSaves);

  check('first run: navigation 232 px at 1366 wide', (await width(nav)) === 232, `${String(await width(nav))} px`);
  check('first run: details panel closed', (await details.count()) === 0);

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

  // The details panel (#61): opened from the toolbar for the selected row, then a drag 100 px left widens it.
  await row(page, 'extremely-long-file-name').click();
  await page.getByRole('button', { name: 'Toggle details panel' }).click();
  await details.waitFor();
  check('details: opens at the 300 px default', (await width(details)) === 300, `${String(await width(details))} px`);
  check('details: shows the selected download', ((await details.getByRole('heading', { level: 2 }).textContent()) ?? '').startsWith('Ubuntu-24.04.1'));
  c = await center(detailsSep);
  await page.mouse.move(c.x, c.y);
  await page.mouse.down();
  await page.mouse.move(c.x - 50, c.y, { steps: 3 });
  await page.mouse.move(c.x - 100, c.y, { steps: 3 });
  await page.mouse.up();
  check('details: drag widens to 400 px', (await width(details)) === 400, `${String(await width(details))} px`);
  check('details: table row still selected', (await row(page, 'extremely-long-file-name').getAttribute('aria-selected')) === 'true');
  await detailsSep.focus();
  await page.keyboard.press('ArrowLeft');
  check('keyboard: ArrowLeft widens the details panel by 16 px', (await width(details)) === 416, `${String(await width(details))} px`);
  await shot(page, 'laptop-1366x768-light-details');
  await page.waitForTimeout(400);
  check('details: width and visibility saved', (await saved())?.layout.detailsWidth === 416 && (await saved())?.layout.detailsVisible === true, JSON.stringify((await saved())?.layout));
  await row(page, 'archive.zip').click();
  check('details: follows the selection', (await details.getByRole('heading', { level: 2 }).textContent()) === 'archive.zip');

  // A "restart": the same harness without fresh=1 reads what was saved.
  await page.goto(BASE);
  await grid.waitFor();
  await page.getByText('archive.zip').first().waitFor();
  check('restart: navigation width restored', (await width(nav)) === 292, `${String(await width(nav))} px`);
  check('restart: column width restored', (await width(sizeHeader)) === after, `${String(await width(sizeHeader))} px`);
  check('restart: chosen columns restored', (await page.getByRole('columnheader', { name: 'Destination' }).count()) === 1);
  await details.waitFor();
  check('restart: details panel reopened at its width', (await width(details)) === 416, `${String(await width(details))} px`);
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
  // TABLE_MIN (440 px) holds the list and the 1 px dividers beside it (layoutLogic.ts): two with the details panel open.
  const dividers = (await width(navSep)) + (await width(detailsSep));
  check('960 px: the list keeps a usable width', (await width(grid)) + dividers >= 440, `${String(await width(grid))} px and ${String(dividers)} px of dividers`);
  check('960 px: details shrinks to its minimum first', (await width(details)) === 280, `${String(await width(details))} px`);
  await shot(page, 'min-960x600-light-resized');
  await page.setViewportSize({ width: 1366, height: 768 });
  await page.waitForTimeout(100);
  check('back to 1366 px: saved widths return', (await width(details)) === 416 && (await width(nav)) === 292, `${String(await width(details))} and ${String(await width(nav))} px`);

  // Close and reopen the details panel.
  await page.getByRole('button', { name: 'Close details' }).click();
  const gridWide = await width(grid);
  await page.getByRole('button', { name: 'Toggle details panel' }).click();
  await details.waitFor();
  check('details: closing gives the table the space back', gridWide >= 1366 - 292 - 2, `${String(gridWide)} px`);
  check('details: reopens at the saved width', (await width(details)) === 416, `${String(await width(details))} px`);

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
  await row(dark.page, 'archive.zip').click();
  await dark.page.keyboard.press('Enter');
  await dark.page.getByRole('complementary', { name: 'Download details' }).waitFor();
  await shot(dark.page, 'min-960x600-dark-nav-collapsed-details');
  const darkOverflow = await dark.page.evaluate(() => document.documentElement.scrollWidth);
  check('dark 960 px, collapsed navigation, details open: no window overflow', darkOverflow <= 960, `${String(darkOverflow)} px`);
  await dark.ctx.close();
}
