# vdfs-tool

`vdfs-tool` is a Rust command-line utility for inspecting, unpacking, and
packing Samsung VDFS filesystem images.

It supports both VDFS4 and VDFS1002 images, with native packing support for
each format.

## Supported formats

| Format | Layout | Unpack | Pack |
|---|---|:---:|:---:|
| VDFS4 | 2006 | Yes | Yes |
| VDFS4 | 2007 | Yes | Yes |
| VDFS1002 | Native | Yes | Yes |

### Feature parity

| Feature | VDFS4 | VDFS1002 |
|---|:---:|:---:|
| Full directory restore | Yes | Yes |
| Regular files | Yes | Yes |
| Directories | Yes | Yes |
| Symlinks | Yes | Yes |
| Hard links | Yes | Yes |
| Inline forks | Yes | Yes |
| Extent-backed files | Yes | Yes |
| Selected-path extraction | Yes | Yes |
| Manifest/xattr export | Yes | No |
| AES encrypted chunks | Yes | No |
| RSA descriptor verification | Yes | No |
| Catalog listing and JSON audit | Yes | Yes |
| Single-file extraction and `--parent` | Yes | Yes |
| Layout-specific packing | 2006 and 2007 | Native only |
| Compression packing | None, zlib, gzip, lzo, zstd | None, zlib, gzip, lzo, zstd |
| Superblock RSA verification | No | N/A |

VDFS4 and VDFS1002 share core file, directory, symlink, hard-link, inline-fork,
extent, and compression support, but they expose different format-specific
workflows. Both formats support catalog listing, JSON audit, single-file
extraction, and selected-subtree restoration. VDFS4 additionally provides
manifest/xattr export and AES key support. VDFS1002 manifest, AES, and RSA
descriptor formats remain open until their on-disk structures are confirmed.

## Build

Use the checked-in lockfile:

```sh
cargo build --release --locked
```

The executable is written to `target/release/vdfs-tool`.

## Unpack

Automatic format detection selects VDFS1002 or VDFS4:

```sh
vdfs-tool unpack <image.img> -o <output-directory>
```

The output directory must not already exist. Without `-o`, the default is
`<image-stem>-extracted` in the current directory.

For an unrecognized image, use `--force` to try the VDFS4 reader explicitly:

```sh
vdfs-tool unpack <image.img> -o <output-directory> --force
```

### Common options

| Option | Description |
|---|---|
| `-o, --output <DIR>` | Output directory or file, depending on the mode |
| `--include <PATH>` | Extract only this VDFS4 image path; repeat to include multiple paths |
| `--manifest-dir <DIR>` | Write VDFS4 catalog and xattr metadata manifest files |
| `--aes-key <FILE>` | Read a 16-byte raw AES-128 key for encrypted VDFS4 chunks |
| `--public-key <FILE>` | Read a PEM RSA public key for hashed VDFS4 descriptor verification |

Examples:

```sh
vdfs-tool unpack firmware.img --include root/usr/bin
vdfs-tool unpack firmware.img -o restored --manifest-dir metadata
vdfs-tool unpack firmware.img -o restored --aes-key aes.key
vdfs-tool unpack firmware.img -o restored --public-key vendor.pem
```

VDFS4 extraction restores regular files, directories, symlinks, hard-link
names, inline data, and extent-backed data. It can also emit catalog records
and xattrs in manifest mode.

VDFS4 images support the same catalog-read workflows as VDFS1002:

```sh
vdfs-tool unpack firmware.img --list
vdfs-tool unpack firmware.img --list --json
vdfs-tool unpack firmware.img --audit
vdfs-tool unpack firmware.img --extract hello.txt -o hello.txt
vdfs-tool unpack firmware.img --extract hello.txt --parent 12345 -o hello.txt
```

### VDFS1002 read options

VDFS1002 images support catalog listing, audit, and single-file extraction:

```sh
# Print catalog records
vdfs-tool unpack platform.img --list

# Print catalog records as JSON
vdfs-tool unpack platform.img --list --json

# Print block reachability and fork consistency as JSON
vdfs-tool unpack platform.img --audit

# Extract one file by catalog name
vdfs-tool unpack platform.img --extract TIFSDaemon -o TIFSDaemon

# Narrow name or parent matching
vdfs-tool unpack platform.img --extract README.md --parent 12345 -o README.md
```

Use `--parent <OBJECT_ID>` with `--list` or `--extract` to restrict matching
to a parent object ID.

Use `--include <PATH>` repeatedly for a selected VDFS1002 restore. Selected
folders retain their ancestors and restore the complete selected subtree:

```sh
vdfs-tool unpack platform.img -o selected --include root/usr --include root/etc
```

VDFS1002 normal unpack restores the catalog tree under a new destination.
The output may contain regular files, directories, symlinks, hard-link names,
back-references, and retained POSIX modes. Unsupported or unreadable records
are reported rather than silently presented as successful extraction.

### VDFS1002 TIF patch mode

```sh
vdfs-tool unpack platform.img --patch-tif -o patched.img
```

`--patch-tif` is a research mode for a pinned synthetic TIFSDaemon payload
and requires `-o`. It writes a new in-place-patched image and verifies a
round trip through the reader. It does not establish vendor trust,
authorization, or device-side acceptance.

## Pack

Pack a host directory into the default VDFS4 layout-2007 image:

```sh
vdfs-tool pack <directory> -o <image.img>
```

Select VDFS4 layout 2006:

```sh
vdfs-tool pack <directory> -o <image.img> --layout 2006
```

Create a native VDFS1002 image:

```sh
vdfs-tool pack <directory> -o <image.img> --format vdfs1002
```

`--layout 2006` is only valid with `--format vdfs4`; VDFS1002 uses its native
layout. The output image must not already exist.

Select a compression codec:

```sh
vdfs-tool pack <directory> -o <image.img> --compression zlib
```

Supported packing codecs:

- `none`
- `zlib`
- `gzip`
- `zstd`
- `lzo`

The packer supports regular files, directories, symbolic links, inline forks,
extent-backed files, and hard-link identity where the target format can
represent it. Unsupported special files are rejected.

## Verification and authentication

VDFS4 descriptor authentication supports layout-5 and layout-6
RSA-PKCS1 v1.5 trailers, including MD5, SHA-1, SHA-256, RSA1024, and
RSA2048. Provide the key with `--public-key`.

The tool does not currently provide vendor trust enrollment and does not
verify VDFS4 superblock RSA signatures.

VDFS1002 authentication, layout, trust-anchor, and device-side authorization
behavior remain research areas. Do not interpret a successful host-side
round trip as device authorization or a security bypass.

## Testing

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets
```

Test fixtures, real-image validation limits, build instructions, and
ignored survey tests are documented in [DEVELOP.md](DEVELOP.md).

Open implementation and validation work is tracked in [TODO.md](TODO.md).

## Development and releases

Do not commit directly to `main` or `develop`. Create a feature branch and
submit a pull request to `develop`. After merging approved changes into
`develop`, merge `develop` into `main` manually.

Pull requests targeting `develop` run formatting, tests, and Clippy. A push to
`main` runs the full test and cross-platform release pipeline. Releases use
the next patch version after the highest existing `vMAJOR.MINOR.PATCH` tag,
without requiring a manual `Cargo.toml` version bump.

Pull requests also build the `x86_64-pc-windows-msvc` target to prevent
Unix-only dependencies or APIs from reaching the release pipeline.

## License

GPL-2.0-or-later. See [LICENSE](LICENSE).
