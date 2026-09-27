// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

// Puts the SPDX licence header at the top of every source file, or checks that it is there:
//
//   node scripts/license-headers.mjs           # add it where it is missing
//   node scripts/license-headers.mjs --check   # list the files without one and fail (CI)
//
// Source is what `git ls-files` lists with one of the extensions in COMMENT below; data
// files, fixtures, manifests, lock files and generated files carry no header. A file that
// already has the tag is left alone, so one whose header names a second copyright holder
// keeps it. The check also reads the `license` field of the four manifests, which must
// say the same as the tag.

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const LICENSE = 'GPL-2.0-or-later';
const TAG = `SPDX-License-Identifier: ${LICENSE}`;
const HOLDER = 'Copyright (C) 2026 AJ Banck';
const COMMENT = {
  '.rs': ['// ', ''],
  '.ts': ['// ', ''],
  '.tsx': ['// ', ''],
  '.mjs': ['// ', ''],
  '.css': ['/* ', ' */'],
  '.html': ['<!-- ', ' -->'],
};
const MANIFESTS = ['package.json', 'web/package.json', 'core/Cargo.toml', 'desktop/Cargo.toml'];

const check = process.argv.includes('--check');
const files = execFileSync('git', ['ls-files', '-z'], { cwd: ROOT, encoding: 'utf8' })
  .split('\0')
  .filter((f) => f && COMMENT[extname(f)]);

const missing = [];
for (const file of files) {
  const path = join(ROOT, file);
  const text = readFileSync(path, 'utf8');
  if (text.split('\n', 3).some((line) => line.includes(TAG))) continue;
  missing.push(file);
  if (check) continue;
  const [open, close] = COMMENT[extname(file)];
  const header = `${open}${TAG}${close}\n${open}${HOLDER}${close}\n\n`;
  // An HTML file keeps its doctype on the first line.
  const doctype = /^<!doctype[^\n]*\n/i.exec(text);
  writeFileSync(path, doctype ? doctype[0] + header + text.slice(doctype[0].length) : header + text);
}

const wrong = MANIFESTS.filter((m) => {
  const text = readFileSync(join(ROOT, m), 'utf8');
  return !new RegExp(`^\\s*"?license"?\\s*[:=]\\s*"${LICENSE}"`, 'm').test(text);
});
for (const m of wrong) console.error(`${m}: license is not ${LICENSE}`);

if (check) {
  for (const f of missing) console.error(`${f}: no licence header`);
  if (missing.length || wrong.length) process.exit(1);
  console.log(`${files.length} source files carry the licence header`);
} else {
  console.log(`${missing.length} of ${files.length} source files given the licence header`);
  if (wrong.length) process.exit(1);
}
