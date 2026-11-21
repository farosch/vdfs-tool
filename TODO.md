# Open work

Only unresolved implementation and validation work belongs here. Test commands and
fixture provenance are in [DEVELOP.md](DEVELOP.md).

## Packing

- Validate large multi-chunk blobs on confirmed device layouts.
- Confirm layout-5 real-image compatibility. The reader accepts layout-5
  descriptors and has synthetic coverage, but HawkM p22 survey evidence found
  45,285 layout-4 descriptors and no original layout-5 objects. Preserve layout
  5 support and obtain a confirmed layout-5 real-image fixture before claiming
  device compatibility.

## VDFS authentication

- Identify the actual JazzM device kernel's handling of `h` descriptors.
  The public Linux 3.10.30 tree rejects `h`, while the sampled image contains
  3,291 such descriptors with matching SHA-256 chunk digests. Resolve this
  discrepancy before claiming device-side verification.
- Confirm VDFS4 superblock RSA verification against a signed retail fixture.
  Host-side RSA1024/2048 and SHA-1/SHA-256/MD5 verification plus local trust
  enrollment are implemented; preserve this only as a real-fixture/device-trust
  validation lead, not an implementation gap.

## Format-specific parity gaps

- Establish whether VDFS1002 stores AES-encrypted file bodies and, if so,
  implement `--aes-key` with confirmed encryption and nonce layouts.
- Define and validate a VDFS1002 manifest/xattr representation. VDFS4's
  `entries.tsv`, `catalog.bin`, and `xattrs.bin` formats do not establish
  VDFS1002 catalog or extended-attribute layout.
- Determine whether VDFS1002 descriptor signatures or superblock signatures
  are independently verifiable. VDFS4 RSA descriptor support does not establish
  VDFS1002 trust or vendor-key behavior.

## Overflow extents

- Confirm VDFS1002 inode 5 records against an image with nonempty overflow
  extents. Synthetic tests and the earlier Python reader agree, but HawkM
  `platform.img` and the cached HawkM section-6 image have zero overflow
  objects. The public VDFS4 record layout does not establish the VDFS1002 layout.
- Validate the VDFS4 `extents_resolve_fork` filter `iblock >= inline_end`
  against a real overflow tree and the reference kernel. The kernel does not
  use this heuristic.

## Remaining codec validation and support

- Validate LZO on a retail image and compare the kernel's handling of bytes
  after the end-of-stream marker. Reference streams and generated VDFS4
  images cover decoding, not device compatibility.
- Establish VDFS1002 `Zhw` layout and implement supported decoding
  with fixtures. VDFS4 codec support does not establish their VDFS1002 form.
  The cached HawkM section-6 survey has no valid `Lzo`, `Zst` or `Zhw`
  descriptors: 44,924 Gzp descriptors, 361 nested-gzip payload cases, 2,230
  raw bodies and one unknown codec marker were observed.
