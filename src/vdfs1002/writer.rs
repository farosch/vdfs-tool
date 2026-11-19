use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};

use super::format::*;
use crate::cli::PackCompression;
use crate::crc::kernel_crc32;

const BLOCK: u64 = 4096;
const NODE: u64 = 16384;
const BASE_BLOCK: u64 = 6;
const NODE_EXTENT_BLOCK: u64 = 16;
const DATA_START: u64 = 4;

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    parent_id: u64,
    object_id: u64,
}

pub(crate) fn write_image(input: &Path, output: &Path, compression: PackCompression) -> Result<()> {
    if !input.is_dir() {
        bail!("Input directory does not exist: {}", input.display());
    }
    if output.exists() {
        bail!("Output image already exists: {}", output.display());
    }

    let mut entries = Vec::new();
    collect_entries(input, 1, &mut entries, &mut 8u64)?;

    let mut next_block = DATA_START;
    let mut image = vec![0u8; (DATA_START as usize + 64) * BLOCK as usize];
    let mut catalog = Vec::new();
    let mut hardlinks = std::collections::HashMap::new();

    catalog.push(folder_record(0, 1, b"root", 0o040755));
    for entry in &entries {
        let metadata = fs::symlink_metadata(&entry.path)
            .with_context(|| format!("Failed to read metadata: {}", entry.path.display()))?;
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name_bytes = name.as_bytes();

        let catalog_record = if metadata.is_dir() {
            folder_record(entry.parent_id, entry.object_id, name_bytes, 0o040755)
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&entry.path)
                .with_context(|| format!("Failed to read symlink: {}", entry.path.display()))?;
            let target = target.as_os_str().to_string_lossy();
            let fork = allocate_data_fork(
                target.as_bytes(),
                &mut image,
                &mut next_block,
                PackCompression::None,
            )?;
            file_record(entry.parent_id, entry.object_id, name_bytes, 0o120777, fork)
        } else if metadata.is_file() {
            #[cfg(unix)]
            let inode = {
                use std::os::unix::fs::MetadataExt;
                metadata.ino()
            };
            #[cfg(unix)]
            if {
                use std::os::unix::fs::MetadataExt;
                metadata.nlink()
            } > 1
            {
                if let Some(target) = hardlinks.get(&inode) {
                    hardlink_record(entry.parent_id, *target, name_bytes)
                } else {
                    hardlinks.insert(inode, entry.object_id);
                    let data = fs::read(&entry.path).with_context(|| {
                        format!("Failed to read file: {}", entry.path.display())
                    })?;
                    let fork = allocate_data_fork(&data, &mut image, &mut next_block, compression)?;
                    file_record(entry.parent_id, entry.object_id, name_bytes, 0o100755, fork)
                }
            } else {
                let data = fs::read(&entry.path)
                    .with_context(|| format!("Failed to read file: {}", entry.path.display()))?;
                let fork = allocate_data_fork(&data, &mut image, &mut next_block, compression)?;
                file_record(entry.parent_id, entry.object_id, name_bytes, 0o100755, fork)
            }
            #[cfg(not(unix))]
            {
                let data = fs::read(&entry.path)
                    .with_context(|| format!("Failed to read file: {}", entry.path.display()))?;
                let fork = allocate_data_fork(&data, &mut image, &mut next_block, compression)?;
                file_record(entry.parent_id, entry.object_id, name_bytes, 0o100755, fork)
            }
        } else {
            bail!("Unsupported special file type: {}", entry.path.display());
        };
        catalog.push(catalog_record);
    }
    catalog.sort_by(|left, right| catalog_cmp(left, right));

    let head = node_head(1, 2);
    let root = index_node(1, &[2]);
    let leaf = leaf_node(&catalog, 0);
    let ext_head = node_head(1, 1);
    let ext_leaf = leaf_node(&[], 0);

    if image.len() < (80 + 64) as usize * BLOCK as usize {
        image.resize((80 + 64) as usize * BLOCK as usize, 0);
    }
    copy_node(&mut image, NODE_EXTENT_BLOCK, 0, &head)?;
    copy_node(&mut image, NODE_EXTENT_BLOCK + 4, 1, &root)?;
    copy_node(&mut image, NODE_EXTENT_BLOCK + 8, 2, &leaf)?;
    copy_node(&mut image, NODE_EXTENT_BLOCK + 12, 0, &ext_head)?;
    copy_node(&mut image, NODE_EXTENT_BLOCK + 16, 1, &ext_leaf)?;

    write_base_table(&mut image)?;
    let volume_blocks = image.len() as u64 / BLOCK;
    write_superblocks(&mut image, volume_blocks)?;
    fs::write(output, &image)
        .with_context(|| format!("Failed to write image: {}", output.display()))
}

fn collect_entries(
    directory: &Path,
    parent_id: u64,
    entries: &mut Vec<Entry>,
    next_id: &mut u64,
) -> Result<()> {
    let mut children = fs::read_dir(directory)
        .with_context(|| format!("Failed to read directory: {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        let object_id = *next_id;
        *next_id = next_id.checked_add(1).context("Object ID overflow")?;
        entries.push(Entry {
            path: path.clone(),
            parent_id,
            object_id,
        });
        if fs::symlink_metadata(&path)?.is_dir() {
            collect_entries(&path, object_id, entries, next_id)?;
        }
    }
    Ok(())
}

fn align8(value: usize) -> usize {
    (value + 7) & !7
}

fn catalog_cmp(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    le_u64(left, 8)
        .unwrap_or(0)
        .cmp(&le_u64(right, 8).unwrap_or(0))
        .then_with(|| {
            let left_name = &left[KEY_FIXED_SIZE..le_u16(left, 4).unwrap_or(26) as usize];
            let right_name = &right[KEY_FIXED_SIZE..le_u16(right, 4).unwrap_or(26) as usize];
            left_name.cmp(right_name)
        })
        .then_with(|| {
            le_u64(left, 16)
                .unwrap_or(0)
                .cmp(&le_u64(right, 16).unwrap_or(0))
        })
}

fn folder_record(parent: u64, object: u64, name: &[u8], mode: u16) -> Vec<u8> {
    let key_length = align8(KEY_FIXED_SIZE + name.len());
    let record_length = key_length + 80;
    let mut record = vec![0u8; record_length];
    record[0..4].copy_from_slice(b"eFd ");
    record[4..6].copy_from_slice(&(key_length as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(record_length as u16).to_le_bytes());
    record[8..16].copy_from_slice(&parent.to_le_bytes());
    record[16..24].copy_from_slice(&object.to_le_bytes());
    record[24] = RECORD_FOLDER;
    record[25] = name.len() as u8;
    record[26..26 + name.len()].copy_from_slice(name);
    record[key_length + 32..key_length + 34].copy_from_slice(&mode.to_le_bytes());
    record
}

fn file_record(parent: u64, object: u64, name: &[u8], mode: u16, fork: Vec<u8>) -> Vec<u8> {
    let key_length = align8(KEY_FIXED_SIZE + name.len());
    let record_length = key_length + fork.len();
    let mut record = vec![0u8; record_length];
    record[0..4].copy_from_slice(b"eFr ");
    record[4..6].copy_from_slice(&(key_length as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(record_length as u16).to_le_bytes());
    record[8..16].copy_from_slice(&parent.to_le_bytes());
    record[16..24].copy_from_slice(&object.to_le_bytes());
    record[24] = RECORD_FILE;
    record[25] = name.len() as u8;
    record[26..26 + name.len()].copy_from_slice(name);
    record[key_length + 32..key_length + 34].copy_from_slice(&mode.to_le_bytes());
    record[key_length..].copy_from_slice(&fork);
    record
}

fn hardlink_record(parent: u64, target: u64, name: &[u8]) -> Vec<u8> {
    let key_length = align8(KEY_FIXED_SIZE + name.len());
    let record_length = key_length;
    let mut record = vec![0u8; record_length];
    record[0..4].copy_from_slice(b"hRd ");
    record[4..6].copy_from_slice(&(key_length as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(record_length as u16).to_le_bytes());
    record[8..16].copy_from_slice(&parent.to_le_bytes());
    record[16..24].copy_from_slice(&target.to_le_bytes());
    record[24] = RECORD_HARDLINK;
    record[25] = name.len() as u8;
    record[26..26 + name.len()].copy_from_slice(name);
    record
}

fn compress(data: &[u8], compression: PackCompression) -> Result<Vec<u8>> {
    match compression {
        PackCompression::None => Ok(data.to_vec()),
        PackCompression::Zlib => {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data)?;
            Ok(encoder.finish()?)
        }
        PackCompression::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data)?;
            Ok(encoder.finish()?)
        }
        PackCompression::Zstd => {
            zstd::stream::encode_all(data, 3).context("Zstd compression failed")
        }
        PackCompression::Lzo => Ok(rust_lzo1x::compress(data)),
    }
}

fn codec_magic(compression: PackCompression) -> &'static [u8; 3] {
    match compression {
        PackCompression::None => b"Zip",
        PackCompression::Zlib => b"Zip",
        PackCompression::Gzip => b"Gzp",
        PackCompression::Zstd => b"Zst",
        PackCompression::Lzo => b"Lzo",
    }
}

fn allocate_data_fork(
    data: &[u8],
    image: &mut Vec<u8>,
    next_block: &mut u64,
    compression: PackCompression,
) -> Result<Vec<u8>> {
    let logical = data.len();
    let payload = if compression == PackCompression::None {
        data.to_vec()
    } else {
        let packed = compress(data, compression)?;
        if packed.len() < data.len() {
            build_compressed_blob(data, &packed, compression)?
        } else {
            data.to_vec()
        }
    };

    let blocks = payload.len().div_ceil(BLOCK as usize) as u64;
    let begin = *next_block;
    *next_block = next_block
        .checked_add(blocks)
        .context("Image block range overflow")?;
    let start = begin as usize * BLOCK as usize;
    if start + payload.len() > image.len() {
        image.resize(start + payload.len(), 0);
    }
    image[start..start + payload.len()].copy_from_slice(&payload);

    let size = if payload == data {
        logical as u64
    } else {
        payload.len() as u64
    };
    let mut fork = vec![0u8; FORK_SIZE];
    fork[..8].copy_from_slice(&size.to_le_bytes());
    fork[8..16].copy_from_slice(&blocks.to_le_bytes());
    fork[16..24].copy_from_slice(&begin.to_le_bytes());
    fork[24..32].copy_from_slice(&blocks.to_le_bytes());
    Ok(fork)
}

fn build_compressed_blob(
    unpacked: &[u8],
    chunk: &[u8],
    compression: PackCompression,
) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    let mut table = Vec::new();
    let chunk_payload = if chunk.len() < unpacked.len() {
        chunk
    } else {
        unpacked
    };
    payload.extend_from_slice(chunk_payload);
    table.extend_from_slice(b"XT");
    table.extend_from_slice(&0u16.to_le_bytes());
    table.extend_from_slice(&(chunk_payload.len() as u32).to_le_bytes());
    table.extend_from_slice(&0u64.to_le_bytes());

    let mut descriptor = vec![0u8; COMPRESSED_DESCRIPTOR_SIZE];
    descriptor[0] = b'C';
    descriptor[1..4].copy_from_slice(codec_magic(compression));
    descriptor[4..6].copy_from_slice(&1u16.to_le_bytes());
    descriptor[6..8].copy_from_slice(&COMPRESSED_LAYOUT.to_le_bytes());
    descriptor[8..16].copy_from_slice(&(unpacked.len() as u64).to_le_bytes());
    descriptor[20..24].copy_from_slice(&12u32.to_le_bytes());
    let crc = kernel_crc32(&[&table, &descriptor]);
    descriptor[16..20].copy_from_slice(&crc.to_le_bytes());
    payload.extend_from_slice(&table);
    payload.extend_from_slice(&descriptor);
    Ok(payload)
}

fn node_head(root: u32, height: u16) -> Vec<u8> {
    let mut node = vec![0u8; NODE as usize];
    node[0..4].copy_from_slice(b"eHND");
    node[12..16].copy_from_slice(&root.to_le_bytes());
    node[16..18].copy_from_slice(&height.to_le_bytes());
    node
}

fn index_node(node_id: u32, children: &[u32]) -> Vec<u8> {
    let mut node = vec![0u8; NODE as usize];
    node[14..16].copy_from_slice(&(children.len() as u16).to_le_bytes());
    node[24..28].copy_from_slice(&0u32.to_le_bytes());
    node[28..32].copy_from_slice(&NODE_INDEX.to_le_bytes());
    node[20..24].copy_from_slice(&node_id.to_le_bytes());
    let mut cursor = 32usize;
    let mut offsets = Vec::new();
    for child in children {
        let record = vec![0u8; 30];
        let mut record = record;
        record[4..6].copy_from_slice(&26u16.to_le_bytes());
        record[6..8].copy_from_slice(&30u16.to_le_bytes());
        record[26..30].copy_from_slice(&child.to_le_bytes());
        node[cursor..cursor + record.len()].copy_from_slice(&record);
        offsets.push(cursor);
        cursor += record.len();
    }
    offsets.push(cursor);
    for (index, offset) in offsets.iter().enumerate() {
        let at = NODE as usize - 4 * (index + 2);
        node[at..at + 4].copy_from_slice(&(*offset as u32).to_le_bytes());
    }
    node
}

fn leaf_node(records: &[Vec<u8>], next: u32) -> Vec<u8> {
    let mut node = vec![0u8; NODE as usize];
    node[14..16].copy_from_slice(&(records.len() as u16).to_le_bytes());
    node[24..28].copy_from_slice(&next.to_le_bytes());
    node[28..32].copy_from_slice(&NODE_LEAF.to_le_bytes());
    let mut cursor = 32usize;
    let mut offsets = Vec::new();
    for record in records {
        node[cursor..cursor + record.len()].copy_from_slice(record);
        offsets.push(cursor);
        cursor += record.len();
    }
    offsets.push(cursor);
    for (index, offset) in offsets.iter().enumerate() {
        let at = NODE as usize - 4 * (index + 2);
        node[at..at + 4].copy_from_slice(&(*offset as u32).to_le_bytes());
    }
    node
}

fn copy_node(image: &mut [u8], block: u64, node_id: u64, node: &[u8]) -> Result<()> {
    let start = (block as usize) * BLOCK as usize;
    if start + node.len() > image.len() {
        bail!("VDFS1002 node exceeds image");
    }
    image[start..start + node.len()].copy_from_slice(node);
    let _ = node_id;
    Ok(())
}

fn write_base_table(image: &mut Vec<u8>) -> Result<()> {
    let start = BASE_BLOCK as usize * BLOCK as usize;
    let mut base = vec![0u8; 1028];
    base[0..4].copy_from_slice(b"CoWB");
    base[16..24].copy_from_slice(&1024u64.to_le_bytes());
    base[24..32].copy_from_slice(&2u64.to_le_bytes());
    base[32..40].copy_from_slice(&1u64.to_le_bytes());
    base[40..48].copy_from_slice(&1u64.to_le_bytes());
    base[64..72].copy_from_slice(&256u64.to_le_bytes());
    base[72..80].copy_from_slice(&512u64.to_le_bytes());
    base[80..88].copy_from_slice(&768u64.to_le_bytes());
    base[88..96].copy_from_slice(&768u64.to_le_bytes());
    base[96..104].copy_from_slice(&768u64.to_le_bytes());
    for (node_id, meta_iblock) in [(0u64, 0u64), (1, 4), (2, 8)] {
        let at = 256 + node_id as usize * 16;
        base[at..at + 8].copy_from_slice(&meta_iblock.to_le_bytes());
    }
    for (node_id, meta_iblock) in [(0u64, 12u64), (1, 16)] {
        let at = 768 + node_id as usize * 16;
        base[at..at + 8].copy_from_slice(&meta_iblock.to_le_bytes());
    }
    let crc = kernel_crc32(&[&base[..1024]]);
    base[1024..1028].copy_from_slice(&crc.to_le_bytes());
    if start + base.len() > image.len() {
        image.resize(start + base.len(), 0);
    }
    image[start..start + base.len()].copy_from_slice(&base);
    Ok(())
}

fn write_superblocks(image: &mut [u8], volume_blocks: u64) -> Result<()> {
    let mut sb = vec![0u8; 512];
    sb[0..4].copy_from_slice(b"VDFS");
    sb[4..8].copy_from_slice(b"1002");
    sb[8..16].copy_from_slice(&volume_blocks.to_le_bytes());
    sb[164] = 12;
    sb[165] = 14;
    let crc = kernel_crc32(&[&sb[..508]]);
    sb[508..512].copy_from_slice(&crc.to_le_bytes());
    image[1024..1536].copy_from_slice(&sb);

    let mut ex = vec![0u8; 2560];
    ex[88..96].copy_from_slice(&BASE_BLOCK.to_le_bytes());
    ex[96..104].copy_from_slice(&1u64.to_le_bytes());
    ex[104..112].copy_from_slice(&NODE_EXTENT_BLOCK.to_le_bytes());
    ex[112..120].copy_from_slice(&64u64.to_le_bytes());
    ex[1640..1648].copy_from_slice(&volume_blocks.to_le_bytes());
    let crc = kernel_crc32(&[&ex[..2556]]);
    ex[2556..2560].copy_from_slice(&crc.to_le_bytes());
    image[1536..4096].copy_from_slice(&ex);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_round_trip_vdfs1002_image() {
        let root = std::env::temp_dir().join(format!("vdfs-tool-vdfs1002-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-vdfs1002-{}.img", std::process::id()));
        let output =
            std::env::temp_dir().join(format!("vdfs-tool-vdfs1002-output-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("hello.txt"), b"hello").unwrap();
        write_image(&root, &image, PackCompression::None).unwrap();

        let bytes = fs::read(&image).unwrap();
        let reader = super::super::Image::open(&bytes).unwrap();
        let records = reader.catalog_records().unwrap();
        assert!(records.iter().any(|record| record.name == b"hello.txt"));

        super::super::restore(&bytes, &output).unwrap();
        assert_eq!(fs::read(output.join("root/hello.txt")).unwrap(), b"hello");

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
    }

    #[test]
    fn writes_compressed_vdfs1002_round_trip() {
        let root =
            std::env::temp_dir().join(format!("vdfs-tool-vdfs1002-zlib-{}", std::process::id()));
        let image = std::env::temp_dir().join(format!(
            "vdfs-tool-vdfs1002-zlib-{}.img",
            std::process::id()
        ));
        let output = std::env::temp_dir().join(format!(
            "vdfs-tool-vdfs1002-zlib-output-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();
        let data = vec![0x5a; 20 * 1024];
        fs::write(root.join("large.bin"), &data).unwrap();

        write_image(&root, &image, PackCompression::Zlib).unwrap();
        let bytes = fs::read(&image).unwrap();
        super::super::restore(&bytes, &output).unwrap();
        assert_eq!(fs::read(output.join("root/large.bin")).unwrap(), data);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
    }
}
