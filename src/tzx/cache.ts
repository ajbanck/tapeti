// Answers from the core, remembered per tape.
//
// Every call sends the blocks across the wasm boundary, so asking twice for the
// same tape costs twice. Blocks and the arrays holding them are immutable here —
// every edit makes a new array (see src/state/store.ts) — so a WeakMap keyed on
// the array itself cannot go stale, and the entry disappears with the tape.
//
// This is what `useMemo` in the components used to do, one layer lower, so two
// components asking the same question only pay once.
import { Block } from './types';

/** Memoize a function of a block list on the identity of that list. */
export function perTape<T>(fn: (blocks: Block[]) => T): (blocks: Block[]) => T {
  const cache = new WeakMap<Block[], T>();
  return (blocks) => {
    if (cache.has(blocks)) return cache.get(blocks) as T;
    const value = fn(blocks);
    cache.set(blocks, value);
    return value;
  };
}

/** As [`perTape`], for a function that also takes a small extra argument. */
export function perTapeWith<A, T>(fn: (blocks: Block[], arg: A) => T): (blocks: Block[], arg: A) => T {
  const cache = new WeakMap<Block[], Map<A, T>>();
  return (blocks, arg) => {
    let byArg = cache.get(blocks);
    if (!byArg) {
      byArg = new Map<A, T>();
      cache.set(blocks, byArg);
    }
    if (byArg.has(arg)) return byArg.get(arg) as T;
    const value = fn(blocks, arg);
    byArg.set(arg, value);
    return value;
  };
}
