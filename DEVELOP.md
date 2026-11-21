# Developing vdfs-tool

This project is licensed under GPL-2.0-or-later; see [LICENSE](LICENSE).

Native VDFS4 packing is implemented for layout 2007. `pack` supports inline
and extent-backed regular files, directories, symlinks, and `none`, `zlib`,
`gzip`, `zstd`, and `lzo` compression.

`pack --layout 2006` writes VDFS4 layout 2006 descriptor and superblock records.
`pack --format vdfs1002` writes a native VDFS1002 image with catalog, index,
leaf, and extent-backed file records.

Unpacking refuses an existing VDFS4 or VDFS1002 output path. Packing rejects
`--layout 2006` with `--format vdfs1002` because VDFS1002 is not layout 2006.
Both formats provide `--list`, `--list --json`, `--audit`, `--extract`,
`--parent`, and selected-subtree restore behavior. VDFS4 additionally provides
`--manifest-dir` and AES key handling; VDFS1002 manifest/AES/RSA descriptor
support remains format-specific open work.

VDFS4 descriptor authentication can verify layout-5 and layout-6 RSA-PKCS1
v1.5 signature trailers with `--public-key <PEM>`. The verifier supports MD5,
SHA-1, SHA-256, RSA1024, and RSA2048 descriptor trailers. It does not provide
vendor trust enrollment or verify superblock signatures.

## Test and build

```sh
cargo test --locked                  # all tests (105 passing unit tests + 1 ignored survey, plus integration fixtures)
cargo build --release --locked       # ./target/release/vdfs-tool
cargo fmt --check                    # formatting (rustup component add rustfmt)
cargo clippy --locked --all-targets  # lints (rustup component add clippy)
```

The compressed-descriptor survey is ignored by default because it needs a real image. Run it with
`VDFS1002_SURVEY_IMAGE=<image> cargo test --release --locked -- --ignored survey_compressed_descriptors --nocapture`.
It lists the outcome of every file body and fails on any read, CRC, descriptor or decode error.

`unpack --patch-tif` preserves the original `IGzp` codec and pins the
live-compatible VDFS1002 descriptor to layout 4. The synthetic one-chunk table
uses the IGzp gap formula `144 + 16*count`, giving a 160-byte gap for count 1.
The reader accepts layouts 4 and 5. Regressions are:
`cargo test --locked --bin vdfs-tool synthetic_tif_igzp_uses_layout4`,
`cargo test --locked --bin vdfs-tool synthetic_tif_igzp_gap_is_crc_pinned`, and
`cargo test --locked --bin vdfs-tool accepts_layout5_descriptor`.
After writing a patched image, `--patch-tif` reopens it and requires the
TIFSDaemon round trip to return the exact guarded logical payload.

Focused decompression tests (layout 5, layout 6, digests and AES):
`cargo test --locked --bin vdfs-tool vdfs4::decompress`.
Layout-5 tests include valid descriptor-prefix hashes with 0/128/256-byte
signature blocks, corrupt stored hashes, and changed codec/size fields that
must fail the digest check before decompression. The unhashed size-mismatch
test continues to exercise the output-length check independently of hashing.

Layout-5 CRC regressions cover the table, optional signature block, stored
CRC, chunk-size field and padding, plus an unhashed payload change that the
metadata CRC must not reject. To cover both users of the shared CRC helper:
`cargo test --locked --bin vdfs-tool -- vdfs4::decompress vdfs1002 crc::tests`.
The CRC known-answer test lives in `src/crc.rs`; the VDFS1002 callers retain
their existing checksum behavior.

To repeat the real-image layout-5 check, build the release binary and run
`./target/release/vdfs-tool unpack /path/to/JazzM/section-6.bin -o /path/to/new-output`.
The sample hash, reconciled byte offsets and recorded results are in
[README.md](README.md#layout-5-descriptor-digest). The image is not committed;
the regression fixtures are synthesized in Rust.

Focused manifest and special-node tests:
`cargo test --locked --test manifest_mode`.
The node regression temporarily changes the `hello.txt` catalog record in
`small-zlib.vdfs` into each of FIFO, character-device, block-device and socket,
with empty and nonempty binary inline forks. Both extraction modes must write
regular files with the exact fork contents, and manifest mode must preserve
the original catalog record. No stored fixture or production code is changed.
Real-image validation and its limits are recorded in
[README.md](README.md#vdfs4-special-node-validation).

To inspect the same real nodes, run the release binary on the documented
JazzM sample with `--include root/dev` and
`--include root/usr/apps/org.tizen.services-fw/api/control`, using a fresh
output directory. Repeat with `--manifest-dir` pointing to a fresh metadata
directory; compare the node outputs and the corresponding `ENTRY` records.

Always pass `--locked`; `Cargo.lock` is pinned.

## Branch and release workflow

- Never commit directly to `main` or `develop`.
- Create a feature branch from `develop`.
- Submit every change as a pull request targeting `develop`.
- CI (`.github/workflows/test.yml`) runs formatting, tests, and Clippy for pull
  requests into `develop` and `main`.
- Pull requests into `develop` must come from a feature branch, never `main`
  or `develop`.
- Merge approved work to `develop` by pull request. Do not open pull requests
  that change the repository while the user has not explicitly approved PR
  creation.
- Merge `develop` into `main` manually only after all tests pass.
- Pull requests into `main` must come from `develop`.
- A push to `main` runs the release pipeline
  (`.github/workflows/release.yml`), builds all release targets, and creates a
  GitHub release automatically.
- The release version is the next patch version after the highest existing
  `vMAJOR.MINOR.PATCH` tag. It is applied to `Cargo.toml`, `Cargo.lock`, and
  the binaries without changing or committing the source version in place.


Coding rules (module layout, error handling, tests, comments) are in
[CONVENTIONS.md](CONVENTIONS.md).

## Layout

- `src/main.rs`: entry point. Dispatches to VDFS1002 or VDFS4 by image magic.
  An image that matches neither is rejected unless `--force` is given, in which
  case it is reported on stderr and sent to VDFS4.
- `src/cli.rs`: the shared `Cli` argument struct.
- `src/lzo1x.rs`: LZO1X codec, shared by VDFS4 and VDFS1002.
- `src/hex.rs`: shared hex helpers (`encode`; `decode` is test-only).
- `src/crc.rs`: shared kernel-style CRC-32 for VDFS1002 structures and VDFS4
  layout-5 compressed metadata (zero seed, no final inversion).
- `src/vdfs4/mod.rs`: the VDFS4 pipeline (`run`). Submodules are listed below.
- `src/vdfs4/decompress.rs`: compressed-file decoding.
  - Layout 2007 (`find_descriptor`, `decompress_blob`): the descriptor magic's
    first byte selects the digest (`DigestAlgo::from_magic`): `C` none, `I`
    MD5, `H` SHA-1, `h` SHA-256. Chunk digests are checked in extent-table
    order, then the descriptor digest (computed with bytes 24..28 zeroed).
  - Layout 2006 (`decompress_layout5`): `h` chunk hashes are checked, then the
    descriptor-prefix hash before decompression. The stored digest follows
    the chunk-hash table and is SHA-256 of descriptor bytes `[0, 16)`, starting
    at the magic. Both `C` and `h` also check the kernel CRC over the table
    through the full 32-byte descriptor, with its `[16, 20)` CRC field zeroed.
    This includes the hashed trailer and descriptor padding, but excludes
    chunk payloads. Signatures remain unverified. The prefix hash differs
    from layout 6's full, CRC-zeroed descriptor hash.
  - Codecs: `Zip`, `Gzp`, and `Zst` (VDFS4 only) plus `Lzo`. `Lzo` is raw
    LZO1X in `src/lzo1x.rs`, a wrapper over the `lzo` crate. Each chunk is
    bounded by the bytes still expected for the file. Tests use liblzo2
    reference streams. Zst is implemented for VDFS1002 descriptor-backed bodies.
- `src/vdfs1002/`: VDFS1002 reader (HawkM `platform.img`, `data_backup.img`).
  `mod.rs` re-exports the pieces: `format.rs` (on-disk structs and constants),
  `image.rs` (`Image`, catalog and fork access), `codec.rs` (descriptors and
  chunk decoding), `restore.rs` (tree restore), `tests.rs` (unit tests).
  `codec.rs` accepts descriptor markers `C` and `I`. `locate_chunk_table` finds
  the 16-byte `XT` table by the descriptor CRC, searching back up to
  `COMPRESSED_MAX_TABLE_GAP` (64 KiB) for padding before the descriptor.
  Written for this crate from the VDFS1002 format. `main.rs` dispatches to it
  when the image has a `VDFS1002` magic at a known superblock offset (1024 or
  1536, see `LAYOUTS` in `src/vdfs1002/format.rs`); the VDFS4 path is
  taken otherwise. An image that matches neither is rejected unless `--force`
  is given, which warns on stderr and continues with VDFS4. `src/vdfs1002/cli.rs` holds its CLI handler (`run`). It restores a
  tree (`restore`): the catalog walk (inode 3),
  forks, overflow extents (inode 5), Zip/Gzp descriptors, and hardlink and
  back-reference handling. Record modes are applied (`format::parse_mode`,
  field at `key_length + 32`; folder modes after all files are written).
  Symlink records (type bits `0o120000`) are created as symlinks, with the body
  as the target; links are made after folders exist and are never chmod-ed.
  Unix-only calls (`std::os::unix`: mode bits, symlinks, raw name bytes) are
  `cfg(unix)`-gated. On Windows, modes are ignored and symlink records fail
  with `Unsupported` (VDFS4 symlink creation is also Unix-only).
  It also provides the read-only modes `--list`
  (`--json`), `--audit`, and `--extract NAME -o FILE` (`select_file`, `audit`).
  `--include`, `--manifest-dir`, and `--aes-key` are rejected in VDFS1002 mode.
  The restore output directory must not exist. A bad extended-record CRC only
  warns.
  - Layout facts are documented in the module comments and tests. The
    overflow-extent record layout (inode 5) is not yet verified against a real
    image (see TODO.md). Lzo, Zlib/Gzip and Zstd are decoded.
  - Tests are synthetic: `build_image` in the `tests` module writes a complete
    image in memory, and the tests cover bad magic and checksums, loops,
    truncation, overflow, zlib/gzip/plain bodies, audit, selection, and tree
    restore.
- `src/vdfs4/manifest.rs`: manifest files and `ENTRY`/`LINK` rows. Names
  use the same checks as plain extraction.
- `src/vdfs4/unpacker.rs`: path selection (`--include`), safe output names, and
  symlink creation. Manifest mode and plain mode share this code path.
- `tests/manifest_mode.rs`: checks that manifest mode produces the same tree as
  plain mode on `small-zlib.vdfs`, including the symlink and its `LINK` row.
  Also covers regular-file output and raw manifest metadata preservation for
  all four special-node types using temporary fixture mutations.
- Other `src/vdfs4/` files: `disk` (on-disk structs), `superblock`, `btree`,
  `cattree`, `extents`, `snapshot`, `reader` (image access).

## Test fixtures

Tests build synthetic blobs in memory (`fixture`, `hashed_fixture`,
layout-5 helpers). No real Samsung images are used or stored in the repo.

`tests/fixtures/small-zlib.vdfs` (148 KB) is a `mkfs.vdfs`-built VDFS2007
image: zlib-compressed, read-only, unsigned. It holds `hello.txt`, an 8 KB
`zeros.bin`, `sub/deeper/n.txt`, and a symlink `link.txt -> hello.txt`.
`tests/fixture_zlib.rs` runs the built binary against it and checks the
SHA-256 (`9c2dc702…1f`), the file contents, the symlink target, and the exact
sets of regular files and directories.
`tests/fixture_encrypted.rs` checks `small-zlib-aes.vdfs` (SHA-256
`f71096dd…bb`).

`tests/fixtures/hashed-{md5-zlib,sha1-lzo,sha256-lzo}.vdfs` (205 KB each) are
`mkfs.vdfs` images built with `--hash-priv-rsa-key <key> --sign-all=1 -h <alg>`,
one per hash letter: `I` (MD5, Zip), `H` (SHA-1, Lzo) and `h` (SHA-256, Lzo).
They hold `hello.txt`, an 8 KB `zeros.bin`, and a 60 KB `rand.bin` (the LCG
bytes from `src/lzo1x.rs`, seed 1). `tests/fixture_hashed.rs` restores each one
and compares the three files byte for byte. The RSA key is random and was not
committed, so the images are not reproducible and their hashes are not pinned.
Regenerate them with `mkfs.vdfs` built as described below (the `RAND_bytes`
shim is not needed, because the RSA key is random anyway), and keep the
file contents and the `hashed-` names in sync with the test.

`tests/fixtures/codec-{lzo,zst}.vdfs` (205 KB each) are `mkfs.vdfs` images
built with `-c lzo` and `-c zstd` (VDFS4 layout 6, unhashed, unsigned) from the
same tree as the hashed fixtures, with the 60 KB `rand.bin`. Each holds a
descriptor tagged `Lzo` or `Zst`.

`tests/fixtures/codec-zhw.vdfs` (288 KB) is built by the upstream `vdfs-tools`
0010 `mkfs.vdfs` with a `-q` list containing `COMPRESS=zhw /zeros.bin`. Only
`zeros.bin` is compressed, because random bytes stay uncompressed under
`COMPRESS_RATIO`. It is the one codec fixture that comes from upstream output
rather than the `hsand` fork.

`tests/fixture_codecs.rs` checks each tag is present and restores all four files
byte for byte. These are synthetic: they show the codec path works, not that a
real Samsung image uses it. Regenerate them with
`MKFS=/path/to/hsand/mkfs.vdfs ZHW_MKFS=/path/to/upstream/mkfs.vdfs sh tests/fixtures/regenerate-codec.sh`.

Build the upstream `mkfs.vdfs` with
`sh tests/fixtures/build-upstream-mkfs.sh /path/to/vdfs4-tools.0010.tar.gz OUT`.
The bundled LZO 2.06 configure check and the bundled OpenSSL 1.0.1g build both
fail on gcc 15. The script compiles `lib/` and `mkfs/` by hand, with
`-fgnu89-inline` (the code relies on gnu89 `inline`) and `-no-pie`. It uses the
Debian `liblzo2-dev` and `libssl-dev` headers (fetched with `apt-get download`)
and the `libcrypto.a` shipped in the tarball. The LZO link is not exercised by
the `Zhw` fixture, which uses only zlib.

`tests/fixtures/lzo1x-long-distance.lzo` (20 KB) is a raw liblzo2 LZO1X stream
of 22000 bytes. It holds 20000 pseudo-random bytes followed by a copy of the
first 2000 bytes, which is a match 20000 bytes back (above 16 KiB, so it uses
the M4 opcode). `src/lzo1x.rs` decodes it and compares it with the same input,
rebuilt from the LCG in the test. It was produced with `lzo1x_1_compress`
(liblzo2 2.x) through `ctypes`. The 2000-byte copy compresses to about 96 bytes
where literals would cost 2000.

To rebuild them, build `mkfs.vdfs` from `hsand/vdfs4-linux` and run
`MKFS=/path/to/mkfs.vdfs sh tests/fixtures/regenerate.sh`. Build `mkfs` in
`vdfs-tools/` with `CFLAGS="-no-pie -fno-pie" make mkfs` (the bundled OpenSSL
is not PIE-compatible). Do not use `make mkfs CFLAGS+=...` on the command line:
that overrides the Makefile's `-I./include` and the build fails on `bitmap.h`. Then relink it with the random-number shim, so AES
nonces are fixed:

```sh
cc -c tests/fixtures/rand-fixed.c -o rand-fixed.o   # run from this repo
# In vdfs-tools/: rerun the link line from `make -n mkfs CFLAGS+=" -no-pie -fno-pie"`
# with `-no-pie -Wl,--wrap=RAND_bytes /path/to/rand-fixed.o` added, and output mkfs-det.vdfs.
MKFS=/path/to/vdfs-tools/mkfs-det.vdfs sh tests/fixtures/regenerate.sh
```

The script produces byte-identical images for a given `mkfs` build. It runs
`mkfs` under `faketime -f '@2020-01-01 00:00:00 x0'` (frozen clock) with
`tests/fixtures/fixed-ctime.c` preloaded and under `setarch -R` (ASLR off).
The shim is needed because `mkfs` stores each source file's ctime, and
libfaketime does not fake the `fstat`/`lstat64` calls `mkfs` makes. ASLR off
prevents uninitialized heap pointers from leaking into inode records. The
script uses a fixed build directory and relative paths, because `mkfs` embeds
its command line in the image. If you change the `mkfs` source or compiler, the
hashes change. Update them in `tests/fixture_zlib.rs` and
`tests/fixture_encrypted.rs`, then here, after regenerating.

`tests/fixtures/small-zlib-aes.vdfs` is the same tree encrypted with AES-128-CTR
(`mkfs --aes-key key.bin --encrypt-all -j 1`). The 16-byte key is the synthetic
string `0123456789abcdef`, written by the script and shared with
`tests/fixture_encrypted.rs`. It is not a secret. `-j 1` keeps chunks in table
order (multithreaded compression writes them in completion order). The tests
check the SHA-256 (`97b284a1…68fb`), the decrypted contents, the missing-key
error, and a wrong key.

Decryption is in `src/vdfs4/decompress.rs` (`decrypt_chunk`). A unit test checks it
against OpenSSL's AES-CTR output. Layout 2006 images reject the encrypted flag.
Signature trailers are still not verified (see TODO.md).

## Consumers

This crate is independent and has no dependencies on other projects. Consumers
that build it from a pinned commit and check the binary hash must re-pin after
any change under `src/`, because that changes the binary hash.
