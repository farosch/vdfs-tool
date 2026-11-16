# Open work

Only unresolved implementation and validation work belongs here. Recorded
corpus observations, reference-source findings and closed investigation results
are in [README.md](README.md#research-evidence-and-limits). Test commands and
fixture provenance are in [DEVELOP.md](DEVELOP.md).

## VDFS authentication

- Identify the actual JazzM device kernel's handling of `h` descriptors.
  The public Linux 3.10.30 tree rejects `h`, while the sampled image contains
  3,291 such descriptors with matching SHA-256 chunk digests. Resolve this
  discrepancy before claiming device-side verification.
- Establish and verify the layout-5 descriptor CRC. The checked
  [descriptor-prefix digest](README.md#layout-5-descriptor-digest) excludes
  the CRC and trailing fields; it does not validate them.
- Identify and verify the hashed-descriptor signature trailers: signing key,
  signed bytes, algorithm and byte order. Separate a supplied-key consistency
  check from authentication against a trusted vendor anchor. The recorded
  key searches have not established a verifier for these trailers.
- Find retail VDFS2006/2007 samples with `I`, `H` or `F`, or explicitly accept
  reference-tool-only coverage for `I`/`H`. `F` support needs a confirmed
  format and regression fixture before implementation. VDFS1002 `I` samples
  do not establish VDFS4 `I` behavior.

## Overflow extents

- Confirm VDFS1002 inode 5 records against an image with nonempty overflow
  extents. Synthetic tests and the earlier Python reader agree, but HawkM
  `platform.img` has zero overflow objects. The public VDFS4 record layout
  does not establish the VDFS1002 layout.
- Validate the VDFS4 `extents_resolve_fork` filter `iblock >= inline_end`
  against a real overflow tree and the reference kernel. The kernel does not
  use this heuristic.

## Remaining codec validation and support

- Validate LZO on a retail image and compare the kernel's handling of bytes
  after the end-of-stream marker. Reference streams and generated VDFS4
  images cover decoding, not device compatibility.
- Establish VDFS1002 `Zhw` and `Zst` layouts and implement supported decoding
  with fixtures. VDFS4 codec support does not establish their VDFS1002 form.
  No sampled retail VDFS4 image has yet supplied a valid `Lzo`, `Zst` or
  `Zhw` descriptor.
