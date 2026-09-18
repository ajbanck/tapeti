// Builds Tapeti, the desktop app (desktop/), and packages it for the platform it
// is run on. Stage 5 of the Rust migration: this replaced `tauri build`, so it
// is what CI calls too — one script, so a release is the same steps a person runs.
//
//   node scripts/build-desktop.mjs [--debug] [--package] [--universal] [--no-build]
//
//   (nothing)    release build, plus Tapeti.app on macOS
//   --debug      debug build, for a quick run
//   --package    also write the artifacts a release carries:
//                  macOS    Tapeti_<version>_<arch>.dmg and .zip
//                  Linux    Tapeti-<version>-<arch>.AppImage (if appimagetool is
//                           there) and a .tar.gz, always
//                  Windows  Tapeti_<version>_x64_portable.exe and, with the WiX
//                           `wix` command on PATH, an .msi that registers .tzx/.tap
//   --universal  macOS: build both architectures and lipo them into one binary
//   --no-build   package what is already in desktop/target
//
// It never opens a window: running the app is the person's job. On macOS the
// binary is wrapped in an .app even for a plain build, because an unbundled
// binary gets the executable's name in the menu bar, starts behind the terminal,
// and cannot declare the document types that make "open with" work at all.
import { execFileSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  copyFileSync,
  writeFileSync,
  statSync,
  rmSync,
  symlinkSync,
  chmodSync,
  readFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const crate = join(root, 'desktop');
const dist = join(crate, 'dist');
const icons = join(root, 'assets', 'icons');

const args = process.argv.slice(2);
const has = (flag) => args.includes(flag);
const release = !has('--debug');
const profile = release ? 'release' : 'debug';
const packaging = has('--package');
const universal = has('--universal');

// The version this packages is the crate's, which is the one the app itself reports
// through CARGO_PKG_VERSION — in the About dialog, in the macOS About panel and at
// the top of a crash log. Taking it from package.json instead, as this did until
// stage 6, meant a bundle could be named after a number the app inside it did not
// say. desktop/tests/version.rs fails if package.json and core/ have drifted from it.
const version = cargoVersion(join(crate, 'Cargo.toml'));
const exe = process.platform === 'win32' ? 'tapeti.exe' : 'tapeti';

/** The `version = "…"` of a Cargo.toml's [package], read without a TOML parser:
 *  it is the first such key, before any [section] that follows. */
function cargoVersion(path) {
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const m = /^version\s*=\s*"([^"]+)"/.exec(line);
    if (m) return m[1];
    if (/^\[/.test(line) && !/^\[package\]/.test(line)) break;
  }
  throw new Error(`no [package] version in ${path}`);
}

// Homebrew's cargo is on PATH and is enough for a host build; scripts/build-wasm.mjs
// explains why the wasm build picks the rustup shim instead. A cross build (the
// universal one) needs whichever cargo has the other target installed.
const cargo = process.env.CARGO || 'cargo';
const run = (cmd, argv, opts = {}) => execFileSync(cmd, argv, { stdio: 'inherit', ...opts });
const mb = (p) => (statSync(p).size / 1024 / 1024).toFixed(1);
const out = (name) => join(dist, name);

mkdirSync(dist, { recursive: true });

// ---- build ----------------------------------------------------------------

/** The binary to package, built unless --no-build said it is already there. */
function build() {
  const flags = release ? ['--release'] : [];
  if (!universal) {
    if (!has('--no-build')) {
      console.log(`Building Tapeti (${profile})…`);
      run(cargo, ['build', ...flags], { cwd: crate });
    }
    return join(crate, 'target', profile, exe);
  }

  // A universal macOS binary is two builds and a lipo; Rust has no fat target.
  const targets = ['aarch64-apple-darwin', 'x86_64-apple-darwin'];
  const built = targets.map((t) => join(crate, 'target', t, profile, exe));
  if (!has('--no-build')) {
    for (const target of targets) {
      console.log(`Building Tapeti (${profile}, ${target})…`);
      run(cargo, ['build', ...flags, '--target', target], { cwd: crate });
    }
  }
  const fat = join(crate, 'target', `universal-${profile}`, exe);
  mkdirSync(dirname(fat), { recursive: true });
  run('lipo', ['-create', '-output', fat, ...built]);
  return fat;
}

const bin = build();
console.log(`\nBinary: ${bin} (${mb(bin)} MB)`);

// ---- macOS ----------------------------------------------------------------

/** `Tapeti.app`: the document types live in its Info.plist, and nothing else can
 *  declare them — this is what makes a tape open Tapeti when it is double-clicked. */
function macApp() {
  const app = out('Tapeti.app');
  rmSync(app, { recursive: true, force: true });
  mkdirSync(join(app, 'Contents', 'MacOS'), { recursive: true });
  mkdirSync(join(app, 'Contents', 'Resources'), { recursive: true });
  copyFileSync(bin, join(app, 'Contents', 'MacOS', 'tapeti'));
  chmodSync(join(app, 'Contents', 'MacOS', 'tapeti'), 0o755);
  const icon = join(icons, 'icon.icns');
  if (existsSync(icon)) copyFileSync(icon, join(app, 'Contents', 'Resources', 'icon.icns'));
  writeFileSync(
    join(app, 'Contents', 'Info.plist'),
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Tapeti</string>
  <key>CFBundleDisplayName</key><string>Tapeti</string>
  <key>CFBundleIdentifier</key><string>com.zxtoolkit.tapeti</string>
  <key>CFBundleExecutable</key><string>tapeti</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>ZX Spectrum tape image</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>LSItemContentTypes</key>
      <array><string>com.zxtoolkit.tapeti.tzx</string><string>com.zxtoolkit.tapeti.tap</string></array>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>com.zxtoolkit.tapeti.tzx</string>
      <key>UTTypeDescription</key><string>TZX tape image</string>
      <key>UTTypeConformsTo</key><array><string>public.data</string></array>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>tzx</string></array></dict>
    </dict>
    <dict>
      <key>UTTypeIdentifier</key><string>com.zxtoolkit.tapeti.tap</string>
      <key>UTTypeDescription</key><string>TAP tape image</string>
      <key>UTTypeConformsTo</key><array><string>public.data</string></array>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>tap</string></array></dict>
    </dict>
  </array>
</dict>
</plist>
`,
  );
  return app;
}

/** A .dmg and a .zip of the bundle. `hdiutil create` lays the image out itself, so
 *  unlike the DMG packager Tauri used it opens no Finder window. */
function macPackages(app) {
  const arch = universal ? 'universal' : process.arch === 'x64' ? 'x64' : 'aarch64';
  const dmg = out(`Tapeti_${version}_${arch}.dmg`);
  const zip = out(`Tapeti_${version}_${arch}.zip`);
  const staging = join(dist, 'dmg');
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  run('cp', ['-R', app, join(staging, 'Tapeti.app')]);
  symlinkSync('/Applications', join(staging, 'Applications'));
  rmSync(dmg, { force: true });
  run('hdiutil', [
    'create', '-volname', 'Tapeti', '-srcfolder', staging,
    '-fs', 'HFS+', '-format', 'UDZO', '-ov', '-quiet', dmg,
  ]);
  rmSync(staging, { recursive: true, force: true });
  rmSync(zip, { force: true });
  run('ditto', ['-c', '-k', '--keepParent', app, zip]);
  return [dmg, zip];
}

// ---- Linux ----------------------------------------------------------------

const DESKTOP_ENTRY = `[Desktop Entry]
Type=Application
Name=Tapeti
Comment=ZX Spectrum TZX/TAP tape editor
Exec=tapeti %F
Icon=tapeti
Categories=Utility;AudioVideo;Development;
MimeType=application/x-tzx;application/x-tap;
Terminal=false
`;

/** The AppDir an AppImage is made of, and the tarball for people who would rather
 *  have a binary. The .desktop file is where Linux learns about .tzx and .tap. */
function linuxPackages() {
  const appdir = join(dist, 'Tapeti.AppDir');
  rmSync(appdir, { recursive: true, force: true });
  mkdirSync(join(appdir, 'usr', 'bin'), { recursive: true });
  mkdirSync(join(appdir, 'usr', 'share', 'icons', 'hicolor', '128x128', 'apps'), { recursive: true });
  copyFileSync(bin, join(appdir, 'usr', 'bin', 'tapeti'));
  chmodSync(join(appdir, 'usr', 'bin', 'tapeti'), 0o755);
  writeFileSync(join(appdir, 'tapeti.desktop'), DESKTOP_ENTRY);
  copyFileSync(join(icons, '128x128.png'), join(appdir, 'tapeti.png'));
  copyFileSync(join(icons, '128x128.png'), join(appdir, '.DirIcon'));
  copyFileSync(
    join(icons, '128x128.png'),
    join(appdir, 'usr', 'share', 'icons', 'hicolor', '128x128', 'apps', 'tapeti.png'),
  );
  writeFileSync(join(appdir, 'AppRun'), '#!/bin/sh\nexec "$(dirname "$0")/usr/bin/tapeti" "$@"\n');
  chmodSync(join(appdir, 'AppRun'), 0o755);

  const made = [];
  const tar = out(`Tapeti_${version}_${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}.tar.gz`);
  run('tar', ['-czf', tar, '-C', appdir, 'usr/bin/tapeti', 'tapeti.desktop', 'tapeti.png']);
  made.push(tar);

  // appimagetool is not a build dependency: without it the tarball is the download.
  const tool = process.env.APPIMAGETOOL || 'appimagetool';
  const appimage = out(`Tapeti-${version}-${process.arch === 'arm64' ? 'aarch64' : 'x86_64'}.AppImage`);
  try {
    rmSync(appimage, { force: true });
    run(tool, ['--appimage-extract-and-run', appdir, appimage], {
      env: { ...process.env, ARCH: process.arch === 'arm64' ? 'aarch64' : 'x86_64' },
    });
    made.push(appimage);
  } catch (e) {
    console.log(`\nNo AppImage: ${tool} would not run (${e.message.split('\n')[0]}).`);
    console.log('Set APPIMAGETOOL to its path to get one; the tarball above is complete without it.');
  }
  return made;
}

// ---- Windows --------------------------------------------------------------

/** WiX v4+ source: one component with the exe, a Start menu shortcut, and the two
 *  file associations. Windows has no bundle to declare them in, so they are
 *  registry entries an installer writes — which is why there is an .msi at all. */
function wxs() {
  return `<?xml version="1.0" encoding="utf-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package Name="Tapeti" Manufacturer="Tapeti" Version="${version}" Language="1033"
           UpgradeCode="7d6f1f6e-5f1a-4a1e-9a2b-5f2d1d5a4e10" Scope="perMachine">
    <MajorUpgrade DowngradeErrorMessage="A newer version of Tapeti is already installed." />
    <MediaTemplate EmbedCab="yes" />
    <Icon Id="TapetiIcon" SourceFile="icon.ico" />
    <Property Id="ARPPRODUCTICON" Value="TapetiIcon" />
    <StandardDirectory Id="ProgramFiles6432Folder">
      <Directory Id="INSTALLFOLDER" Name="Tapeti" />
    </StandardDirectory>
    <StandardDirectory Id="ProgramMenuFolder" />
    <ComponentGroup Id="TapetiFiles" Directory="INSTALLFOLDER">
      <Component Id="TapetiExe" Guid="2f1c7d4a-9b3e-4c55-8a71-3c0f6b9d2a41">
        <File Id="TapetiExeFile" Source="tapeti.exe" KeyPath="yes">
          <Shortcut Id="StartMenuShortcut" Directory="ProgramMenuFolder" Name="Tapeti"
                    Icon="TapetiIcon" Advertise="yes" />
        </File>
        <ProgId Id="Tapeti.tzx" Description="TZX tape image" Icon="TapetiExeFile">
          <Extension Id="tzx" ContentType="application/x-tzx">
            <!-- TargetFile is what a non-advertised verb runs; without it WiX0045. -->
            <Verb Id="open" Command="Open" TargetFile="TapetiExeFile" Argument="&quot;%1&quot;" />
          </Extension>
        </ProgId>
        <ProgId Id="Tapeti.tap" Description="TAP tape image" Icon="TapetiExeFile">
          <Extension Id="tap" ContentType="application/x-tap">
            <Verb Id="open" Command="Open" TargetFile="TapetiExeFile" Argument="&quot;%1&quot;" />
          </Extension>
        </ProgId>
      </Component>
    </ComponentGroup>
    <Feature Id="Main">
      <ComponentGroupRef Id="TapetiFiles" />
    </Feature>
  </Package>
</Wix>
`;
}

function windowsPackages() {
  const made = [];
  const portable = out(`Tapeti_${version}_x64_portable.exe`);
  copyFileSync(bin, portable);
  made.push(portable);

  // The staging directory is what the .wxs paths are relative to.
  const staging = join(dist, 'msi');
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  copyFileSync(bin, join(staging, 'tapeti.exe'));
  copyFileSync(join(icons, 'icon.ico'), join(staging, 'icon.ico'));
  writeFileSync(join(staging, 'tapeti.wxs'), wxs());
  const msi = out(`Tapeti_${version}_x64.msi`);
  try {
    run('wix', ['build', 'tapeti.wxs', '-arch', 'x64', '-o', msi], { cwd: staging, shell: true });
    made.push(msi);
  } catch (e) {
    console.log(`\nNo .msi: the WiX \`wix\` command would not run (${e.message.split('\n')[0]}).`);
    console.log('Install it with `dotnet tool install --global wix`; the portable exe above needs no installer.');
  }
  return made;
}

// ---- what to say afterwards -----------------------------------------------

const made = [];
if (process.platform === 'darwin') {
  const app = macApp();
  console.log(`Bundle: ${app}`);
  if (packaging) made.push(...macPackages(app));
  console.log(`
Run it — the bundle, so the menu bar says Tapeti:
  "${app}/Contents/MacOS/tapeti" "public/samples/Tapeti demo.tzx"

A second tape opens in the right pane:
  "${app}/Contents/MacOS/tapeti" tape-a.tzx tape-b.tzx`);
} else if (process.platform === 'win32') {
  if (packaging) made.push(...windowsPackages());
  console.log(`\nRun it with:\n  "${bin}" "public\\samples\\Tapeti demo.tzx"`);
} else {
  if (packaging) made.push(...linuxPackages());
  console.log(`\nRun it with:\n  "${bin}" "public/samples/Tapeti demo.tzx"`);
}

if (made.length) {
  console.log('\nPackaged:');
  for (const f of made) console.log(`  ${f} (${mb(f)} MB)`);
}

// One line, and the binary's own --help carries the flags. Everything that used to
// be printed here was scaffolding for a question that has since been answered: the
// measuring flags for numbers that are now in CLAUDE.md, and a symlink
// into /Applications for an "open with" path confirmed on 2026-09-18.
console.log(`
Screenshots, measuring and the rest:
  "${bin}" --help`);
