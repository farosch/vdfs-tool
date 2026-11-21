use super::{Image, select_file};
use crate::cli::UnpackArgs;
use crate::crc::kernel_crc32;
use anyhow::{Context, Result, bail};
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

const IGZP_LAYOUT: u16 = 4;
const IGZP_GAP_BASE: usize = 144;
const IGZP_TABLE_ENTRY: usize = 16;
const IGZP_COUNT: usize = 1;

pub(crate) fn run(image_bytes: &mut [u8], args: &UnpackArgs) -> Result<()> {
    let modes = [args.audit, args.list, args.extract.is_some()]
        .iter()
        .filter(|set| **set)
        .count();
    if modes > 1 {
        bail!("--audit, --list and --extract are mutually exclusive");
    }
    if args.parent.is_some() && !args.list && args.extract.is_none() {
        bail!("--parent needs --list or --extract");
    }
    if modes == 0
        && (!args.include.is_empty()
            || args.manifest_dir.is_some()
            || args.aes_key.is_some()
            || args.public_key.is_some())
    {
        bail!(
            "--include, --manifest-dir, --aes-key and --public-key are not supported for VDFS1002 images"
        );
    }

    if args.patch_tif {
        let output = args
            .output
            .as_deref()
            .context("--patch-tif requires -o/--output")?;
        if output.exists() {
            bail!("patch output already exists: {}", output.display());
        }
        let catalog_copy = image_bytes.to_vec();
        let catalog_reader = Image::open(&catalog_copy)?;
        let records = catalog_reader.catalog_records()?;
        let record = select_file(&records, "TIFSDaemon", None)?;
        let overflow = catalog_reader.overflow_extents()?;
        let raw = catalog_reader.read_file(record, &overflow, 256 << 20)?;
        let mut logical = catalog_reader.file_contents(record, &overflow)?;
        let at_count = 0x83a8eusize;
        let at_bmi = 0x83a92usize;
        if logical.get(at_count..at_count + 2) != Some(&b"\x00\x2b"[..])
            || logical.get(at_bmi..at_bmi + 2) != Some(&b"\x35\xdd"[..])
        {
            bail!("TIFSDaemon ELF bytes do not match the pinned original guard");
        }
        logical[at_count..at_count + 2].copy_from_slice(b"\x00\x29");
        logical[at_bmi..at_bmi + 2].copy_from_slice(b"\x35\xd4");

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&logical)?;
        let compressed = encoder.finish()?;
        eprintln!(
            "TIF logical={} compressed={} object={}",
            logical.len(),
            compressed.len(),
            raw.len()
        );
        let blob_size = raw.len();
        let gap = IGZP_GAP_BASE + IGZP_TABLE_ENTRY * IGZP_COUNT;
        let table = blob_size
            .checked_sub(32)
            .and_then(|value| value.checked_sub(gap))
            .and_then(|value| value.checked_sub(IGZP_TABLE_ENTRY))
            .context("TIF raw blob is too small for IGzp layout")?;
        if compressed.len() > table {
            bail!(
                "patched compressed blob needs {} bytes, but original object has {} bytes before the table",
                compressed.len(),
                table,
            );
        }
        let mut blob = vec![0u8; blob_size];
        blob[..compressed.len()].copy_from_slice(&compressed);
        blob[table..table + 2].copy_from_slice(b"XT");
        blob[table + 2..table + 4].copy_from_slice(&0u16.to_le_bytes());
        blob[table + 4..table + 8].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
        blob[table + 8..table + 16].copy_from_slice(&0u64.to_le_bytes());
        let descriptor = blob_size - 32;
        blob[descriptor..descriptor + 4].copy_from_slice(b"IGzp");
        blob[descriptor + 4..descriptor + 6].copy_from_slice(&1u16.to_le_bytes());
        blob[descriptor + 6..descriptor + 8].copy_from_slice(&IGZP_LAYOUT.to_le_bytes());
        blob[descriptor + 8..descriptor + 16]
            .copy_from_slice(&(logical.len() as u64).to_le_bytes());
        blob[descriptor + 16..descriptor + 20].fill(0);
        blob[descriptor + 20..descriptor + 24].copy_from_slice(&17u32.to_le_bytes());
        blob[descriptor + 24..descriptor + 32].fill(0);
        let descriptor_zeroed = blob[descriptor..descriptor + 32].to_vec();
        let mut tail = vec![0u8; 32];
        tail.copy_from_slice(&descriptor_zeroed);
        tail[16..20].fill(0);
        let crc = kernel_crc32(&[&blob[table..descriptor], &tail]);
        blob[descriptor + 16..descriptor + 20].copy_from_slice(&crc.to_le_bytes());
        let written = catalog_reader.replace_file_blob(record, &overflow, image_bytes, &blob)?;
        std::fs::write(output, image_bytes)
            .with_context(|| format!("Failed to write patched image: {}", output.display()))?;
        let patched = std::fs::read(output)
            .with_context(|| format!("Failed to reread patched image: {}", output.display()))?;
        let patched_reader = Image::open(&patched)?;
        let patched_records = patched_reader.catalog_records()?;
        let patched_record = select_file(&patched_records, "TIFSDaemon", None)?;
        let patched_overflow = patched_reader.overflow_extents()?;
        let restored = patched_reader.file_contents(patched_record, &patched_overflow)?;
        if restored != logical {
            bail!(
                "patched TIFSDaemon round-trip mismatch: got {} bytes, expected {}",
                restored.len(),
                logical.len(),
            );
        }
        if blob[descriptor + 6..descriptor + 8] != IGZP_LAYOUT.to_le_bytes() {
            bail!("patched TIFSDaemon descriptor layout is not layout {IGZP_LAYOUT}");
        }
        println!(
            "{}",
            serde_json::json!({
                "output": output.display().to_string(),
                "file": "TIFSDaemon",
                "patched_bytes": written,
            })
        );
        return Ok(());
    }

    let reader = Image::open(image_bytes)?;

    if args.audit {
        let records = reader.catalog_records()?;
        let audit = reader.audit(&records)?;
        println!("{}", serde_json::to_string_pretty(&audit.to_json())?);
        return Ok(());
    }

    if args.list {
        let records = reader.catalog_records()?;
        let rows: Vec<&super::CatalogRecord> = records
            .iter()
            .filter(|record| args.parent.is_none_or(|id| record.parent_id == id))
            .collect();
        if args.json {
            let values: Vec<serde_json::Value> =
                rows.iter().map(|record| record.to_json()).collect();
            let listing = serde_json::json!({ "record_count": values.len(), "records": values });
            println!("{}", serde_json::to_string_pretty(&listing)?);
        } else {
            for record in rows {
                println!(
                    "{}\t{}\t{}\t{}",
                    record.parent_id,
                    record.object_id,
                    record.type_name(),
                    String::from_utf8_lossy(&record.name)
                );
            }
        }
        return Ok(());
    }

    if let Some(name) = args.extract.as_deref() {
        let output = args
            .output
            .as_deref()
            .context("--extract requires -o/--output")?;
        let records = reader.catalog_records()?;
        let record = select_file(&records, name, args.parent)?;
        let overflow = reader.overflow_extents()?;
        let contents = reader.file_contents(record, &overflow)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .with_context(|| format!("Failed to create output: {}", output.display()))?;
        file.write_all(&contents)?;
        let digest = format!("{:x}", Sha256::digest(&contents));
        let report = serde_json::json!({
            "output": output.display().to_string(),
            "size": contents.len(),
            "sha256": digest,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let output_dir = args.output.clone().unwrap_or_else(|| {
        let stem = args.input.file_stem().unwrap_or_default();
        PathBuf::from(format!("{}-extracted", stem.to_string_lossy()))
    });
    if output_dir.exists() {
        bail!("Output path already exists: {}", output_dir.display());
    }
    println!("VDFS1002 Image");
    println!("Extracting to: {}", output_dir.display());
    let summary = super::restore(image_bytes, &output_dir)?;
    if !summary.extended_checksum_valid {
        eprintln!("  Warning: extended record checksum mismatch");
    }
    println!("  Directories: {}", summary.directories);
    println!("  Files: {}", summary.files);
    println!("  Hard-link names: {}", summary.hardlinks);
    println!("  Bytes written: {}", summary.bytes);
    println!(
        "  Back-references (not created): {}",
        summary.back_references
    );
    println!("  Links not created: {}", summary.links_not_created);
    println!(
        "  Nameless file bodies (hard-link data): {}",
        summary.nameless_bodies
    );
    println!("  Skipped entries: {}", summary.skipped);
    for (object_id, message) in &summary.failures {
        eprintln!("  Failed object {object_id}: {message}");
    }
    if !summary.failures.is_empty() {
        bail!("{} entries failed to restore", summary.failures.len());
    }
    println!("Done!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{IGZP_COUNT, IGZP_GAP_BASE, IGZP_LAYOUT, IGZP_TABLE_ENTRY};

    #[test]
    fn synthetic_tif_igzp_uses_layout4() {
        assert_eq!(IGZP_LAYOUT, 4);
    }

    #[test]
    fn synthetic_tif_igzp_gap_is_crc_pinned() {
        assert_eq!(IGZP_GAP_BASE + IGZP_TABLE_ENTRY * IGZP_COUNT, 160,);
    }
}
