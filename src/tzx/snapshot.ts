// Snapshots (.z80, .sna) turned into a tape that loads them: a BASIC loader
// followed by the memory as packed turbo blocks. The reading and the loader are
// the Rust core in core/ (snapshot.rs), which has the whole story.
import { ParsedTape } from './types';
import { snapshotInfoCore, snapshotToTapeCore } from './core';

export type SnapshotKind = 'z80' | 'sna';

/** Bits per second of each loading speed; the first is ROM timing in standard blocks. */
export const SNAPSHOT_SPEEDS = [1500, 2250, 3000, 6000];
export const DEFAULT_SNAPSHOT_SPEED = 2;
export const SNAPSHOT_SPEED_NAMES = ['Normal', 'High', 'Turbo', 'Ludicrous'];
export const SNAPSHOT_MACHINES = ['48K', '128K', 'Scorpion 256K'];
export const SCREEN_BYTES = 6912;

export interface SnapshotInfo {
  /** Index into SNAPSHOT_MACHINES. */
  machine: number;
  border: number;
  screen: Uint8Array;
}

export interface SnapshotOptions {
  speed: number;
  border: number;
  /** Pack the screen's page even when the packed bytes show on screen while they load. */
  compressAll: boolean;
  /** A 6912 byte loading screen to show instead of the snapshot's own. */
  screen: Uint8Array | null;
}

/** By extension: neither format has a signature. */
export function snapshotKind(name: string): SnapshotKind | null {
  const ext = name.split('.').pop()?.toLowerCase();
  return ext === 'z80' || ext === 'sna' ? ext : null;
}

const kindNumber = (kind: SnapshotKind) => (kind === 'sna' ? 1 : 0);

/** Throws when the file is not a snapshot the core can read. */
export function snapshotInfo(file: Uint8Array, kind: SnapshotKind): SnapshotInfo {
  return snapshotInfoCore(file, kindNumber(kind));
}

export function snapshotToTape(file: Uint8Array, kind: SnapshotKind, name: string, opts: SnapshotOptions): ParsedTape {
  return snapshotToTapeCore(file, kindNumber(kind), name, opts);
}
