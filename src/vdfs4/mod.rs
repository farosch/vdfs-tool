pub(crate) mod authentication;
mod btree;
mod cattree;
mod decompress;
mod disk;
mod extents;
mod manifest;
mod reader;
mod snapshot;
mod superblock;
mod unpacker;
pub(crate) mod writer;

use crate::cli::UnpackArgs;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

/// Checks the superblock signature and layout version that `superblock::read_superblock` requires.
pub(crate) fn is_vdfs4(image: &[u8]) -> bool {
    let sb = disk::SECTOR_SIZE * 2;
    let signature = image.get(sb..sb + 4);
    let layout = image.get(sb + 4..sb + 8);
    signature == Some(disk::VDFS4_SB_SIGNATURE.as_slice())
        && (layout == Some(disk::VDFS4_LAYOUT_VERSION_2006.as_slice())
            || layout == Some(disk::VDFS4_LAYOUT_VERSION_2007.as_slice()))
}

pub(crate) fn run(image: &[u8], args: &UnpackArgs) -> Result<()> {
    if args.patch_tif {
        bail!("--patch-tif is only supported for VDFS1002 images");
    }

    let read_modes = [args.audit, args.list, args.extract.is_some()]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();
    if read_modes > 1 {
        bail!("--audit, --list and --extract are mutually exclusive");
    }
    if args.parent.is_some() && !args.list && args.extract.is_none() {
        bail!("--parent needs --list or --extract");
    }
    if read_modes > 0 && (!args.include.is_empty() || args.manifest_dir.is_some()) {
        bail!("--include and --manifest-dir are only supported by full VDFS4 extraction");
    }

    let output_dir = args.output.clone().unwrap_or_else(|| {
        let stem = args.input.file_stem().unwrap_or_default();
        PathBuf::from(format!("{}-extracted", stem.to_string_lossy()))
    });
    if output_dir.exists() {
        bail!("Output path already exists: {}", output_dir.display());
    }

    let mut reader = reader::ImageReader::new(image)?;
    if let Some(key_path) = args.aes_key.as_deref() {
        let raw = std::fs::read(key_path)
            .with_context(|| format!("Failed to read AES key: {}", key_path.display()))?;
        let key: [u8; 16] = raw
            .as_slice()
            .try_into()
            .with_context(|| format!("AES key must be exactly 16 bytes: {}", key_path.display()))?;
        reader.aes_key = Some(key);
    }
    if let Some(public_key_path) = args.public_key.as_deref() {
        reader.rsa_public_key = Some(authentication::read_public_key(public_key_path)?);
    }

    if args.list {
        return list_records(&reader, args);
    }
    if args.audit {
        return audit_records(&reader);
    }
    if let Some(name) = args.extract.as_deref() {
        return extract_file(&reader, name, args.parent, args.output.as_deref());
    }

    println!("VDFS4 Image Info:");
    println!("  Block size: {} bytes", reader.sbi.block_size);
    println!("  Volume blocks: {}", reader.sbi.volume_blocks_count);
    println!("  Case insensitive: {}", reader.sbi.case_insensitive);

    println!("Extracting to: {}", output_dir.display());
    manifest::start(args.manifest_dir.as_deref(), &output_dir)?;
    if let Some(directory) = args.manifest_dir.as_deref() {
        manifest::export_xattrs(&reader, directory)?;
    }
    unpacker::unpack_filesystem(&reader, &output_dir, &args.include)?;
    manifest::finish()?;

    println!("Done!");
    Ok(())
}

fn record_type_name(record_type: u8) -> &'static str {
    match record_type {
        disk::VDFS4_CATALOG_FOLDER_RECORD => "folder",
        disk::VDFS4_CATALOG_FILE_RECORD => "file",
        disk::VDFS4_CATALOG_HLINK_RECORD => "hardlink",
        disk::VDFS4_CATALOG_ILINK_RECORD => "indirect_link",
        _ => "unknown",
    }
}

fn list_records(reader: &reader::ImageReader, args: &UnpackArgs) -> Result<()> {
    let cat_tree = cattree::open_cat_tree(reader)?;
    let records = cat_tree.collect_all_records()?;
    let rows: Vec<&Vec<u8>> = records
        .iter()
        .filter(|record| {
            args.parent
                .is_none_or(|id| disk::cattree_key_parent_id(record) == id)
        })
        .collect();

    if args.json {
        let values: Vec<serde_json::Value> = rows
            .iter()
            .map(|record| {
                let record_type = disk::cattree_key_record_type(record);
                serde_json::json!({
                    "parent_id": disk::cattree_key_parent_id(record),
                    "object_id": disk::cattree_key_object_id(record),
                    "type": record_type_name(record_type),
                    "name": String::from_utf8_lossy(disk::cattree_key_name(record)),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "record_count": values.len(),
                "records": values,
            }))?
        );
    } else {
        for record in rows {
            println!(
                "{}\t{}\t{}\t{}",
                disk::cattree_key_parent_id(record),
                disk::cattree_key_object_id(record),
                record_type_name(disk::cattree_key_record_type(record)),
                String::from_utf8_lossy(disk::cattree_key_name(record)),
            );
        }
    }
    Ok(())
}

fn audit_records(reader: &reader::ImageReader) -> Result<()> {
    let cat_tree = cattree::open_cat_tree(reader)?;
    let records = cat_tree.collect_all_records()?;
    let overflow = extents::build_extent_overflow_map(reader, &extents::open_ext_tree(reader)?)?;

    let mut folders = 0u64;
    let mut files = 0u64;
    let mut hardlinks = 0u64;
    let mut indirect_links = 0u64;
    let mut fork_consistent = 0u64;
    let mut fork_inconsistent = 0u64;

    for record in &records {
        match disk::cattree_key_record_type(record) {
            disk::VDFS4_CATALOG_FOLDER_RECORD => folders += 1,
            disk::VDFS4_CATALOG_FILE_RECORD => {
                files += 1;
                let key_len = disk::cattree_key_key_len(record);
                let value = record
                    .get(key_len..)
                    .context("Truncated VDFS4 file record")?;
                if value.len() < std::mem::size_of::<disk::Vdfs4CatalogFileRecord>() {
                    fork_inconsistent += 1;
                    continue;
                }
                let file: disk::Vdfs4CatalogFileRecord =
                    unsafe { std::ptr::read_unaligned(value.as_ptr().cast()) };
                let object_id = disk::cattree_key_object_id(record);
                let resolved = extents::extents_resolve_fork(
                    &file.fork,
                    file.fork.size_in_bytes,
                    &overflow,
                    object_id,
                );
                match resolved {
                    Ok(extents) if !extents.is_empty() => fork_consistent += 1,
                    Ok(_) if file.fork.size_in_bytes == 0 => fork_consistent += 1,
                    _ => fork_inconsistent += 1,
                }
            }
            disk::VDFS4_CATALOG_HLINK_RECORD => hardlinks += 1,
            disk::VDFS4_CATALOG_ILINK_RECORD => indirect_links += 1,
            _ => {}
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "catalog_records": records.len(),
            "folders": folders,
            "files": files,
            "hardlinks": hardlinks,
            "indirect_links": indirect_links,
            "overflow_objects": overflow.len(),
            "overflow_records": overflow.values().map(|items| items.len()).sum::<usize>(),
            "fork_consistent": fork_consistent,
            "fork_inconsistent": fork_inconsistent,
            "volume_blocks": reader.sbi.volume_blocks_count,
        }))?
    );
    Ok(())
}

fn extract_file(
    reader: &reader::ImageReader,
    name: &str,
    parent: Option<u64>,
    output: Option<&std::path::Path>,
) -> Result<()> {
    let output = output.context("--extract requires -o/--output")?;
    if output.exists() {
        bail!("Extract output already exists: {}", output.display());
    }

    let cat_tree = cattree::open_cat_tree(reader)?;
    let records = cat_tree.collect_all_records()?;
    let candidates: Vec<&Vec<u8>> = records
        .iter()
        .filter(|record| {
            let record_type = disk::cattree_key_record_type(record);
            (record_type == disk::VDFS4_CATALOG_FILE_RECORD
                || record_type == disk::VDFS4_CATALOG_HLINK_RECORD)
                && disk::cattree_key_name(record) == name.as_bytes()
                && parent.is_none_or(|id| disk::cattree_key_parent_id(record) == id)
        })
        .collect();
    let record = match candidates.as_slice() {
        [record] => *record,
        _ => bail!(
            "{} file records named {name:?}; narrow with --parent",
            candidates.len()
        ),
    };

    let target = if disk::cattree_key_record_type(record) == disk::VDFS4_CATALOG_HLINK_RECORD {
        let target_id = disk::cattree_key_object_id(record);
        records
            .iter()
            .find(|candidate| {
                disk::cattree_key_record_type(candidate) == disk::VDFS4_CATALOG_FILE_RECORD
                    && disk::cattree_key_object_id(candidate) == target_id
            })
            .context("Could not resolve VDFS4 hard-link target")?
    } else {
        record
    };

    let key_len = disk::cattree_key_key_len(target);
    let value = target
        .get(key_len..)
        .context("Truncated VDFS4 file catalog record")?;
    if value.len() < std::mem::size_of::<disk::Vdfs4CatalogFileRecord>() {
        bail!("Truncated VDFS4 file catalog record");
    }
    let file: disk::Vdfs4CatalogFileRecord =
        unsafe { std::ptr::read_unaligned(value.as_ptr().cast()) };
    let object_id = disk::cattree_key_object_id(target);
    let overflow = extents::build_extent_overflow_map(reader, &extents::open_ext_tree(reader)?)?;
    let fork = file.fork;
    let contents = if decompress::is_compressed_file(file.common.flags) {
        decompress::decompress_file(reader, &fork, object_id, &overflow)?
    } else if fork.total_blocks_count == 0 {
        let len = usize::try_from(fork.size_in_bytes)?;
        if len > fork.inline_data().len() {
            bail!("Inline VDFS4 file size exceeds fork");
        }
        fork.inline_data()[..len].to_vec()
    } else {
        let extents =
            extents::extents_resolve_fork(&fork, fork.size_in_bytes, &overflow, object_id)?;
        extents::extents_read_data(reader, &extents, 0, fork.size_in_bytes)?
    };

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .with_context(|| format!("Failed to create output: {}", output.display()))?;
    file.write_all(&contents)?;
    let digest = format!("{:x}", Sha256::digest(&contents));
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "output": output.display().to_string(),
            "size": contents.len(),
            "sha256": digest,
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_vdfs4, run};
    use crate::cli::UnpackArgs;
    use std::path::PathBuf;

    fn image_with(signature: &[u8], layout: &[u8]) -> Vec<u8> {
        let mut image = vec![0u8; 2048];
        image[1024..1028].copy_from_slice(signature);
        image[1028..1032].copy_from_slice(layout);
        image
    }

    #[test]
    fn detects_vdfs4_layouts() {
        assert!(is_vdfs4(&image_with(b"VDFS", b"2007")));
        assert!(is_vdfs4(&image_with(b"VDFS", b"2006")));
    }

    #[test]
    fn rejects_other_images() {
        assert!(!is_vdfs4(&image_with(b"VDFS", b"1002")));
        assert!(!is_vdfs4(&image_with(b"XXXX", b"2007")));
        assert!(!is_vdfs4(&[0u8; 2048]));
        assert!(!is_vdfs4(&[0u8; 100]));
    }

    #[test]
    fn vdfs4_unpack_refuses_existing_output_directory() {
        let output =
            std::env::temp_dir().join(format!("vdfs-tool-existing-output-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output);
        std::fs::create_dir_all(&output).unwrap();
        let args = UnpackArgs {
            input: PathBuf::from("unused.vdfs"),
            output: Some(output.clone()),
            include: Vec::new(),
            manifest_dir: None,
            aes_key: None,
            public_key: None,
            force: false,
            list: false,
            json: false,
            audit: false,
            extract: None,
            parent: None,
            patch_tif: false,
        };

        let result = run(&[0u8; 2048], &args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Output path already exists")
        );
        let _ = std::fs::remove_dir_all(&output);
    }
}
