# `.vni` format, version 1

A `.vni` file is a version-controlled image. It holds a **base image** and an
append-only chain of **commits**, each with an author, timestamp, message and
the pixel changes it introduces. All integers are little-endian.

## Layout

```
file    = magic version record+
magic   = 89 56 4E 49 0D 0A 1A 0A      ("\x89VNI\r\n\x1a\n")
version = u16                          (currently 1)
record  = tag:u8  length:u64  payload  (length counts payload bytes only)
```

Tag `0x43` (`C`) is the only record type in v1. A reader must reject unknown
tags. The first record is the base commit; every later record is appended to the
end of the file, so committing never rewrites existing bytes.

## Commit record payload

| Field     | Size | Notes |
|-----------|------|-------|
| id        | 32   | SHA-256 of every byte that follows in this payload |
| parent    | 32   | id of the previous commit; all zeros for the base |
| tree      | 32   | SHA-256 of `width:u32 height:u32 rgba bytes` of the resulting image |
| timestamp | 8    | i64, Unix seconds, UTC |
| kind      | 1    | `0` = full snapshot, `1` = delta |
| width     | 4    | u32, dimensions of the image *after* this commit |
| height    | 4    | u32 |
| name      | 2+n  | u16 length, UTF-8 author name |
| email     | 2+n  | u16 length, UTF-8 author email |
| message   | 4+n  | u32 length, UTF-8 |
| data      | 8+n  | u64 length, payload bytes (see below) |

Because `id` covers `parent` and `tree`, the chain is tamper-evident the same
way a Git history is: changing any commit changes its id and breaks every
descendant's `parent` link.

## Byte order on disk

Fields appear in exactly the order below, with no padding or alignment.
Offsets are relative to the start of the file for the header, and relative to
the start of the record for each commit.

```
FILE
  0x00  8   magic         89 56 4E 49 0D 0A 1A 0A
  0x08  2   version       u16
  0x0A  ..  record 0      (base commit)
        ..  record 1      (first edit)
        ..  record N      (HEAD, always last)

RECORD (framing)
  +0    1   tag           0x43 'C'
  +1    8   length        u64, size of the payload that follows
  +9    ..  payload       exactly `length` bytes

PAYLOAD (commit)
  +0    32  id            SHA-256 of everything after this field
  +32   32  parent        previous commit id, or 32 zero bytes for the base
  +64   32  tree          SHA-256 of the resulting pixels
  +96   8   timestamp     i64 Unix seconds
  +104  1   kind          0 snapshot, 1 delta
  +105  4   width         u32
  +109  4   height        u32
  +113  2   name_len      u16
  +115  n   name          UTF-8
        2   email_len     u16
        n   email         UTF-8
        4   message_len   u32
        n   message       UTF-8
        8   data_len      u64
        n   data          PNG bytes (kind 0) or zlib stream (kind 1)
```

The record length is therefore `113 + 2 + name_len + 2 + email_len + 4 +
message_len + 8 + data_len`. Readers walk the file by reading a 9-byte frame,
then skipping `length` bytes to the next frame, until end of file. A file that
ends partway through a frame or payload is corrupt.

### Worked example

A 1x1 black pixel with author `Ada <a@x>` and message `base`, followed by one
edit by `Bo <b@x>` that raises red to 10. The file is 386 bytes. Hash bytes
are abbreviated in the annotations.

```
00000000: 8956 4e49 0d0a 1a0a 0100 43d1 0000 0000  magic | version=1 | tag 'C' | length=0xD1 (209) ...
00000010: 0000 0089 46ab 4dd3 c482 bae5 6cee 92d1  ... length | id begins at 0x13 (89 46 ab ...)
00000020: cde0 e7bb d51a 5bfe 2afb 6fc6 7088 c225  id
00000030: 7ad3 df00 0000 0000 0000 0000 0000 0000  id ends 0x32 | parent begins 0x33 (all zeros: base)
00000040: 0000 0000 0000 0000 0000 0000 0000 0000  parent
00000050: 0000 00cb c8f9 ff69 1519 8f61 83d1 b67b  parent ends 0x52 | tree begins 0x53
00000060: 8947 8ecb c1aa 857a 740c 29e2 d5f0 fe95  tree
00000070: 2e20 90e5 7ed2 6a00 0000 0000 0100 0000  tree ends 0x72 | timestamp 0x73 (8 bytes) | kind 0x7B=00 | width=1
00000080: 0100 0000 0300 4164 6103 0061 4078 0400  height=1 | name_len=3 "Ada" | email_len=3 "a@x" | message_len=4 ...
00000090: 0000 6261 7365 4600 0000 0000 0000 8950  ... "base" | data_len=0x46 (70) | data begins 0x9E: PNG signature
000000a0: 4e47 0d0a 1a0a 0000 000d 4948 4452 0000  PNG (IHDR ...)
000000b0: 0001 0000 0001 0806 0000 001f 15c4 8900  PNG
000000c0: 0000 0d49 4441 5478 da63 6160 60f8 0f00  PNG (IDAT ...)
000000d0: 0118 0104 7d28 2468 0000 0000 4945 4e44  PNG
000000e0: ae42 6082 4395 0000 0000 0000 00bd ab59  PNG ends 0xE3 | record 1: tag 'C' @0xE4 | length=0x95 (149) | id begins 0xED
000000f0: 382c fdd6 a659 e888 f5ae 2b5e c737 0476  id
00000100: 5ec2 eea3 c870 36ac 0589 7f24 3e89 46ab  id ends 0x10C | parent begins 0x10D = base id (89 46 ab ...)
00000110: 4dd3 c482 bae5 6cee 92d1 cde0 e7bb d51a  parent
00000120: 5bfe 2afb 6fc6 7088 c225 7ad3 dfd8 e49f  parent ends 0x12C | tree begins 0x12D
00000130: 4fe6 f54c 26a2 9b78 cdb8 eec6 95fe 1a6d  tree
00000140: ca72 fb3a 2629 6ea7 fa19 077c 13e5 7ed2  tree ends 0x14C | timestamp begins 0x14D
00000150: 6a00 0000 0001 0100 0000 0100 0000 0200  timestamp | kind=01 (delta) | width=1 | height=1 | name_len=2
00000160: 426f 0300 6240 7804 0000 0065 6469 740b  "Bo" | email_len=3 "b@x" | message_len=4 "edit" | data_len=0x0B ...
00000170: 0000 0000 0000 0078 da63 60e4 0200 000f  ... (11) | data begins 0x177: zlib stream (78 da ...)
00000180: 000c                                     end of file
```

The 11-byte delta inflates to `00 01 0A`: skip 0 bytes, 1 literal byte, `0x0A`.
That adds 10 to the first byte (red) of the parent pixel. Nothing else is
stored, and the trailing zeros are implied.

## Pixel model

Images are normalised to 8-bit RGBA (4 bytes per pixel, row-major, top-left
origin). Any input format is converted on import, so revisions are reproduced
bit for bit in that model. Lossy sources such as JPEG are frozen as decoded.

## Data

- **kind 0, snapshot:** a PNG file (RGBA8). The base is always a snapshot.
  A snapshot is also used when dimensions change, or when it is smaller than
  the delta.
- **kind 1, delta:** a zlib stream holding the difference between this
  commit's pixels and its parent's, taken per byte with wrapping subtraction.
  The difference is stored sparsely as repeated segments of
  `zero_run:varint  literal_len:varint  literal bytes`.
  Runs of zeros between segments are skipped, and trailing zeros are implicit.
  Varints are LEB128 (7 bits per byte, high bit = continue).
  To apply: add each literal byte to the parent byte at the same offset,
  wrapping at 256.

## Validation

A reader must check the magic and version, the record framing, each commit's
`id`, and that each `parent` equals the previous commit's `id`. A full verify
additionally rebuilds every revision and compares its hash with `tree`.

## Not in v1

Branches, merges, and tags (a commit has a single parent), per-region
snapshots (keyframes are only inserted when a snapshot is smaller), and
signatures.
