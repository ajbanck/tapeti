// Headless UI smoke test. Start `npm run dev` first, then `npm run smoke`.
// Loads the two generated sample tapes (scripts/make-samples.mjs), exercises the list, editor, data window, menus and undo,
// fails on console errors, and writes screenshots to scratch/.
import puppeteer from 'puppeteer-core';
import fs from 'node:fs';

const CHROME = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const URL = process.env.SMOKE_URL ?? "http://localhost:5173/?open=samples/Tapeti%20demo.tzx&right=samples/Tapeti%20demo%20(variant).tzx";
const OUT = 'scratch';
fs.mkdirSync(OUT, { recursive: true });

const browser = await puppeteer.launch({ executablePath: CHROME, headless: true, args: ['--no-sandbox'] });
const page = await browser.newPage();
await page.setViewport({ width: 1280, height: 860 });
const errors = [];
page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = (pane) => page.$$eval(`.pane:${pane} .blocklist .row`, (r) => r.length);
const expect = (cond, msg) => { if (!cond) { errors.push('ASSERT: ' + msg); console.log('FAIL', msg); } else console.log('ok  ', msg); };
const clickMenu = async (menu, label) => {
  let title;
  for (const t of await page.$$('.menubar .menu .title')) if ((await t.evaluate((e) => e.textContent)).trim() === menu) title = t;
  if (!title) throw new Error('menu not found: ' + menu);
  await title.click();
  await wait(120);
  for (const it of await page.$$('.menu .dropdown .item')) {
    const t = (await it.evaluate((e) => e.textContent)).trim();
    if (t.startsWith(label)) { await it.click(); await wait(200); return; }
  }
  throw new Error('menu item not found: ' + label);
};

await page.goto(URL, { waitUntil: 'networkidle0' });
await page.waitForSelector('.blocklist .row');
await wait(300);
await page.evaluate(() => { document.documentElement.dataset.theme = 'light'; });
expect((await rows('first-child')) === 19, 'left tape has 19 blocks');
expect((await rows('last-child')) === 10, 'right tape has 10 blocks');
await page.screenshot({ path: `${OUT}/01-main.png` });

// content detection + data window default view
const right = await page.$$('.pane:last-child .blocklist .row');
await right[6].click();
await wait(150);
expect((await page.$eval('.pane:last-child .row.cursor .kind', (e) => e.textContent)) === 'SCREEN?', 'screen block detected');
await page.keyboard.press('Enter');
await page.waitForSelector('.datawin');
await wait(300);
expect((await page.$eval('.datawin .tab.active', (e) => e.textContent)) === 'Screen', 'data window opens on Screen tab');
await page.screenshot({ path: `${OUT}/02-datawindow-screen.png` });

// Reverse order moves the base to the end of screen memory; unticking it must
// move the base back, or the picture sits above the screen and the view is blank
const check = async (label) => page.evaluateHandle((l) => [...document.querySelectorAll('.datawin label')].find((x) => x.textContent.trim() === l).querySelector('input'), label);
const screenNote = () => page.$eval('.datawin .screenwrap .note', (e) => e.textContent.trim());
const wholePicture = await screenNote();
const rev = await check('Reverse order (DEC IX)');
await rev.click(); await wait(200);
expect((await screenNote()) === wholePicture, 'reversed, the picture is still on the screen');
await rev.click(); await wait(200);
expect((await screenNote()) === wholePicture, 'unticking Reverse order brings the picture back');

await page.click('.datawin .tab[data-view=dump]');
await wait(150);

// the Dec/Hex switch belongs to the screen it is on
const mainBase = () => page.$eval('.statusbar .cell', (e) => e.textContent.trim());
const winBase = () => page.$eval('.datawin .basecell', (e) => e.textContent.trim());
expect((await winBase()) === 'Dec', 'the data window opens on Dec');
await page.click('.datawin .basecell');
await wait(150);
expect((await winBase()) === 'Hex', 'its own switch turns it to Hex');
expect((await mainBase()) === 'Dec', 'and the main window stays on Dec');
await page.keyboard.press('Escape');
await wait(150);
expect(!(await page.$('.datawin')), 'Escape closes the data window');

// a header block opens on the view that reads it out
const leftH = await page.$$('.pane:first-child .blocklist .row');
await leftH[1].click();
await page.keyboard.press('Enter');
await page.waitForSelector('.datawin');
await wait(250);
expect((await page.$eval('.datawin .tab.active', (e) => e.textContent)) === 'Header', 'header block opens on the Header tab');
await page.screenshot({ path: `${OUT}/05-datawindow-header.png` });
const hdrName = () => page.$eval('.datawin .hdrview .row .v input[type=text]', (e) => e.value.trim());
expect((await hdrName()).startsWith('demo'), 'the header view names the file');
// the same fields the block editor has, and they edit here too
await page.click('.datawin .hdrview .row .v input[type=text]');
await page.keyboard.press('End');
for (let i = 0; i < 10; i++) await page.keyboard.press('Backspace');
await page.keyboard.type('renamed');
await wait(150);
expect((await hdrName()) === 'renamed', 'the header view edits the name');
await page.click('.datawin .mfooter button.primary');
await wait(200);
const editorName = () => page.evaluate(() => { const l = [...document.querySelectorAll('.pane:first-child .editor .grid label')].find((x) => x.textContent.trim() === 'Name'); return l && l.nextElementSibling ? l.nextElementSibling.value : null; });
expect((await editorName()) === 'renamed', 'committing writes the header back into the block');
await page.keyboard.down('Meta'); await page.keyboard.press('z'); await page.keyboard.up('Meta');
await wait(150);
expect((await editorName()).startsWith('demo'), 'and undo puts the old name back');
await page.keyboard.press('Enter');
await page.waitForSelector('.datawin');
await wait(250);
await page.keyboard.press('Escape');
await wait(150);

// the menu bar is grouped by subject, not by pane; what is per pane hangs off the pane's header
const menuTitles = await page.$$eval('.menubar .menu .title', (t) => t.map((e) => e.textContent.trim()));
expect(menuTitles.join(' ') === 'File Edit Block Tape Play View Help', 'the menu bar reads File Edit Block Tape Play View Help');
await page.click('.pane:last-child .toolbar button[title="More for this tape"]');
await wait(150);
expect((await page.$$('.pane:last-child .pane-more .dropdown .item')).length === 8, "the pane header's overflow menu opens");
await page.screenshot({ path: `${OUT}/06-pane-menu.png` });
await page.click('.pane:last-child .toolbar button[title="More for this tape"]');
await wait(150);
expect((await page.$$('.pane-more .dropdown')).length === 0, 'and closes again');
const ctxRow = (await page.$$('.pane:first-child .blocklist .row'))[3];
await ctxRow.click({ button: 'right' });
await wait(150);
expect((await page.$$('.ctxmenu .item')).length === 13, 'the right-click menu is the short list');
await page.screenshot({ path: `${OUT}/07-context-menu.png` });
await page.keyboard.press('Escape');
await page.mouse.click(5, 5);
await wait(150);

// delete via menu, undo via menu
const left = await page.$$('.pane:first-child .blocklist .row');
await left[3].click();
await clickMenu('Edit', 'Delete');
expect((await rows('first-child')) === 18, 'menu Delete removes a block');
await clickMenu('Edit', 'Undo');
expect((await rows('first-child')) === 19, 'menu Undo restores it');

// hex edit + commit + undo (block 3 is BASIC, so the window opens on the BASIC tab)
const left2 = await page.$$('.pane:first-child .blocklist .row');
await left2[2].click();
await page.keyboard.press('Enter');
await page.waitForSelector('.datawin');
expect((await page.$eval('.datawin .tab.active', (e) => e.textContent)) === 'BASIC', 'BASIC block opens on BASIC tab');
await page.click('.datawin .tab[data-view=dump]');
await wait(150);
// a standard block hides its flag and checksum by default; the dump is editable
// anyway, and the byte typed lands where the view shows it
const cell = (n) => page.$$eval('.datawin .dump .hexb', (e, i) => e[i].textContent, n);
await (await page.$('.datawin .dump .hexb')).click();
await page.keyboard.type('2a');
await wait(150);
expect((await cell(0)) === '2A', 'typing edits a byte while the flag and checksum are hidden');
for (const label of ['Hide flag byte', 'Hide checksum byte']) {
  const el = await page.evaluateHandle((l) => [...document.querySelectorAll('.datawin label')].find((x) => x.textContent.trim() === l).querySelector('input'), label);
  if (await el.evaluate((e) => e.checked)) await el.click();
}
await wait(150);
expect((await cell(0)) === 'FF' && (await cell(1)) === '2A', 'that byte was the first body byte, not the flag');
await (await page.$('.datawin .dump .hexb')).click();
await page.keyboard.type('41');
await wait(150);
expect((await page.$eval('.datawin .dump .hexb', (e) => e.textContent)) === '41', 'typing hex edits the byte');
await page.click('.datawin .mfooter button.primary');
await wait(200);
expect((await page.$eval('.pane:first-child .fname', (e) => e.textContent)) !== '' && !!(await page.$('.pane:first-child .dirty-dot')), 'commit marks the tape dirty');
await page.keyboard.down('Meta'); await page.keyboard.press('z'); await page.keyboard.up('Meta');
await wait(150);
expect((await page.$eval('.pane:first-child .editor .infocol', (e) => e.textContent)).includes('Flag byte 255'), 'undo restores the original flag byte');

// program picker selects the whole program; extract copies it to the other pane
await page.click('.pane:first-child .toolbar button[title="Programs…"]');
await page.waitForSelector('.modal.programs');
expect((await page.$$('.modal.programs .proglist .t')).length === 1, 'demo tape shows one program');
expect((await page.$eval('.modal.programs .proglist .t .name', (e) => e.textContent)).startsWith('demo'), 'program is named after its BASIC header');
await page.click('.modal.programs .mfooter button.primary');
await wait(150);
expect((await page.$$('.pane:first-child .row.selected')).length === 19, 'picking the program selects all its blocks');
await clickMenu('Block', 'Extract to other pane');
await wait(200);
expect((await rows('last-child')) === 19, 'extract copies the selection into the right pane');
expect((await page.$eval('.pane:last-child .fname', (e) => e.textContent)) === 'demo.tzx', 'the extracted tape is named after the program');
expect((await rows('first-child')) === 19, 'the source tape is untouched');
await page.screenshot({ path: `${OUT}/04-extract.png` });

// deleting the last block leaves a new tape, not a named one with nothing on it
const rightRows = await page.$$('.pane:last-child .blocklist .row');
await rightRows[0].click();
await page.keyboard.down('Shift');
await (await page.$$('.pane:last-child .blocklist .row')).at(-1).click();
await page.keyboard.up('Shift');
await page.keyboard.press('Delete');
await wait(200);
expect((await rows('last-child')) === 0, 'the whole tape is deleted');
expect((await page.$eval('.pane:last-child .fname', (e) => e.textContent)) === 'new', 'an emptied pane is a new tape');
expect(!(await page.$('.pane:last-child .dirty-dot')), 'with nothing to save');
await page.keyboard.down('Meta'); await page.keyboard.press('z'); await page.keyboard.up('Meta');
await wait(200);
expect((await rows('last-child')) === 19 && (await page.$eval('.pane:last-child .fname', (e) => e.textContent)) === 'demo.tzx', 'undo brings the tape and its name back');

// dark theme render
await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; });
await wait(150);
await page.screenshot({ path: `${OUT}/03-dark.png` });

await browser.close();
if (errors.length) {
  console.error('\nErrors:\n' + errors.join('\n'));
  process.exit(1);
}
console.log('\nSmoke test passed. Screenshots in ' + OUT + '/');
