// The checksums a tape file is known by, over the file's bytes as read: `core/src/hash.rs`.
import { fileHashesCore } from './core';

/** A file's CRC32, MD5 and SHA-1, in lowercase hex. */
export interface FileHashes {
  crc32: string;
  md5: string;
  sha1: string;
}

export function fileHashes(bytes: Uint8Array): FileHashes {
  const [crc32, md5, sha1] = fileHashesCore(bytes);
  return { crc32, md5, sha1 };
}
