# Open work

Only unresolved implementation and validation work belongs here. Test commands and
fixture provenance are in [DEVELOP.md](DEVELOP.md).

## Packing

- Validate large multi-chunk blobs on confirmed device layouts.
- Preserve VDFS1002 `IGzp` layout-4 table-gap and descriptor-CRC behavior.
  The synthetic `TIFSDaemon` patch now uses the real one-chunk gap
  `144 + 16*count` (160 bytes), round-trips exactly, and a live synthetic
  boot passed layout/CRC decompression. Validate the same behavior across
  additional real layout-4 objects and nontrivial chunk counts.
- Complete layout-5 real-image support. The reader now accepts layout-5
  descriptors and has synthetic coverage, but HawkM p22 survey evidence found
  45,285 layout-4 descriptors and no original layout-5 objects. Preserve layout
  5 support and obtain a confirmed layout-5 real-image fixture before claiming
  device compatibility.

## VDFS authentication

- VDFS1002 synthetic `TIFSDaemon` patching reaches a live RSA-signature
  mismatch for inode 61098. Preserve the modified payloads, but do not claim
  authorization or a device-side bypass. Vendor descriptor/public-key trust
  and signed layout-4 IGzp verification remain open.
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
- Establish VDFS1002 `Zhw` layout and implement supported decoding
  with fixtures. VDFS4 codec support does not establish their VDFS1002 form.
  The cached HawkM section-6 survey has no valid `Lzo`, `Zst` or `Zhw`
  descriptors: 44,924 Gzp descriptors, 361 nested-gzip payload cases, 2,230
  raw bodies and one unknown codec marker were observed.
