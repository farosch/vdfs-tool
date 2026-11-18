use anyhow::{Result, bail};

use crate::cli::PackArgs;

pub(crate) fn run(args: PackArgs) -> Result<()> {
    if !args.input.is_dir() {
        bail!("Input directory does not exist: {}", args.input.display());
    }

    let output = args.output.unwrap_or_else(|| {
        let name = args
            .input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("filesystem");
        args.input.with_file_name(format!("{name}.vdfs"))
    });

    crate::vdfs4::writer::write_image(&args.input, &output, args.compression)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::PackCompression;
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
        });
        assert!(result.is_ok());
        assert!(output.exists());

        let _ = fs::remove_dir_all(&path);
    }
}
