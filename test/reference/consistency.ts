// The TypeScript consistency.ts as it stood before the Rust core replaced it, kept as
// the reference implementation the core is tested against (test/core.test.ts).
// Not part of the app bundle, and not to gain features. See test/reference/parser.ts.
import { Block, isUnknown, isDataBlock, Issue } from '../../src/tzx/types';
import { bitsPerSymbol } from '../../src/tzx/parser';
import { detectContent, blockBody } from './content';

/** "Check consistency": structure, useless blocks, infinite loops, cross nesting. */
/** `base` is the number shown for the first block (1, or 0 with zero-based numbering). */
export function checkConsistency(blocks: Block[], base = 1): Issue[] {
  const issues: Issue[] = [];
  const n = blocks.length;
  if (n === 0) return [{ block: -1, severity: 'info', message: 'Tape is empty' }];

  // Structural nesting of groups and loops (static)
  const stack: { kind: 'group' | 'loop'; at: number }[] = [];
  blocks.forEach((b, i) => {
    if (isUnknown(b)) {
      issues.push({ block: i, severity: 'warning', message: `Unknown or deprecated block ID ${b.id.toString(16).toUpperCase()}` });
      return;
    }
    switch (b.id) {
      case 0x21:
        if (stack.some((s) => s.kind === 'group')) issues.push({ block: i, severity: 'error', message: 'Nested group (groups cannot be nested)' });
        stack.push({ kind: 'group', at: i });
        break;
      case 0x22: {
        const top = stack[stack.length - 1];
        if (!top) issues.push({ block: i, severity: 'error', message: 'Group end without group start' });
        else if (top.kind !== 'group') issues.push({ block: i, severity: 'error', message: 'Group end crosses an open loop' });
        else stack.pop();
        break;
      }
      case 0x24:
        if (b.count === 0) issues.push({ block: i, severity: 'warning', message: 'Loop with 0 repetitions is useless' });
        else if (b.count === 1) issues.push({ block: i, severity: 'warning', message: 'Loop with 1 repetition is useless' });
        if (stack.some((s) => s.kind === 'loop')) issues.push({ block: i, severity: 'error', message: 'Nested loop (loops cannot be nested)' });
        stack.push({ kind: 'loop', at: i });
        break;
      case 0x25: {
        const top = stack[stack.length - 1];
        if (!top) issues.push({ block: i, severity: 'error', message: 'Loop end without loop start' });
        else if (top.kind !== 'loop') issues.push({ block: i, severity: 'error', message: 'Loop end crosses an open group' });
        else stack.pop();
        break;
      }
      case 0x23: {
        const t = i + b.offset;
        if (b.offset === 0) issues.push({ block: i, severity: 'error', message: 'Jump to itself (infinite loop)' });
        else if (t < 0 || t > n) issues.push({ block: i, severity: 'error', message: `Jump target ${t + base} is outside the tape` });
        break;
      }
      case 0x26:
        if (b.offsets.length === 0) issues.push({ block: i, severity: 'warning', message: 'Call sequence with no calls is useless' });
        b.offsets.forEach((o, k) => {
          const t = i + o;
          if (o === 0) issues.push({ block: i, severity: 'error', message: `Call ${k + 1} calls itself` });
          else if (t < 0 || t >= n) issues.push({ block: i, severity: 'error', message: `Call ${k + 1} target ${t + base} is outside the tape` });
        });
        break;
      case 0x28:
        b.entries.forEach((e, k) => {
          const t = i + e.offset;
          if (t < 0 || t >= n) issues.push({ block: i, severity: 'error', message: `Selection ${k + 1} target ${t + base} is outside the tape` });
        });
        break;
      case 0x11:
      case 0x14:
        if (b.usedBits < 1 || b.usedBits > 8) issues.push({ block: i, severity: 'error', message: `Used bits in last byte is ${b.usedBits}, must be 1-8` });
        if (b.data.length === 0) issues.push({ block: i, severity: 'warning', message: 'Data block is empty' });
        break;
      case 0x15:
        if (b.usedBits < 1 || b.usedBits > 8) issues.push({ block: i, severity: 'error', message: `Used bits in last byte is ${b.usedBits}, must be 1-8` });
        if (b.tstates === 0) issues.push({ block: i, severity: 'error', message: 'T-states per sample is 0' });
        break;
      case 0x10:
        if (b.data.length === 0) issues.push({ block: i, severity: 'warning', message: 'Data block is empty' });
        else if (b.data.length >= 2) {
          let c = 0;
          for (let k = 0; k < b.data.length - 1; k++) c ^= b.data[k];
          if (c !== b.data[b.data.length - 1]) issues.push({ block: i, severity: 'warning', message: 'Checksum byte does not match data' });
        }
        break;
      case 0x12:
        if (b.count === 0 || b.pulseLen === 0) issues.push({ block: i, severity: 'warning', message: 'Pure tone with zero length is useless' });
        break;
      case 0x13:
        if (b.pulses.length === 0) issues.push({ block: i, severity: 'warning', message: 'Pulse sequence with no pulses is useless' });
        break;
      case 0x19: {
        if (b.totp > 0) {
          if (b.pilotSymbols.length === 0) issues.push({ block: i, severity: 'error', message: 'Pilot stream present but no pilot symbols defined' });
          if (b.pilotStream.length !== b.totp) issues.push({ block: i, severity: 'error', message: `Pilot stream has ${b.pilotStream.length} runs but TOTP is ${b.totp}` });
          for (const r of b.pilotStream) if (r.symbol >= b.pilotSymbols.length) {
            issues.push({ block: i, severity: 'error', message: `Pilot stream references undefined symbol ${r.symbol}` });
            break;
          }
          for (const s of b.pilotSymbols) if (s.pulses.length > b.npp) {
            issues.push({ block: i, severity: 'error', message: 'A pilot symbol has more pulses than NPP allows' });
            break;
          }
        }
        if (b.totd > 0) {
          if (b.dataSymbols.length === 0) issues.push({ block: i, severity: 'error', message: 'Data stream present but no data symbols defined' });
          else {
            const need = Math.ceil((bitsPerSymbol(b.dataSymbols.length) * b.totd) / 8);
            if (b.data.length < need) issues.push({ block: i, severity: 'error', message: `Data stream too short: ${b.data.length} bytes, ${need} needed for ${b.totd} symbols` });
          }
          for (const s of b.dataSymbols) if (s.pulses.length > b.npd) {
            issues.push({ block: i, severity: 'error', message: 'A data symbol has more pulses than NPD allows' });
            break;
          }
        }
        break;
      }
      case 0x33:
        if (b.entries.length === 0) issues.push({ block: i, severity: 'warning', message: 'Hardware block with no entries' });
        break;
      case 0x32:
        if (b.entries.length === 0) issues.push({ block: i, severity: 'warning', message: 'Archive info block with no entries' });
        break;
    }
  });
  // Data blocks whose length differs from what the preceding ROM header announces.
  blocks.forEach((b, i) => {
    if (!isDataBlock(b)) return;
    const c = detectContent(blocks, i);
    if (c.expectedLength === null) return;
    const len = blockBody(b.data, c.skipFlag, c.skipChecksum).length;
    if (len < c.expectedLength) issues.push({ block: i, severity: 'warning', message: `Data is ${c.expectedLength - len} bytes shorter than its header says (${len} of ${c.expectedLength})` });
    else if (len > c.expectedLength) issues.push({ block: i, severity: 'warning', message: `Data is ${len - c.expectedLength} bytes longer than its header says (${len}, header ${c.expectedLength})` });
  });
  for (const s of stack) {
    issues.push({ block: s.at, severity: 'error', message: s.kind === 'group' ? 'Group is never closed' : 'Loop is never closed' });
  }

  // Dynamic flow: simulate with a visited-state set to detect infinite loops and calls w/o return.
  const flow = simulateFlow(blocks);
  for (const f of flow) issues.push(f);
  return issues.sort((a, b) => a.block - b.block);
}

function simulateFlow(blocks: Block[]): Issue[] {
  const issues: Issue[] = [];
  const n = blocks.length;
  const loopStack: { start: number; remaining: number }[] = [];
  const callStack: { block: number; next: number }[] = [];
  const seen = new Set<string>();
  let i = 0;
  let steps = 0;
  while (i >= 0 && i < n) {
    if (++steps > 200000) {
      issues.push({ block: i, severity: 'error', message: 'Playback does not terminate (too many steps)' });
      break;
    }
    const b = blocks[i];
    if (isUnknown(b)) {
      i++;
      continue;
    }
    if (b.id === 0x23) {
      const key = `${i}:${callStack.map((c) => c.block + '/' + c.next).join(',')}:${loopStack.map((l) => l.start + '/' + l.remaining).join(',')}`;
      if (seen.has(key)) {
        issues.push({ block: i, severity: 'error', message: 'Infinite loop: this jump is reached again in the same state' });
        break;
      }
      seen.add(key);
      i += b.offset;
      continue;
    }
    if (b.id === 0x24) {
      loopStack.push({ start: i, remaining: b.count });
    } else if (b.id === 0x25) {
      const l = loopStack[loopStack.length - 1];
      if (l) {
        if (--l.remaining > 0) {
          i = l.start + 1;
          continue;
        }
        loopStack.pop();
      }
    } else if (b.id === 0x26) {
      if (callStack.length >= 16) {
        issues.push({ block: i, severity: 'error', message: 'Call nesting too deep (recursive calls?)' });
        break;
      }
      if (b.offsets.length > 0) {
        callStack.push({ block: i, next: 0 });
        i += b.offsets[0];
        continue;
      }
    } else if (b.id === 0x27) {
      const c = callStack[callStack.length - 1];
      if (!c) {
        issues.push({ block: i, severity: 'error', message: 'Return reached without a pending call' });
      } else {
        c.next++;
        const offs = (blocks[c.block] as any).offsets as number[];
        if (c.next < offs.length) i = c.block + offs[c.next];
        else {
          callStack.pop();
          i = c.block + 1;
        }
        continue;
      }
    } else if (b.id === 0x20 && b.pause === 0) {
      break;
    }
    i++;
  }
  if (callStack.length > 0) {
    issues.push({ block: callStack[callStack.length - 1].block, severity: 'error', message: 'Call sequence never returns (end of tape reached inside a call)' });
  }
  return issues;
}
