use anyhow::{Result, bail};

use crate::cli::PackArgs;

pub(crate) fn run(args: PackArgs) -> Result<()> {
    if !args.input.is_dir() {
        bail!("Input directory does not exist: {}", args.input.display());
    }
    if args.format == crate::cli::PackFormat::Vdfs1002
        && args.layout == crate::cli::PackLayout::Layout2006
    {
        bail!("--layout 2006 is only supported with --format vdfs4");
    }

    let output = args.output.unwrap_or_else(|| {
        let name = args
            .input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("filesystem");
        args.input.with_file_name(format!("{name}.vdfs"))
    });

    match args.format {
        crate::cli::PackFormat::Vdfs4 => {
            crate::vdfs4::writer::write_image(&args.input, &output, args.compression, args.layout)
        }
        crate::cli::PackFormat::Vdfs1002 => {
            crate::vdfs1002::writer::write_image(&args.input, &output, args.compression)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{PackCompression, PackFormat, PackLayout};
    use std::fs;

    #[test]
    fn rejects_missing_input_directory() {
        let path =
            std::env::temp_dir().join(format!("vdfs-tool-pack-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        let result = run(PackArgs {
            input: path.clone(),
            output: None,
            compression: PackCompression::None,
            format: PackFormat::Vdfs4,
            layout: PackLayout::Layout2007,
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn rejects_before_writing_output() {
        let path =
            std::env::temp_dir().join(format!("vdfs-tool-pack-input-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        let output = path.with_extension("vdfs");
        let _ = fs::remove_file(&output);

        let result = run(PackArgs {
            input: path.clone(),
            output: Some(output.clone()),
            compression: PackCompression::Zlib,
            format: PackFormat::Vdfs4,
            layout: PackLayout::Layout2007,
        });
        assert!(result.is_ok());
        assert!(output.exists());

        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn rejects_layout2006_for_vdfs1002() {
        let path =
            std::env::temp_dir().join(format!("vdfs-tool-pack-layout-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();

        let result = run(PackArgs {
            input: path.clone(),
            output: Some(path.with_extension("vdfs")),
            compression: PackCompression::None,
            format: PackFormat::Vdfs1002,
            layout: PackLayout::Layout2006,
        });
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("only supported with --format vdfs4")
        );

        let _ = fs::remove_dir_all(&path);
    }
}
