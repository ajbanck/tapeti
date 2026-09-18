// The TypeScript spectrum/charset.ts as it stood before the Rust core replaced
// it, kept as the reference implementation the core is tested against
// (test/core.test.ts). Not part of the app bundle, and not to gain features.
// ZX Spectrum character set to Unicode.
const BLOCKS = [' ', '▝', '▘', '▀', '▗', '▐', '▚', '▜', '▖', '▞', '▌', '▛', '▄', '▟', '▙', '█'];

export const TOKENS: string[] = [
  'RND', 'INKEY$', 'PI', 'FN', 'POINT', 'SCREEN$', 'ATTR', 'AT', 'TAB', 'VAL$', 'CODE', 'VAL', 'LEN', 'SIN', 'COS',
  'TAN', 'ASN', 'ACS', 'ATN', 'LN', 'EXP', 'INT', 'SQR', 'SGN', 'ABS', 'PEEK', 'IN', 'USR', 'STR$', 'CHR$', 'NOT',
  'BIN', 'OR', 'AND', '<=', '>=', '<>', 'LINE', 'THEN', 'TO', 'STEP', 'DEF FN', 'CAT', 'FORMAT', 'MOVE', 'ERASE',
  'OPEN #', 'CLOSE #', 'MERGE', 'VERIFY', 'BEEP', 'CIRCLE', 'INK', 'PAPER', 'FLASH', 'BRIGHT', 'INVERSE', 'OVER',
  'OUT', 'LPRINT', 'LLIST', 'STOP', 'READ', 'DATA', 'RESTORE', 'NEW', 'BORDER', 'CONTINUE', 'DIM', 'REM', 'FOR',
  'GO TO', 'GO SUB', 'INPUT', 'LOAD', 'LIST', 'LET', 'PAUSE', 'NEXT', 'POKE', 'PRINT', 'PLOT', 'RUN', 'SAVE',
  'RANDOMIZE', 'IF', 'CLS', 'DRAW', 'CLEAR', 'RETURN', 'COPY',
];

/** Name of a token byte (0xA5-0xFF), or 128k tokens for 0xA3/0xA4 when enabled. */
export function tokenName(code: number, basic128 = false): string | null {
  if (code >= 0xa5) return TOKENS[code - 0xa5];
  if (basic128 && code === 0xa3) return 'SPECTRUM';
  if (basic128 && code === 0xa4) return 'PLAY';
  return null;
}

/** Printable form of a single ZX character, for the dump / text views (no tokens expanded). */
export function zxChar(code: number, expandTokens = true): string {
  if (code === 0x60) return '£';
  if (code === 0x7f) return '©';
  if (code >= 0x20 && code < 0x7f) return String.fromCharCode(code);
  if (code >= 0x80 && code <= 0x8f) return BLOCKS[code - 0x80];
  if (code >= 0x90 && code <= 0xa4) return String.fromCharCode(0x2460 + (code - 0x90)); // circled letters for UDGs
  if (expandTokens && code >= 0xa5) return TOKENS[code - 0xa5];
  return '.';
}

/** Single-cell character for the hex dump ASCII column. */
export function dumpChar(code: number): string {
  if (code === 0x60) return '£';
  if (code === 0x7f) return '©';
  if (code >= 0x20 && code < 0x7f) return String.fromCharCode(code);
  if (code >= 0x80 && code <= 0x8f) return BLOCKS[code - 0x80];
  return '·';
}
