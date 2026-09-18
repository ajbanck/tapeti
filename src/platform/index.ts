// Platform adapter: the only place that knows how files get in and out of the app.
// This build is the browser one — <input type=file> and blob downloads. The desktop
// app is `desktop/` (Rust), which has its own file layer in `desktop/src/files.rs`;
// the adapter is kept because the rest of the UI is written against it.

export interface OpenedFile {
  name: string;
  bytes: Uint8Array;
}

export interface FileFilter {
  name: string;
  extensions: string[]; // without dots
}

export interface SaveRequest {
  suggestedName: string;
  filters: FileFilter[];
  bytes: Uint8Array;
  mime?: string;
}

export interface SaveResult {
  name: string;
}

export interface Platform {
  openFiles(opts: { filters: FileFilter[]; multiple: boolean }): Promise<OpenedFile[]>;
  /** Resolves to null when the user cancelled. */
  saveFile(req: SaveRequest): Promise<SaveResult | null>;
  /** System clipboard text access for text fields. */
  readClipboard(): Promise<string>;
  writeClipboard(text: string): Promise<void>;
}

/** macOS (or iOS): shortcuts use ⌘ there and Ctrl everywhere else. */
export const isMac: boolean = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.userAgent);

let impl: Promise<Platform> | null = null;

/** Loaded lazily, so nothing touches the DOM until something asks for a file. */
export function platform(): Promise<Platform> {
  if (!impl) impl = import('./web').then((m) => m.webPlatform);
  return impl;
}

export const TAPE_FILTERS: FileFilter[] = [
  { name: 'Tape images', extensions: ['tzx', 'tap'] },
  { name: 'TZX tape image', extensions: ['tzx'] },
  { name: 'TAP tape image', extensions: ['tap'] },
];

export function filtersForName(name: string): FileFilter[] {
  const ext = name.split('.').pop()?.toLowerCase() ?? '';
  const known: Record<string, string> = { tzx: 'TZX tape image', tap: 'TAP tape image', wav: 'WAV audio', scr: 'Spectrum screen', png: 'PNG image', bin: 'Binary data' };
  if (known[ext]) return [{ name: known[ext], extensions: [ext] }, { name: 'All files', extensions: ['*'] }];
  return [{ name: 'All files', extensions: ['*'] }];
}

/** Wrap a browser File (drag & drop, file input) as an OpenedFile; paths are unknown. */
export async function fileToOpened(f: File): Promise<OpenedFile> {
  return { name: f.name, bytes: new Uint8Array(await f.arrayBuffer()) };
}
