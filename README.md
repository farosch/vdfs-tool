# vdfs-tool

A Rust utility for packing and unpacking **Samsung VDFS** filesystem images (`.img`) and
regular host directory. It supports VDFS4 with layout version **2006** or
**2007**, and VDFS1002 (for example the HawkM `platform.img` and
`data_backup.img`). The format is detected from the image. It decompresses
compressed files, reconstructs symbolic links (e.g. `bin -> usr/bin`), and can
extract only selected paths or write a metadata manifest alongside the files
(VDFS4).

Compressed files are verified when they carry digests. The descriptor magic's
first byte selects the algorithm for layout 2007 files: `C` (none), `I` (MD5),
`H` (SHA-1), or `h` (SHA-256). Per-chunk and descriptor digests must match, and
a mismatch fails the file rather than writing unverified data. Layout-2006
checks the metadata CRC for both `C` and `h` files; `h` additionally checks
chunk hashes and the SHA-256 of the 16-byte descriptor prefix. Signatures
remain unverified, and the CRC does not cover compressed payload bytes. The
MD5/SHA-1 paths have byte-exact coverage from `mkfs.vdfs`-generated fixtures,
but have not been confirmed against retail VDFS4 images.

# Current project status

Implementation status of VDFS types (checked = fully implemented and tested):

| Done | Type | Gap |
|------|------|-----|
| [x] | VDFS4 layout 5 (`VDFS2006`) | |
| [x] | VDFS4 layout 6 (`VDFS2007`) | |
| [x] | VDFS4 descriptor `C` (plain) | |
| [ ] | VDFS4 descriptor `h` (SHA-256) | Layout 6: chunk and descriptor digests verified on real images (e.g. KTM2D, 5118 `h` descriptors); layout 5: metadata CRC plus chunk and 16-byte descriptor-prefix digests verified on JazzM (3291 `h` descriptors); signature not verified (no vendor key); public kernel has no `h` verifier |
| [ ] | VDFS4 descriptor `I` (MD5) | Restored from `mkfs.vdfs` output (`--sign-all=1 -h md5`); no real image; signature not verified |
| [ ] | VDFS4 descriptor `H` (SHA-1) | Restored from `mkfs.vdfs` output (`--sign-all=1 -h sha1`); no real image; signature not verified |
| [ ] | VDFS4 descriptor `F` (forced) | Rejected by `DigestAlgo::from_magic` |
| [x] | VDFS4 codec `Zip` (zlib) | |
| [x] | VDFS4 codec `Gzp` (gzip) | |
| [x] | VDFS4 codec `Lzo` | Verified against liblzo2 reference streams and a synthetic `mkfs.vdfs` image (`codec-lzo.vdfs`); no valid layout 5/6 `Lzo` descriptor in the 13 cached VDFS4 images |
| [x] | VDFS4 codec `Zst` (zstd) | Synthetic `mkfs.vdfs` image (`codec-zst.vdfs`) only; no valid layout 5/6 `Zst` descriptor in the cached VDFS4 images |
| [x] | VDFS4 codec `Zhw` (zlib, 16-byte aligned chunks) | Verified against upstream `vdfs-tools` 0010 `mkfs.vdfs` output (`codec-zhw.vdfs`, decoded as zlib in `src/vdfs4/decompress.rs`); no valid layout 5/6 `Zhw` descriptor in the cached VDFS4 images |
| [x] | VDFS1002 superblock and detection | |
| [x] | VDFS1002 record 1 (folder) | |
| [x] | VDFS1002 record 2 (file) | |
| [x] | VDFS1002 record 3 (hard link) | |
| [x] | VDFS1002 record 5 (indirect link, back-reference) | |
| [x] | VDFS1002 codecs `Zip` and `Gzp` | Descriptor marker `C` or `I`; the chunk table is found by the descriptor CRC, with up to 64 KiB padding before the descriptor. Real HawkM image restores with 0 skipped |
| [ ] | VDFS1002 codec `Zst` | Rejected by `vdfs1002/codec.rs` (test `rejects_zstd_compressed_file` in `vdfs1002/tests.rs`) |
| [x] | VDFS1002 codec `Lzo` | Verified against liblzo2 reference streams only; no real image has used LZO |
| [ ] | VDFS1002 overflow extents (inode 5) | Layout not confirmed on a real image |

## Build and usage

```sh
cargo build --release --locked
./target/release/vdfs-tool unpack <image.img> -o <output_dir>
```

Pack a directory into a native layout-2007 VDFS4 image:

```sh
./target/release/vdfs-tool pack <directory> -o <image.img>
```
Optional compression:

```sh
./target/release/vdfs-tool pack <directory> -o <image.img> --compression zlib
```

Supported native packing codecs are `none`, `zlib`, `gzip`, and `zstd`.
`lzo` requires a confirmed native encoder and is currently rejected.

Arguments:

- `<INPUT>` — path to the VDFS image (e.g. `platform.img`).
- `-o, --output <OUTPUT>` — target directory for extraction. Defaults to
  `<image-stem>-extracted` in the current directory.
- `--include <PATH>` — extract only this path (repeatable, up to 1024 values).
  Paths are image paths such as `root/usr/bin/SWUCoreTV`. Without `--include`
  the whole image is extracted.
- `--aes-key <FILE>` — 16 raw bytes of AES-128 key for encrypted chunks
  (`ENCRYPTED` flag, layout 2007). Required when the image has encrypted chunks;
  the key is not validated, so a wrong key fails with a decompression or digest
  error.
- `--force` — extract with the VDFS4 path even when the image is not a
  recognized VDFS1002 or VDFS4 image. Without it, such an image is rejected.
- `--manifest-dir <DIR>` — write these files into `DIR` (must not already
  contain them):
  - `catalog.bin`: raw catalog records.
  - `entries.tsv`: tab-separated entry list.
  - `xattrs.bin`: extended attributes, with raw values in the `OSEAX01` format.

Manifest mode extracts the same files, directories and symbolic links as plain
extraction, with the same names and contents. It also records every catalog
entry in the manifest files above. Manifests contain raw extended-attribute
values, so review them before sharing.

VDFS4 device, FIFO and socket records are extracted as **regular files**, never
as active host devices, pipes or sockets. Empty forks produce empty files;
nonempty bodies follow the file extraction path. Manifest mode additionally
preserves the original catalog record, including its mode and device metadata.

## VDFS1002 images

`platform.img` and `data_backup.img` (HawkM) use the VDFS1002 format, detected by
the `VDFS1002` magic at offset 1024 or 1536 (the SDP1406 card layout). For these images the default action is a
restore into `-o`. The read-only modes are:

- `--list [--json] [--parent <ID>]` — print catalog records as
  `parent_id<TAB>object_id<TAB>type<TAB>name` (or a JSON document).
- `--audit` — print block-reach and file-fork consistency as JSON.
- `--extract <NAME> [--parent <ID>] -o <FILE>` — write one file record to a new
  file and print its size and SHA-256.

`--list`, `--audit` and `--extract` are mutually exclusive.

## Testing

```sh
cargo test --locked
```

GitHub Actions (`.github/workflows/test.yml`) runs this command on pull requests
to `main` and `develop`, and on pushes to `develop`. Pushes to `main` run the
release workflow (`.github/workflows/release.yml`), which builds Linux, macOS
and Windows archives.

## Research evidence and limits

These are recorded observations from firmware, reference sources and earlier
experiments, not facts established solely by this reader's implementation.
Counts belong to the named samples and runs; generated fixtures are not retail
firmware evidence. Remaining work is tracked in [TODO.md](TODO.md).

### Retail VDFS4 descriptors

- JazzM (`T-JZMDEUC`, 2016), `platform.img` unpacked with unixtract:
  `VDFS2006`, with `--sha256 -H` in the embedded mkfs command line. The
  layout-5 tally was 7,782 `CGzp` and 3,291 `hGzp` descriptors. There were no
  `H` descriptors; the single `I` byte match was followed by ASCII text,
  not a valid descriptor tail.
- Extraction of that 1.3 GB image with the layout-5 descriptor-prefix check
  enabled exited 0 and reported 52,052 files and 1,920 symlinks, with no
  chunk or descriptor-prefix digest mismatch. This establishes digest
  consistency, not signature verification.
- KTM2D, `upgrade.msd.plain/sections/8.bin`: 5,118 layout-6 `h` descriptors.
  Extraction exited 0 with no chunk or descriptor-digest mismatch. NKM2's
  recorded `h` count was 10,178.
- The broader survey included JazzM, KTM2D, NKM2, NKM, MSM, PTMC, PTMD and
  HKMF. The early summary called seven samples VDFS2007, but HKMF was later
  identified as VDFS1002; that summary is not a reliable family/layout
  inventory. In particular, HKMF's `I` descriptors are not VDFS4 evidence.
- A later scan covered 12 decrypted firmwares, all
  `platform.img`/`rocommon.img`/`secos.bin` under `image/out`. Valid layout-5/6
  hits were `C` and `h` only. Every `F` plus `Zip|Gzp|Lzo|Zst` hit had an
  invalid layout field at +6 and was attributed to compressed-data noise.
  No valid VDFS4 `I` or `H` was found either.
- The 13 cached `*.plain.zip` VDFS4 images contained no valid layout-5/6
  `Lzo`, `Zst` or `Zhw` descriptors. Raw `Zhw` byte matches had invalid
  layout fields. Absence in this corpus does not establish absence on devices.
- KSU2E, MSL and PTML were outside the decrypted scan. At the time,
  `T-KSU2EDEUC` could not be decrypted: unixtract's public list supplied
  `KantSU2E 2023` for `T-KSU2EC*`, not this family. All 13 local candidate
  key files matched public-list entries by value, and none decrypted KSU2E.
- In the recorded unhashed `C` corruption experiment, changing one data byte
  was accepted and changed extracted output without a digest error. Successful
  extraction is not an integrity guarantee for unhashed files.

### Layout-5 descriptor digest

Reconciled on 2026-11-16 against JazzM `section-6.bin` (1,337,647,104 bytes,
SHA-256 `6193e868624e02033a92578965342ee2dd8dfab67013c73d21409cb6f2c80f5c`).
All 3,291 `h` descriptor matches have the same rule. Let `M` be the byte offset
of the `h` in the descriptor magic:

| Bytes relative to `M` | Meaning |
|-----------------------|---------|
| `[0, 4)` | Magic and codec (`hGzp` in this sample) |
| `[4, 6)` | Chunk count |
| `[6, 8)` | Layout version |
| `[8, 16)` | Unpacked size |
| `[16, 20)` | CRC, not covered by this digest |
| `[20, 32)` | Chunk-size field and padding, not covered by this digest |
| `[-160, -128)` | Stored SHA-256 of bytes `[0, 16)` |
| `[-128, 0)` | Unverified signature block |

The old "bytes 8 to 23" notation used an origin eight bytes before the magic,
as in the layout-6 structure. Relative to that origin, the digest *ends* 120
bytes before it. Neither number describes a different hash input or a
120-byte signature. The canonical coordinates above start at the actual magic.
Hashing magic-relative `[8, 24)` or a full descriptor with the CRC zeroed
matched none of the 3,291 stored digests.

The Rust decoder finds the digest immediately after the chunk-hash table,
so it also handles the existing synthetic 0- and 256-byte signature cases;
the sampled JazzM descriptors use 128 bytes. It verifies the prefix before
decompression and rejects mismatches. Tests cover corrupted stored digests,
changed codec/size fields, and all three trailer sizes. A full extraction
with the new check succeeded; common-path regular files were byte-identical
to the saved historical extraction. That older extractor used different name
aliases and did not create symlinks or special-node placeholder files, so
the two trees are not identical.

This closes descriptor-digest reconciliation, not authentication of the
signature block. The excluded CRC and trailing fields are checked separately
by the metadata CRC described below.
The runtime and tests are Rust-only; no external parser is required.

### Layout-5 descriptor CRC

Confirmed on 2026-11-16 using the same JazzM image identified above. A bounded
Rust probe checked all 7,782 `C` and 3,291 `h` descriptors: all 11,073 matched
this rule, with no zero-CRC exceptions:

- Reflected CRC-32, polynomial `0xEDB88320`, initial value zero, no final
  inversion. This is the kernel convention, not the ordinary zlib CRC.
- Input starts at the first 16-byte `XT` chunk-table entry and ends after the
  full 32-byte descriptor. Descriptor bytes `[16, 20)` relative to its magic
  are treated as zero when computing the CRC, then compared with the stored
  little-endian value in that field.
- For `h`, this includes every chunk digest, the descriptor-prefix digest and
  the signature block between the table and descriptor. It excludes the
  compressed/stored payload before the table.

The public reference
[`vdfs4_calc_compext_table_crc`](https://github.com/hsand/vdfs4-linux/blob/8a692793c136154d212ca2da2bbbe32bbaae9800/linux/fs/vdfs4/inode.c#L3862)
chains the kernel CRC over the table and a descriptor copy with its CRC zeroed.
That corroborates the convention; the real-image results establish the
layout-5 descriptor size and field offsets. Candidates using only 24
descriptor bytes, omitting the hashed trailer, or using the ordinary zlib
initial/final convention did not match the `h` corpus.

The extractor now requires this CRC for layout-5 `C` and `h` before
decompression. The full image still extracts with exit 0, 52,052 files and
1,920 symlinks. Rust regressions cover altered table flags, signature bytes,
stored CRC, chunk-size field and descriptor padding with 0/128/256-byte
signature layouts. A separate test preserves the negative result: changing
unhashed stored payload bytes does not invalidate the metadata CRC.

CRC consistency is not RSA authentication or evidence of device-side
enforcement. In particular, including signature bytes in a CRC does not
verify the signature.

### Authentication reference evidence

- `hsand/vdfs4-linux`, `inode.c:4029-4060`, adds a signature trailer only
  for `I`/`H`/`h`; verification requires `hash_fn` (`inode.c:4145`).
  Not consuming a signature trailer for `C` matches that reference.
- The public `doitaljosh/linux-3.10.30-famhubv1` tree has the same Linux
  version as JazzM's uImage, but is not established as the exact device
  source. In `fs/vdfs/vdfs_layout.h:102-104,127`, the starts are `C`
  (plain), `H` (authenticated) and `F` (forced), and `VDFS_HASH_LEN` is 20.
  `inode.c:3636-3637` rejects other starts, including `h` and `I`, with
  `-EOPNOTSUPP`.
- That tree's RSA path hashes `(extents+1)*20` bytes and checks the following
  256-byte signature (`inode.c:3559-3580`), using
  `vdfs_verify_rsa_sha1_signature` (`super.c:755-772`) and the compiled-in
  key (`Secureboot.c:159-199`). It does not explain JazzM's `h` descriptors.
  Matching SHA-256 chunk hashes do not prove the device uses this verifier.
- Its `publicKey.h` contains ten RSA-2048 moduli with exponent 65537 for
  SDP1304/1406/1404/1412 variants, not SDP1501 as named by JazzM's DTB.
  None matched the JazzM superblock check. That sample's `sb_hash` region
  was zero, so there was no superblock signature there to test.
- The sampled ZIP metadata (`REL.xml`, `USBHardwareKeys.xml`, `info.txt`)
  supplied no public key or certificate. `REL.xml` named
  `RSAPublicProductKey` and `itemsPrivateRSAKey.txt` only as build-server paths.
  The JazzM partition scan (`secos.bin`, `secos_drv.bin`, `seret.bin`,
  `sign.bin`, `uImage`, `dtb.bin`, `ddr.init`) found no standard RSA key
  structure (PEM, SPKI, PKCS#1 or X.509), or exponent 65537 adjacent to a
  modulus. These are bounded negative searches, not proof that no key exists.
- JazzM's `secos.bin` was 1,040,384 bytes, with no recognized container magic
  and entropy 7.33 bits/byte: consistent with packing or encryption, not proof
  of either. The static investigation did not identify a verifier in
  `secos_drv.bin`; `seret.bin` was identified as the Linux boot stub. The
  secure-OS decryption route was closed as out of scope, not as verification
  of the trailers.
- Later signer searches on JazzM found no PKCS#1 v1.5 or RSASSA-PSS match
  under the device's RSA-1024 `usr/lib/key.pub`. Candidate padding scores
  across the known keys, upstream corpus and local candidates were at most
  0.46%, consistent with chance. These negative searches do not identify the
  signature block's signing key, signed bytes or byte order.

### HawkM VDFS1002 validation

`T-HKMFDEUC` `platform.img` (`section-6.bin`) and `data_backup.img` are
VDFS1002, not VDFS2006/2007. unixtract extracted `image/upgrade.msd` with its
public MSD10 `HawkM` entry. The recorded restored platform image had:

| Result | Count |
|--------|------:|
| Directories | 20,280 |
| Files | 46,535 |
| Hard-link names | 2,093 |
| Back-references | 66,814 |
| Nameless hard-link data bodies | 981 |
| Skipped | 0 |

Every nameless body was a self-parented, empty-name file record supplying data
to at least one hard-link record, not a failed restore. Comparison with the
earlier Python reader gave 136,703 listing rows, identical audit JSON and
identical extracted bytes for `--list`, `--audit` and `--extract`. This image
had zero overflow objects, so these results do not validate inode 5 records.

The initial compressed scan counted 3,213 `I`-marked descriptors and 42,061
`C`-marked descriptors. The `I` bodies had previously been emitted as raw gzip
despite a zero-skipped summary. The follow-up `survey_compressed_descriptors`
run used `section-6.bin` with the input SHA-256 recorded in `restore.json`:

- 47,516 file bodies: 46,535 restored files plus 981 nameless bodies.
- 45,285 compressed descriptors decoded with CRC verification: 44,924 ordinary
  outputs and 361 outputs that were themselves legitimate gzip payloads.
- 2,230 raw bodies had no marker or codec pattern in the last 64 KiB.
- Zero read, CRC, descriptor or decode errors. Table gaps had 54 distinct
  values, at most 4,768 bytes, within the reader's 64 KiB search window.
- The remaining flagged body was a 38-byte symlink target,
  `libug-IPv6_StatusUG-efl.so.15.09.03.01`: its `IPv6` text was a false positive,
  correctly restored raw. The earlier nine unmatched descriptors all decoded
  in this run with no CRC mismatch.

After the fix, no raw-gzip executables remained. `usr/bin/mkfs.vdfs` was an
`IGzp` body with its table 320 bytes before the descriptor; it restored as an
ARM EXEC ELF with interpreter `/lib/ld-linux.so.3`. The earlier apparent
"128 KB non-ELF prefix" came from treating concatenated members as one gzip
stream. Restored links included `bin/sh -> bash` and
`lib/ld-linux.so.3 -> ld-2.18.so`, and the restored mkfs ran under `qemu-arm`.

### VDFS4 special-node validation

Checked on 2026-11-16 using the same
[JazzM section-6 sample](#layout-5-descriptor-digest):

- Twelve character-device records under `root/dev`: `console`, `fb0`, `full`,
  `null`, `ptmx`, `random`, `tty`, `tty0`, `tty1`, `tty2`, `urandom` and `zero`.
- One FIFO: `root/usr/apps/org.tizen.services-fw/api/control`.
- All thirteen forks declare zero bytes. Plain and manifest extraction both
  produced empty regular files, not host special nodes or symlinks.
- Every emitted manifest catalog record matched the saved original record
  byte-for-byte, retaining the original node type and device metadata.

The Rust regression in [tests/manifest_mode.rs](tests/manifest_mode.rs) covers
FIFO, character-device, block-device and socket records, each with an empty
and a nonempty binary inline body, in both modes. It checks regular-file
output, exact contents, tree parity and unchanged manifest catalog bytes.
The fixture is a temporary, in-memory mutation of `small-zlib.vdfs`; the
committed fixture is unchanged, and no privileged node creation is needed.
The JazzM sample has no block-device or socket records, so those types have
synthetic coverage only.

### Reference-tool and source findings

- `hsand/vdfs4-linux` mkfs writes VDFS2007. Unsigned, uncompressed and zlib
  read-only fixtures round-tripped byte-exact in manifest mode.
  `mkfs.vdfs -s <key>` changes only the superblock signature. Per-file hashing
  requires `--hash-priv-rsa-key <key> --sign-all=1 -h {md5,sha1,sha256}`;
  otherwise `is_need_sign` signs only executables. `--sign-all` requires an
  argument and can consume the next option if written without `=1`.
  The generated `IZip`, `HLzo` and `hLzo` fixtures restore byte-exact.
- Upstream `vdfs-tools` 0010 defines `Zhw` in `include/vdfs4_layout.h:88`,
  decodes it with zlib (`lib/decompress.c:133-135,353`), and pads hardware
  chunks to 16-byte alignment (`lib/compress.c:231-237`). Its
  `COMPRESS=zhw /zeros.bin` fixture restores byte-exact. The header also
  defines `VDFS4_HW_COMPR_PAGE_PER_CHUNK = 32`; this reader does not use it.
- VDFS4 `ILINK` rows are reverse-parent indexes, not listed children.
  Kernel `readdir` skips them (`inode.c:195`), while `lookup` and
  `__vdfs4_iget` follow them. mkfs writes one for each object whose
  `object_id != parent_id` (`vdfs-tools/lib/cattree.c:345-352`).
- `HinTak/vdfs-driver` (`vdfs4_layout.h`, `extents.c`) describes a 40-byte
  overflow record: 8-byte generic key, object ID at +8, iblock at +16,
  extent begin at +24, length at +32. This corroborates VDFS4 only; that
  source does not establish VDFS1002 inode 5.
- Upstream `gtors/vdfs-unpack` PR #1's btree refactor did not compile against
  this fork's callers. PR #2 omitted chunk/descriptor verification and
  dropped the single-stream fallback, so it was not taken as written. Its
  `I`/`H` digest mapping was adopted in `c378669`; verification and fallback
  were retained.

Fixture provenance, commands and reproducibility controls are documented in
[DEVELOP.md](DEVELOP.md#test-fixtures). Earlier runs differed by about 48 bytes;
the diagnosed sources were the real superblock clock (offsets `0x214`/`0x218`),
per-file ctime, embedded paths, uninitialized inode pointers, AES randomness
and multithreaded chunk order. With the documented controls, regenerating the
plain and AES fixtures twice produced byte-identical images. This does not
apply to the hashed fixtures, whose signing key was randomly generated.

### Small-area and tiny-file storage: investigation closed pending evidence

The reference documentation describes files smaller than a chunk stored in a
small area, addressed through the extent tree, with `tiny`/`tinysmall` mount
options. No usable implementation was found in the inspected references:

- `vdfs-tools` 0010 declares `init_small`,
  `test_and_set_small_area_bitmap_bit` and `copy_small_file_to_image` in
  `include/vdfs_tools.h`, but has no `small.c` in `lib/`, `mkfs/` or `unpack/`.
- Tiny-file handling is commented out in `unpack/unpack.c` (around lines 819
  and 1388) and `mkfs/mkfs.c` (419, 681, 872); `TINY_FILE` is never defined.
- `HinTak/vdfs-driver`, `snapshot.c` (around line 1095), says there is "no such
  thing as ... small files bitmap anymore". Mount-option names remain in
  `vdfs4.h` and `vdfs.txt:36-38`.

There is no small-area reader here; files using such storage could be read
incorrectly or skipped. Implementation was deferred rather than guessed.
Reopen only with a real tiny/small extent or an executable format reference.

## License

This project is distributed under the Apache License, Version 2.0. See the
[LICENSE](LICENSE) file.

## Related resources

- https://github.com/synacktiv/samsung-q60t-exploit
- https://github.com/bugficks/msddecrypt
- https://github.com/hsand/vdfs4-linux
- https://github.com/HinTak/vdfs-tools
- https://github.com/HinTak/vdfs-driver
- https://github.com/theubusu/unixtract
- https://github.com/gtors/vdfs-unpack (historical upstream PR reference)
- https://sourceforge.net/projects/vdfs/
