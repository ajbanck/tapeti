# Tapeti

Tapeti is an editor for ZX Spectrum **TZX** and **TAP** tape images.

![Tapeti with two tapes open](docs/screenshot-main.png)

## Download

Desktop builds for macOS, Linux and Windows are attached to each
[GitHub release](../../releases).

| Platform | File | Minimum | Notes |
|---|---|---|---|
| macOS | `.dmg` or `.zip` | 10.14 on Intel, 11 on Apple Silicon | One universal binary. It is unsigned: run `xattr -cr /Applications/Tapeti.app` once, or open it from System Settings > Privacy & Security. |
| Linux | `.AppImage` or `.tar.gz` | Ubuntu 22.04 (glibc 2.35) | X11 or Wayland, OpenGL 3.3 or GLES, ALSA for sound. |
| Windows | `.msi`, or the portable `.exe` | 10 | One executable; the `.msi` only adds the shortcut and the file associations. |
| Browser | — | Safari 14.1, Chrome 90, Firefox 90 | Also runs on web views that never received updates. |

The desktop app is a single native executable with no web view, and the `.tzx`/`.tap` file
associations come with the bundle (macOS) or the installer (Windows).

The browser version is the same app without native file dialogs: Open reads a file you pick,
Save downloads a copy. To run it yourself, see [Building from source](#building-from-source).

## Features

Typical uses:

- **Check a dump.** Blocks with a bad checksum, or data shorter or longer than its header says,
  are marked `!` in the list; the consistency check reports broken loops, jumps and calls.
- **Look inside a block**: hex dump, BASIC listing, loading screen or disassembly, and edit the
  bytes in place.
- **Compare two dumps** of the same tape block by block, and see a file's
  CRC32, MD5 and SHA-1.
- **Fix a tape**: cut, paste, group, re-time, undo, then save it. Blocks you did not touch are
  written back byte for byte.
- **Try it**: play the tape as audio, export a WAV to load on a real Spectrum, or open it in an
  emulator.
- **Turn a snapshot into a tape** that loads on a real machine.

**Two tape windows**, Left and Right, each with a numbered block list, a per-block editor and
Commit / Revert buttons. The menus (File, Edit, Block, Tape, Play, View) act on the active tape;
the buttons in a tape's header act on that tape whichever is active, and its "…" button holds
the rest. On macOS the menus are in the system menu bar, elsewhere at the top of the window.

**File formats**: reads and writes TZX 1.20 (blocks 10–19, 20–28, 2A, 2B, 30–33, 35, 5A); unknown
and deprecated blocks are preserved byte for byte. An unaltered tape round-trips byte for byte:
the TZX version written is the lowest that holds the content, and never lower than the one loaded.
TAP files are read and written; only data blocks survive TAP export.

**Snapshots**: opening a `.z80` or `.sna` snapshot (48K, 128K, +2/+3, Pentagon or Scorpion 256K)
turns it into a tape that loads it on a real machine: a short BASIC loader followed by the memory
as packed blocks. A dialog asks for the loading speed (ROM timings, or 2250, 3000 or 6000 bps
turbo), the border colour and an optional `.scr` loading screen. The loader uses the bottom three
pixel lines of the screen, so those are lost; a 128K snapshot needs `LOAD ""` from 128 BASIC; and
what lived in a paged-in ROM (Interface 1, Multiface) cannot be put back. The method and the
loader are [Taper](#credits)'s.

**Block operations**: insert any block type, an empty BASIC program, or a file. A tape or snapshot
goes in as its blocks; any other file (a `.scr`, a binary) becomes the Bytes header and data block
that `SAVE "name" CODE` would have made. Cut / copy / paste / duplicate / delete, move up and down,
drag & drop within and between tapes (hold Alt or Ctrl to copy), group or loop the selection,
collapse and expand groups and loops (a collapsed group acts as one block), multi-select with
Shift-click and Ctrl-click (⌘-click on macOS), undo / redo per tape, and a lock switch against
accidental edits.

**Collection tapes**: Programs… (Ctrl+J) lists the games on the tape and selects the one you
pick; Extract to other pane (Ctrl+Shift+E) copies the selection into the other pane as a new
tape named after the program, ready for Save As. Programs start at BASIC Program headers, at
groups that contain one, and at Select block entries; custom loaders and stop blocks stay with
the program before them. Group blocks by hand to fix a wrong split.

**Block editors** for every block type: standard / turbo / pure data timings with a ROM-timings
preset, ROM header fields (type, name, length, autostart line, start address), pure tone, pulse
sequence, direct recording (T-states or sample rate), CSW, generalized data (symbol tables and
pilot stream in a text syntax), pause, group, jump, loop, call sequence, return, select,
signal level, text, message, archive info, hardware type (full hardware list), custom info
(with a text editor for `POKEs` blocks) and glue.

**Data window** (Enter, double-click or View data):

- Hex dump with in-place hex and ASCII editing, and a search with `?` wildcards.
- Header view: the 17 bytes of a ROM header as the fields they stand for, editable.
- Screen view, with FLASH animation and Save to SCR / PNG.
- BASIC listing with hidden numbers shown, 32-column formatting and 128K tokens, plus the
  variables area. **Edit** turns the program into text and **Apply** turns it back; lines you do
  not touch keep their bytes exactly, so a protected loader survives an edit elsewhere in it.
  Load text and Save listing read and write the same text as a `.bas` file.
- Text view with the Spectrum character set and tokens.
- Z80 disassembly (all prefixes, undocumented forms) with ROM routine and system variable names.
  It follows the bytes behind `RST 08` and `RST 28` as the ROM does, so a listing stays in step
  through ROM calls, and takes a symbol table of your own (Taper's `.SYM` files). Save listing
  writes it out as text.
- Modifiers: base address, flip bytes, reverse order, hide flag / checksum, and **Decrypt**,
  which shows the bytes as an encrypting loader leaves them in memory, with SpeedLock 2–7 as
  presets. Typing in the dump works through them: the byte lands where the view shows it.
- Bit and byte level Drop / Add / Shift left / Shift right and last-byte mask, on the raw data.
- Append file, replace from file, save block data to file. "View selected as one" joins the
  selected blocks bit-exactly.

**Content labels** in the block list come from the preceding ROM header when there is one. If
the data length differs from the header, the label says so (`SCREEN (short)`, `CODE 32768 (long)`);
blocks without a usable header are guessed from their size and bytes (`SCREEN?`, `BASIC?`).

**Tape tools**: Tape info shows the size, the playing time, the TZX version of the file beside
the one its blocks need, and the file's CRC32, MD5 and SHA-1. The hashes
are of the file as opened or last saved, not of the edited tape. The consistency check finds bad
nesting, useless loops, bad jumps, calls without return, infinite loops, checksum errors and a
SpeedLock group's parity. Compare tapes and Find match colour the blocks: magenta differs, grey
is ignored, green matches. Blocks with problems are marked in the list, a red `!` for errors and
an amber `!` for warnings such as a checksum that does not match or data shorter or longer than
its header says; hover for the details. A collapsed group shows the marks of the blocks inside it.

**Stepping** (Play → Step to next played block, Alt+↓): the cursor moves to the block that plays
next, following loops, jumps and calls as a player would, so a tape's flow can be checked without
playing it. Alt+↑ starts over.

**Audio**: play the tape, from the cursor or the selection, and export WAV (8/16-bit, several
sample rates) as a square wave or a MIC-emulation waveform. Loops, jumps, calls and selects are
followed as an emulator would. While playing, the status bar shows the time and the block being
played is marked in the list; click either to stop.

**Open in emulator** (Play menu, or the button in the tape's header): the desktop app writes the
tape, or the part from the cursor or the selection, to a temporary TZX and starts an emulator
with it. By default it looks for [Fuse](https://fuse-emulator.sourceforge.net/); Play → Emulator
Settings… picks another program (ZEsarUX, Spectaculator, a Flatpak, …) and its arguments. The
browser build downloads the TZX instead.

**Dec / Hex** switch, in the bottom left corner of the main window and of the data window, each
its own. A field that wants a number takes `$`/`0x` for hex and `#` for decimal whatever base is
showing.

**View** menu (remembered between sessions): *Flag and checksum bytes in hex*, *Number blocks
from 0* (on by default, as the file format counts them) and *Theme*: light, dark or
follow-the-system.

### Files on the desktop

Open and Save use native dialogs, and **Save (Ctrl+S) writes back to the file you opened**. It
writes a temporary file first, so a full disk cannot leave half a tape, and with View → Keep a
Backup When Saving the version it replaces stays beside it as `name.tzx.bak`. `.tzx` and `.tap`
files are associated with the app, so they open with a double click. Tapes can also be dropped
onto a tape pane or named on the command line: `tapeti left.tzx right.tzx` fills both panes. In
the browser, Save downloads a copy, and tapes on the same site can be given in the URL:
`?open=tapes/one.tzx&right=tapes/other.tzx`. A page that embeds the app can set its theme with
`?theme=dark`, `light` or `auto` (or just `?dark`, `?light`); the app then leaves the theme to
that page and hides its own switch.

### Keyboard and mouse

The keys are listed here with their Windows and Linux spelling. **On macOS, use ⌘ (Command) in
place of Ctrl and Option in place of Alt**; Delete is the Backspace (⌫) key there.

| Keys | Action |
|---|---|
| Click, Shift+click, Ctrl+click | Current block, select range, toggle selection |
| Double click | View data, or collapse / expand a group or loop |
| Drag & drop | Move blocks within or between tapes; hold Alt or Ctrl to copy |
| Drop a file on a tape | Open it; hold Shift to insert it at the cursor |
| ↑ ↓, Ctrl+↑ Ctrl+↓ | Move cursor, move block |
| Enter, Escape | View data, close window |
| Delete or Backspace | Delete current block or selection |
| Insert (desktop: also Ctrl+Shift+N) | Insert block |
| Ctrl+X, Ctrl+C, Ctrl+V, Ctrl+D | Cut, copy, paste, duplicate |
| Ctrl+Z, Ctrl+Shift+Z, Ctrl+A | Undo, redo, select all |
| Ctrl+O, Ctrl+S, Ctrl+Shift+S | Open, save, save as TZX (the active tape) |
| Ctrl+G, Ctrl+F | Group selection, find match |
| Ctrl+J, Ctrl+Shift+A, Ctrl+Shift+E | Pick a program, select the program at the cursor, extract selection to the other pane |
| Tab, Space | Switch active tape, play / stop from the cursor |
| Ctrl+R, Ctrl+Shift+R | Open the tape / from the cursor in the emulator (desktop) |

Mac keyboards have no Insert key: use ⌘⇧N in the desktop app, or the + button in
the tape toolbar.

### If the desktop app crashes

Tapes with unsaved changes are written out beside the crash log before the app goes, and the next
start puts them back in their panes, still unsaved, and says so.

## Building from source

1. Install a Rust toolchain from [rustup](https://rustup.rs) and Node 22 or newer. On macOS,
   install the Xcode command line tools; on Linux, `libasound2-dev` and `libxkbcommon-dev`.
2. For the browser build, add the wasm target: `rustup target add wasm32-unknown-unknown`.
3. Clone the repository and run `npm ci --prefix web` in it.
4. Build what you want:

```sh
npm run desktop     # desktop app, into desktop/dist/ (Tapeti.app on macOS)
npm run dev         # browser app at http://localhost:5173
npm run build       # browser app as a static site, into web/dist/
```

`npm run desktop:package` also writes the installers a release carries (`.dmg` and `.zip`,
`.AppImage` and `.tar.gz`, `.msi` and portable `.exe`) into `desktop/dist/`.

## Credits

TZX is the tape format defined by the [TZX specification](https://worldofspectrum.net/TZXformat.html).
The snapshot loader — the method, and the Z80 code that runs on the Spectrum — comes from
**Taper** by Martijn van der Heide (ThunderWare Research Center, 1997–2001, GPL 2 or later).
The test tapes in `core/tests/samples/` are generated by `scripts/make-samples.mjs` and contain no
third-party software.

## Licence

Tapeti is free software, released under the GNU General Public License version 2 or (at your
option) any later version. See [LICENSE](LICENSE) for the full text.
