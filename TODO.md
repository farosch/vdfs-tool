# Open work

Only unresolved implementation and validation work belongs here. Test commands and
fixture provenance are in [DEVELOP.md](DEVELOP.md).

## Packing

- Add layout-2006 packing with the 32-byte descriptor format.
- Add overflow-extent B-tree records for files requiring more than nine inline
  extents, hard-link records, xattrs, and complete mode/time metadata.
- Validate large multi-chunk blobs on confirmed device layouts.

## VDFS authentication

- Identify the actual JazzM device kernel's handling of `h` descriptors.
  The public Linux 3.10.30 tree rejects `h`, while the sampled image contains
  3,291 such descriptors with matching SHA-256 chunk digests. Resolve this
  discrepancy before claiming device-side verification.
- Implement and validate VDFS4 superblock RSA signature verification and a
  vendor trust anchor. Supplied descriptor-public-key verification is complete.
- Find retail VDFS2006/2007 samples with `F`. `F` support needs a confirmed
  format and regression fixture before implementation. VDFS1002 `I` samples
  do not establish VDFS4 `I` behavior.

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
- Establish VDFS1002 `Zhw` and `Zst` layouts and implement supported decoding
  with fixtures. VDFS4 codec support does not establish their VDFS1002 form.
  The cached HawkM section-6 survey has no valid `Lzo`, `Zst` or `Zhw`
  descriptors: 44,924 Gzp descriptors, 361 nested-gzip payload cases, 2,230
  raw bodies and one unknown codec marker were observed.
