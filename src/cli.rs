use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "vdfs-tool",
    about = "Pack and unpack Samsung VDFS filesystem images",
    version
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum Command {
    /// Extract a Samsung VDFS4 or VDFS1002 filesystem image.
    Unpack(UnpackArgs),
    /// Create a Samsung VDFS4 filesystem image.
    Pack(PackArgs),
    /// Manage local vendor public-key trust anchors.
    Trust(TrustArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct TrustArgs {
    #[command(subcommand)]
    pub(crate) command: TrustCommand,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum TrustCommand {
    /// Enroll a PEM RSA public key as a local trust anchor.
    Enroll(EnrollTrustArgs),
    /// List enrolled trust anchors.
    List(ListTrustArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct EnrollTrustArgs {
    /// Human-readable vendor or key label.
    #[arg(long)]
    pub(crate) label: String,

    /// PEM RSA public key to enroll.
    #[arg(long)]
    pub(crate) public_key: PathBuf,

    /// Override the local trust store directory.
    #[arg(long)]
    pub(crate) store: Option<PathBuf>,

    /// Replace an existing label.
    #[arg(long)]
    pub(crate) replace: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct ListTrustArgs {
    /// Override the local trust store directory.
    #[arg(long)]
    pub(crate) store: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct UnpackArgs {
    /// Path to the VDFS image (VDFS4 or VDFS1002).
    pub(crate) input: PathBuf,

    /// Directory to extract into. Defaults to `<image-stem>-extracted` in the current directory.
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,

    /// Extract only this image path (repeatable). Without it the whole image is extracted.
    #[arg(long = "include")]
    pub(crate) include: Vec<String>,

    /// VDFS4: write manifest files (catalog and xattrs) into this directory.
    #[arg(long)]
    pub(crate) manifest_dir: Option<PathBuf>,

    /// File containing the 16-byte raw AES-128 key for encrypted chunks (VDFS4 only).
    #[arg(long = "aes-key")]
    pub(crate) aes_key: Option<PathBuf>,

    /// PEM RSA public key used to verify hashed VDFS4 descriptor signatures.
    #[arg(long = "public-key")]
    pub(crate) public_key: Vec<PathBuf>,

    /// PEM RSA public key trust anchor (repeatable).
    #[arg(long = "trust-key")]
    pub(crate) trust_key: Vec<PathBuf>,

    /// Trust store directory (repeatable).
    #[arg(long = "trust-dir")]
    pub(crate) trust_dir: Vec<PathBuf>,

    /// Try VDFS4 extraction on an image that is not a recognized VDFS1002 or VDFS4 image.
    #[arg(long)]
    pub(crate) force: bool,

    /// VDFS1002: print the catalog records instead of restoring.
    #[arg(long)]
    pub(crate) list: bool,

    /// VDFS1002: with --list, print the records as JSON.
    #[arg(long, requires = "list")]
    pub(crate) json: bool,

    /// VDFS1002: print block-reach and file-fork audit as JSON.
    #[arg(long)]
    pub(crate) audit: bool,

    /// VDFS1002: write the file record with this name to -o (a new file).
    #[arg(long)]
    pub(crate) extract: Option<String>,

    /// VDFS1002: with --list or --extract, only match records whose parent object id is this.
    #[arg(long)]
    pub(crate) parent: Option<u64>,

    /// VDFS1002: write an in-place TIFSDaemon negative-index patch to -o.
    #[arg(long)]
    pub(crate) patch_tif: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct PackArgs {
    /// Directory to pack.
    pub(crate) input: PathBuf,

    /// Output VDFS4 image path.
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,

    /// Compression codec.
    #[arg(long, value_enum, default_value = "none")]
    pub(crate) compression: PackCompression,

    /// Filesystem format to create.
    #[arg(long, value_enum, default_value = "vdfs4")]
    pub(crate) format: PackFormat,

    /// VDFS4 layout version.
    #[arg(long, value_enum, default_value = "2007")]
    pub(crate) layout: PackLayout,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackCompression {
    None,
    Zlib,
    Gzip,
    Lzo,
    Zstd,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackFormat {
    Vdfs4,
    Vdfs1002,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackLayout {
    #[value(name = "2006")]
    Layout2006,
    #[value(name = "2007")]
    Layout2007,
}
