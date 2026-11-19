# vdfs-tool

`vdfs-tool` is a command-line utility for working with Samsung VDFS filesystem
images.

It can:

- unpack VDFS4 layout 2006 and 2007 images
- unpack VDFS1002 images
- restore directories, files, and symbolic links
- extract selected paths
- write VDFS4 catalog metadata manifests
- pack a host directory into a native VDFS4 layout-2007 image

## Build

```sh
cargo build --release --locked
```

The release binary is written to `target/release/vdfs-tool`.

## Unpack

```sh
vdfs-tool unpack <image.img> -o <output-directory>
```

When `-o` is omitted, the output directory defaults to
`<image-stem>-extracted` in the current directory.

### Unpack options

- `--include <PATH>`: extract only an image path. May be repeated.
- `--manifest-dir <DIR>`: write VDFS4 catalog and xattr metadata files.
- `--aes-key <FILE>`: supply a 16-byte AES-128 key for encrypted VDFS4 chunks.
- `--public-key <FILE>`: verify hashed VDFS4 descriptor signatures with a PEM RSA public key.
- `--force`: try VDFS4 extraction for an unrecognized image.

VDFS1002-specific read options include:

- `--list`
- `--json`
- `--audit`
- `--extract <NAME>`
- `--parent <OBJECT_ID>`

## Pack

Pack a host directory into a VDFS4 layout-2007 image:

```sh
vdfs-tool pack <directory> -o <image.img>
```

Select VDFS4 layout 2006 or VDFS1002:

```sh
vdfs-tool pack <directory> -o <image.img> --layout 2006
vdfs-tool pack <directory> -o <image.img> --format vdfs1002
```

Select compression with:

```sh
vdfs-tool pack <directory> -o <image.img> --compression zlib
```

Supported packing codecs:

- `none`
- `zlib`
- `gzip`
- `zstd`
- `lzo`

## Supported formats

| Format | Unpack | Pack |
|---|---|---|
| VDFS4 layout 2006 | Yes | Yes |
| VDFS4 layout 2007 | Yes | Yes |
| VDFS1002 | Yes | Yes |

The packer currently supports regular files, directories, symbolic links,
inline forks, and extent-backed files.

## Testing

```sh
cargo test --locked
cargo clippy --locked --all-targets
```

See [DEVELOP.md](DEVELOP.md) for build, test, and fixture details.

## License

Apache-2.0. See [LICENSE](LICENSE).
