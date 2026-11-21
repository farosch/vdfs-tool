use super::{Image, select_file};
use crate::cli::UnpackArgs;
use crate::crc::kernel_crc32;
use anyhow::{Context, Result, bail};
use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

use super::codec::{compressed_descriptor, locate_chunk_table};

const IGZP_LAYOUT: u16 = 4;
const IGZP_GAP_BASE: usize = 144;
const IGZP_TABLE_ENTRY: usize = 16;
const IGZP_COUNT: usize = 1;

pub(crate) fn recompress_file(raw: &[u8], logical: &[u8]) -> Result<Vec<u8>> {
    let descriptor = compressed_descriptor(raw)?
        .context("replacement target is not a supported compressed descriptor")?;
    locate_chunk_table(raw, descriptor.start, descriptor.count as usize)?;

    let compressed = match &descriptor.codec {
        b"Gzp" => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(logical)?;
            encoder.finish()?
        }
        b"Zip" => {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(logical)?;
            encoder.finish()?
        }
        _ => bail!("replacement codec is not zlib or gzip"),
    };
    let descriptor_size = raw.len() - descriptor.start;
    let replacement_table = raw
        .len()
        .checked_sub(descriptor_size)
        .and_then(|start| start.checked_sub(16))
        .context("replacement descriptor leaves no chunk-table entry")?;
    if compressed.len() > replacement_table {
        bail!(
            "replacement compressed blob needs {} bytes but only {} fit before the descriptor",
            compressed.len(),
            replacement_table,
        );
    }

    let mut blob = vec![0u8; raw.len()];
    blob[..compressed.len()].copy_from_slice(&compressed);
    blob[replacement_table..replacement_table + 2].copy_from_slice(b"XT");
    blob[replacement_table + 4..replacement_table + 8]
        .copy_from_slice(&(compressed.len() as u32).to_le_bytes());
    blob[replacement_table + 8..replacement_table + 16].fill(0);

    let start = raw.len() - descriptor_size;
    blob[start] = raw[descriptor.start];
    blob[start + 1..start + 4].copy_from_slice(&descriptor.codec);
    blob[start + 4..start + 6].copy_from_slice(&1u16.to_le_bytes());
    blob[start + 6..start + 8].copy_from_slice(&raw[descriptor.start + 6..descriptor.start + 8]);
    blob[start + 8..start + 16].copy_from_slice(&(logical.len() as u64).to_le_bytes());
    blob[start + 20..start + 24].copy_from_slice(&17u32.to_le_bytes());

    let mut descriptor_tail = blob[start..start + descriptor_size].to_vec();
    descriptor_tail[16..20].fill(0);
    let crc = kernel_crc32(&[&blob[replacement_table..start], &descriptor_tail]);
    blob[start + 16..start + 20].copy_from_slice(&crc.to_le_bytes());

    let repaired_table =
        locate_chunk_table(&blob, start, 1).context("replacement blob CRC validation failed")?;
    if repaired_table != replacement_table {
        bail!("replacement chunk table was not located at the expected offset");
    }
    Ok(blob)
}

pub(crate) fn run(image_bytes: &mut [u8], args: &UnpackArgs) -> Result<()> {
    let modes = [
        args.audit,
        args.list,
        args.extract.is_some(),
        args.replace_file.is_some(),
    ]
    .iter()
    .filter(|set| **set)
    .count();
    if modes > 1 {
        bail!("--audit, --list, --extract and --replace-file are mutually exclusive");
    }
    if args.parent.is_some() && !args.list && args.extract.is_none() && args.replace_file.is_none()
    {
        bail!("--parent needs --list, --extract or --replace-file");
    }
    if modes > 0 && !args.include.is_empty() {
        bail!("--include is only supported by full VDFS1002 restoration");
    }
    if modes == 0
        && (args.manifest_dir.is_some()
            || args.aes_key.is_some()
            || !args.public_key.is_empty()
            || !args.trust_key.is_empty()
            || !args.trust_dir.is_empty())
    {
        bail!(
            "--manifest-dir, --aes-key, --public-key, --trust-key and --trust-dir are not supported until a confirmed VDFS1002 RSA signature format is implemented"
        );
    }
    super::validate_selection(&args.include)?;

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

    if let Some(name) = args.replace_file.as_deref() {
        let output = args
            .output
            .as_deref()
            .context("--replace-file requires -o/--output")?;
        let replacement_path = args
            .replacement
            .as_deref()
            .context("--replace-file requires --replacement")?;
        if output.exists() {
            bail!("replacement output already exists: {}", output.display());
        }
        let replacement = std::fs::read(replacement_path).with_context(|| {
            format!("Failed to read replacement {}", replacement_path.display())
        })?;
        let mut replacement_image = image_bytes.to_vec();
        let reader = Image::open(&replacement_image)?;
        let records = reader.catalog_records()?;
        let record = select_file(&records, name, args.parent)?;
        let overflow = reader.overflow_extents()?;
        let raw = reader.read_file(record, &overflow, 256 << 20)?;
        let block_size = reader.block_size();
        let origin = reader.origin();
        let blob = if compressed_descriptor(&raw)?.is_some() {
            recompress_file(&raw, &replacement)?
        } else {
            if replacement.len() as u64 > record.size {
                bail!(
                    "replacement size {} exceeds original file size {}",
                    replacement.len(),
                    record.size,
                );
            }
            let mut blob = vec![0u8; raw.len()];
            blob[..replacement.len()].copy_from_slice(&replacement);
            blob
        };
        if blob.len() as u64 > record.size {
            bail!(
                "replacement blob is {} bytes but the original file allocation is {} bytes",
                blob.len(),
                record.size,
            );
        }
        drop(reader);
        let written = Image::replace_file_blob_at(
            block_size,
            origin,
            record,
            &overflow,
            &mut replacement_image,
            &blob,
        )?;
        std::fs::write(output, &replacement_image)
            .with_context(|| format!("Failed to write replacement image {}", output.display()))?;
        let patched = std::fs::read(output)
            .with_context(|| format!("Failed to reread replacement image {}", output.display()))?;
        let patched_reader = Image::open(&patched)?;
        let patched_records = patched_reader.catalog_records()?;
        let patched_record = select_file(&patched_records, name, args.parent)?;
        let patched_overflow = patched_reader.overflow_extents()?;
        let restored = patched_reader.file_contents(patched_record, &patched_overflow)?;
        if restored != replacement {
            bail!(
                "replacement round-trip mismatch: got {} bytes, expected {}",
                restored.len(),
                replacement.len(),
            );
        }
        println!(
            "{}",
            serde_json::json!({
                "output": output.display().to_string(),
                "file": name,
                "replacement_bytes": replacement.len(),
                "physical_bytes": written,
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
    let summary = super::restore_selected(image_bytes, &output_dir, &args.include)?;
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
    use super::{IGZP_COUNT, IGZP_GAP_BASE, IGZP_LAYOUT, IGZP_TABLE_ENTRY, recompress_file};
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    use super::super::codec::{compressed_descriptor, decompress_blob};
    use crate::crc::kernel_crc32;

    #[test]
    fn synthetic_tif_igzp_uses_layout4() {
        assert_eq!(IGZP_LAYOUT, 4);
    }

    #[test]
    fn synthetic_tif_igzp_gap_is_crc_pinned() {
        assert_eq!(IGZP_GAP_BASE + IGZP_TABLE_ENTRY * IGZP_COUNT, 160,);
    }

    #[test]
    fn replaces_compressed_gzip_file_and_round_trips_crc() {
        let original = b"<resourceinformationfile>old</resourceinformationfile>";
        let replacement =
            b"<resourceinformationfile>new-longer-resource-list</resourceinformationfile>";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut raw = vec![0u8; 512];
        raw[..compressed.len()].copy_from_slice(&compressed);
        let table = 480 - 16;
        raw[table..table + 2].copy_from_slice(b"XT");
        raw[table + 4..table + 8].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
        raw[480..484].copy_from_slice(b"C?zp");
        raw[480..481].copy_from_slice(b"C");
        raw[481..484].copy_from_slice(b"Gzp");
        raw[484..486].copy_from_slice(&1u16.to_le_bytes());
        raw[486..488].copy_from_slice(&5u16.to_le_bytes());
        raw[488..496].copy_from_slice(&(original.len() as u64).to_le_bytes());
        let mut tail = raw[480..512].to_vec();
        tail[16..20].fill(0);
        let crc = kernel_crc32(&[&raw[table..480], &tail]);
        raw[496..500].copy_from_slice(&crc.to_le_bytes());

        let rebuilt = recompress_file(&raw, replacement).unwrap();
        let descriptor = compressed_descriptor(&rebuilt).unwrap().unwrap();
        let restored = decompress_blob(&rebuilt, &descriptor).unwrap();
        assert_eq!(restored, replacement);
    }
}
