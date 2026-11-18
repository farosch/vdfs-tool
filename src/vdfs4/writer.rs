use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};

use super::disk::*;
use crate::cli::PackCompression;

const BLOCK_SIZE: u64 = 4096;
const TABLE_BLOCK: u64 = 6;
const META_START: u64 = 11;

#[derive(Clone)]
struct HostEntry {
    path: PathBuf,
    object_id: u64,
    parent_id: u64,
}

pub(crate) fn write_image(input: &Path, output: &Path, compression: PackCompression) -> Result<()> {
    if !input.is_dir() {
        bail!("Input directory does not exist: {}", input.display());
    }
    if output.exists() {
        bail!("Output image already exists: {}", output.display());
    }

    let mut entries = Vec::new();
    collect_entries(input, VDFS4_ROOTDIR_OBJ_ID, &mut entries, &mut 8u64)?;

    let mut image = vec![0u8; (META_START as usize + 4) * BLOCK_SIZE as usize];
    let mut next_block = 4u64;
    let mut catalog = Vec::new();

    for entry in &entries {
        let metadata = fs::symlink_metadata(&entry.path)
            .with_context(|| format!("Failed to read metadata: {}", entry.path.display()))?;
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name_bytes = name.as_bytes();

        let (record_type, mode, fork) = if metadata.is_dir() {
            (VDFS4_CATALOG_FOLDER_RECORD, 0o040755, Vdfs4Fork::default())
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&entry.path)
                .with_context(|| format!("Failed to read symlink: {}", entry.path.display()))?;
            let target = target.as_os_str().to_string_lossy();
            (
                VDFS4_CATALOG_FILE_RECORD,
                0o120777,
                inline_fork(target.as_bytes()),
            )
        } else {
            let data = fs::read(&entry.path)
                .with_context(|| format!("Failed to read file: {}", entry.path.display()))?;
            (
                VDFS4_CATALOG_FILE_RECORD,
                0o100755,
                build_fork(&data, compression, &mut image, &mut next_block)?,
            )
        };
        let file_flags = if !metadata.is_dir() && compression != PackCompression::None {
            VDFS4_COMPRESSED_FILE
        } else {
            0
        };

        let record = build_catalog_record(
            entry.parent_id,
            entry.object_id,
            record_type,
            name_bytes,
            mode,
            file_flags,
            if metadata.is_dir() { None } else { Some(fork) },
        );
        catalog.push(record);
        catalog.push(build_ilink_record(
            entry.parent_id,
            entry.object_id,
            name_bytes,
        ));
    }

    catalog.sort_by(|left, right| catalog_cmp(left, right));
    let head_page = build_head_page()?;
    let catalog_page = build_leaf_page(&catalog, 1)?;
    let extents_page = build_leaf_page(&[], VDFS4_EXTENTS_TREE_INO as u32)?;
    let xattrs_page = build_leaf_page(&[], VDFS4_XATTR_TREE_INO as u32)?;

    copy_page(&mut image, META_START, &head_page)?;
    copy_page(&mut image, META_START + 1, &catalog_page)?;
    copy_page(&mut image, META_START + 2, &extents_page)?;
    copy_page(&mut image, META_START + 3, &xattrs_page)?;

    write_base_table(&mut image)?;
    let volume_blocks = (image.len() as u64).div_ceil(BLOCK_SIZE);
    write_superblocks(&mut image, volume_blocks)?;
    fs::write(output, &image)
        .with_context(|| format!("Failed to write image: {}", output.display()))
}

fn collect_entries(
    directory: &Path,
    parent_id: u64,
    entries: &mut Vec<HostEntry>,
    next_id: &mut u64,
) -> Result<()> {
    let mut children = fs::read_dir(directory)
        .with_context(|| format!("Failed to read directory: {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let path = child.path();
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("Failed to read metadata: {}", path.display()))?;
        let object_id = *next_id;
        *next_id = next_id.checked_add(1).context("Object ID overflow")?;

        entries.push(HostEntry {
            path: path.clone(),
            object_id,
            parent_id,
        });

        if metadata.is_dir() {
            collect_entries(&path, object_id, entries, next_id)?;
        }
    }
    Ok(())
}

fn align8(value: usize) -> usize {
    (value + 7) & !7
}

fn catalog_cmp(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    if left.len() < CATTREE_KEY_NAME_OFF || right.len() < CATTREE_KEY_NAME_OFF {
        return left.cmp(right);
    }
    cattree_key_parent_id(left)
        .cmp(&cattree_key_parent_id(right))
        .then_with(|| cattree_key_name(left).cmp(cattree_key_name(right)))
        .then_with(|| cattree_key_name_len(left).cmp(&cattree_key_name_len(right)))
        .then_with(|| cattree_key_object_id(left).cmp(&cattree_key_object_id(right)))
}

fn build_catalog_record(
    parent_id: u64,
    object_id: u64,
    record_type: u8,
    name: &[u8],
    mode: u16,
    flags: u32,
    fork: Option<Vdfs4Fork>,
) -> Vec<u8> {
    let key_len = align8(CATTREE_KEY_NAME_OFF + name.len());
    let value_len = if record_type == VDFS4_CATALOG_FOLDER_RECORD {
        std::mem::size_of::<Vdfs4CatalogFolderRecord>()
    } else {
        std::mem::size_of::<Vdfs4CatalogFileRecord>()
    };
    let record_len = key_len + value_len;
    let mut record = vec![0u8; record_len];

    record[0..4].copy_from_slice(b"catt");
    record[4..6].copy_from_slice(&(key_len as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(record_len as u16).to_le_bytes());
    record[8..16].copy_from_slice(&parent_id.to_le_bytes());
    record[16..24].copy_from_slice(&object_id.to_le_bytes());
    record[24] = record_type;
    record[25] = name.len() as u8;
    record[26..26 + name.len()].copy_from_slice(name);

    let common = Vdfs4CatalogFolderRecord {
        flags,
        generation: 1,
        total_items_count: 0,
        links_count: 0,
        next_orphan_id: 0,
        file_mode: mode,
        pad: 0,
        uid: 0,
        gid: 0,
        creation_time: Vdfs4Timespec::default(),
        modification_time: Vdfs4Timespec::default(),
        access_time: Vdfs4Timespec::default(),
    };

    if record_type == VDFS4_CATALOG_FOLDER_RECORD {
        record[key_len..].copy_from_slice(&serialize_packed(&common));
    } else {
        let file = Vdfs4CatalogFileRecord {
            common,
            fork: fork.unwrap_or_default(),
        };
        record[key_len..].copy_from_slice(&serialize_packed(&file));
    }
    record
}

fn build_ilink_record(parent_id: u64, object_id: u64, name: &[u8]) -> Vec<u8> {
    let key_len = align8(CATTREE_KEY_NAME_OFF + name.len());
    let mut record = vec![0u8; key_len];
    record[0..4].copy_from_slice(b"catt");
    record[4..6].copy_from_slice(&(key_len as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(key_len as u16).to_le_bytes());
    record[8..16].copy_from_slice(&object_id.to_le_bytes());
    record[16..24].copy_from_slice(&parent_id.to_le_bytes());
    record[24] = VDFS4_CATALOG_ILINK_RECORD;
    record[25] = name.len() as u8;
    record[26..26 + name.len()].copy_from_slice(name);
    record
}

fn serialize_packed<T: Copy>(value: &T) -> Vec<u8> {
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
            .to_vec()
    }
}

fn inline_fork(data: &[u8]) -> Vdfs4Fork {
    let mut fork = Vdfs4Fork {
        size_in_bytes: data.len() as u64,
        ..Vdfs4Fork::default()
    };
    let len = data.len().min(VDFS4_INLINE_DATA_MAX_SIZE);
    fork.data[..len].copy_from_slice(&data[..len]);
    fork
}

fn build_fork(
    data: &[u8],
    compression: PackCompression,
    image: &mut Vec<u8>,
    next_block: &mut u64,
) -> Result<Vdfs4Fork> {
    if compression == PackCompression::None {
        if data.len() <= VDFS4_INLINE_DATA_MAX_SIZE {
            return Ok(inline_fork(data));
        }
        return allocate_extent_fork(data, image, next_block);
    }

    if data.len() <= VDFS4_INLINE_DATA_MAX_SIZE {
        let compressed = compress_inline(data, compression)?;
        if compressed.len() + 4 <= VDFS4_INLINE_DATA_MAX_SIZE {
            let mut fork = Vdfs4Fork {
                size_in_bytes: data.len() as u64,
                ..Vdfs4Fork::default()
            };
            fork.data[0..3].copy_from_slice(compression_magic(compression));
            fork.data[3] = compressed.len() as u8;
            fork.data[4..4 + compressed.len()].copy_from_slice(&compressed);
            return Ok(fork);
        }
    }

    let blob = build_compressed_blob(data, compression)?;
    allocate_blob_fork(&blob, image, next_block)
}

fn allocate_extent_fork(
    data: &[u8],
    image: &mut Vec<u8>,
    next_block: &mut u64,
) -> Result<Vdfs4Fork> {
    let blocks = data.len().div_ceil(BLOCK_SIZE as usize) as u64;
    let begin = allocate_blocks(image, next_block, blocks)?;
    let start = begin as usize * BLOCK_SIZE as usize;
    if start + data.len() > image.len() {
        bail!("File extent exceeds image");
    }
    image[start..start + data.len()].copy_from_slice(data);

    let mut fork = Vdfs4Fork {
        size_in_bytes: data.len() as u64,
        total_blocks_count: blocks,
        ..Vdfs4Fork::default()
    };
    fork.data[..24].copy_from_slice(&serialize_packed(&Vdfs4IExtent {
        extent: Vdfs4Extent {
            begin,
            length: blocks,
        },
        iblock: 0,
    }));
    Ok(fork)
}

fn allocate_blocks(image: &mut Vec<u8>, next_block: &mut u64, blocks: u64) -> Result<u64> {
    if *next_block >= META_START {
        *next_block = META_START + 4;
    }
    if *next_block == TABLE_BLOCK {
        *next_block = TABLE_BLOCK + 1;
    }
    let end_before_reserved = next_block
        .checked_add(blocks)
        .context("Image block range overflow")?;
    if *next_block < TABLE_BLOCK && end_before_reserved > TABLE_BLOCK {
        *next_block = TABLE_BLOCK + 1;
    }
    let end_after_table = next_block
        .checked_add(blocks)
        .context("Image block range overflow")?;
    if *next_block > TABLE_BLOCK && end_after_table >= META_START {
        *next_block = META_START + 4;
    }
    let begin = *next_block;
    let end = begin
        .checked_add(blocks)
        .context("Image block range overflow")?;
    if end > META_START {
        let required = end.checked_mul(BLOCK_SIZE).context("Image size overflow")?;
        if image.len() < required as usize {
            image.resize(required as usize, 0);
        }
    }
    *next_block = end;
    Ok(begin)
}

fn compress_inline(data: &[u8], compression: PackCompression) -> Result<Vec<u8>> {
    match compression {
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
        PackCompression::Lzo => bail!("LZO compression is not implemented in the native writer"),
        PackCompression::None => Ok(data.to_vec()),
    }
}

fn compression_magic(compression: PackCompression) -> &'static [u8; 3] {
    match compression {
        PackCompression::Zlib => b"Zip",
        PackCompression::Gzip => b"Gzp",
        PackCompression::Lzo => b"Lzo",
        PackCompression::Zstd => b"Zst",
        PackCompression::None => b"Zip",
    }
}

fn build_compressed_blob(data: &[u8], compression: PackCompression) -> Result<Vec<u8>> {
    let chunk_size = BLOCK_SIZE as usize;
    let mut payload = Vec::new();
    let mut extents = Vec::new();

    for chunk in data.chunks(chunk_size) {
        let compressed = compress_inline(chunk, compression)?;
        let (stored, flags) = if compressed.len() >= chunk.len() {
            (chunk, VDFS4_CHUNK_FLAG_UNCOMPR)
        } else {
            (compressed.as_slice(), 0)
        };
        let start = payload.len();
        payload.extend_from_slice(stored);
        extents.push((start, stored.len(), flags));
    }

    let table_offset = payload.len();
    let mut blob = payload;
    blob.resize(table_offset + extents.len() * 16 + 40, 0);
    for (index, (start, len, flags)) in extents.iter().enumerate() {
        let at = table_offset + index * 16;
        blob[at..at + 2].copy_from_slice(b"XT");
        blob[at + 2..at + 4].copy_from_slice(&flags.to_le_bytes());
        blob[at + 4..at + 8].copy_from_slice(&(*len as u32).to_le_bytes());
        blob[at + 8..at + 16].copy_from_slice(&(*start as u64).to_le_bytes());
    }

    let descriptor = table_offset + extents.len() * 16;
    blob[descriptor + 8..descriptor + 9].copy_from_slice(b"C");
    blob[descriptor + 9..descriptor + 12].copy_from_slice(compression_magic(compression));
    blob[descriptor + 12..descriptor + 14].copy_from_slice(&(extents.len() as u16).to_le_bytes());
    blob[descriptor + 14..descriptor + 16]
        .copy_from_slice(&VDFS4_COMPR_LAYOUT_VER_06.to_le_bytes());
    blob[descriptor + 16..descriptor + 24].copy_from_slice(&(data.len() as u64).to_le_bytes());
    blob[descriptor + 28..descriptor + 32].copy_from_slice(&12u32.to_le_bytes());
    Ok(blob)
}

fn allocate_blob_fork(blob: &[u8], image: &mut Vec<u8>, next_block: &mut u64) -> Result<Vdfs4Fork> {
    let blocks = blob.len().div_ceil(BLOCK_SIZE as usize) as u64;
    let begin = allocate_blocks(image, next_block, blocks)?;
    let start = begin as usize * BLOCK_SIZE as usize;
    if start + blob.len() > image.len() {
        bail!("Compressed blob exceeds image");
    }
    image[start..start + blob.len()].copy_from_slice(blob);

    let mut fork = Vdfs4Fork {
        size_in_bytes: blob.len() as u64,
        total_blocks_count: blocks,
        ..Vdfs4Fork::default()
    };
    fork.data[..24].copy_from_slice(&serialize_packed(&Vdfs4IExtent {
        extent: Vdfs4Extent {
            begin,
            length: blocks,
        },
        iblock: 0,
    }));
    Ok(fork)
}

fn build_head_page() -> Result<Vec<u8>> {
    let mut page = vec![0u8; BLOCK_SIZE as usize];
    page[0..4].copy_from_slice(b"eHND");
    page[4..8].copy_from_slice(&1u32.to_le_bytes());
    page[8..12].copy_from_slice(&0u32.to_le_bytes());
    page[12..16].copy_from_slice(&1u32.to_le_bytes());
    page[16..18].copy_from_slice(&1u16.to_le_bytes());
    Ok(page)
}

fn build_leaf_page(records: &[Vec<u8>], node_id: u32) -> Result<Vec<u8>> {
    let mut page = vec![0u8; BLOCK_SIZE as usize];
    page[0..4].copy_from_slice(b"eHND");
    page[4..8].copy_from_slice(&1u32.to_le_bytes());
    page[8..12].copy_from_slice(&0u32.to_le_bytes());
    page[12..16].copy_from_slice(&0u32.to_le_bytes());
    page[16..18].copy_from_slice(&1u16.to_le_bytes());
    page[18..20].copy_from_slice(&0u16.to_le_bytes());
    page[20..24].copy_from_slice(&node_id.to_le_bytes());
    page[24..28].copy_from_slice(&0u32.to_le_bytes());
    page[28..32].copy_from_slice(&VDFS4_NODE_LEAF.to_le_bytes());
    page[14..16].copy_from_slice(&(records.len() as u16).to_le_bytes());

    let mut cursor = 32usize;
    let mut offsets = Vec::new();
    for record in records {
        let aligned = (cursor + 7) & !7;
        if aligned + record.len() > BLOCK_SIZE as usize - 4 * (records.len() + 2) {
            bail!("B-tree records exceed one VDFS4 metadata page");
        }
        page[aligned..aligned + record.len()].copy_from_slice(record);
        offsets.push(aligned as u32);
        cursor = aligned + record.len();
        if cursor > BLOCK_SIZE as usize {
            bail!("B-tree record overflow");
        }
    }
    offsets.push(cursor as u32);

    for (index, offset) in offsets.iter().enumerate() {
        let at = BLOCK_SIZE as usize - 8 - index * 4;
        page[at..at + 4].copy_from_slice(&offset.to_le_bytes());
    }
    Ok(page)
}

fn copy_page(image: &mut [u8], block: u64, page: &[u8]) -> Result<()> {
    let start = block as usize * BLOCK_SIZE as usize;
    if start + page.len() > image.len() {
        bail!("Metadata page exceeds image");
    }
    image[start..start + page.len()].copy_from_slice(page);
    Ok(())
}

fn write_base_table(image: &mut [u8]) -> Result<()> {
    let start = TABLE_BLOCK as usize * BLOCK_SIZE as usize;
    let mut table = vec![0u8; BLOCK_SIZE as usize];
    table[0..4].copy_from_slice(b"CoW\x01");
    table[4..8].copy_from_slice(&1u32.to_le_bytes());
    table[24..32].copy_from_slice(&1u64.to_le_bytes());
    table[40..48].copy_from_slice(&1u64.to_le_bytes());
    table[56..64].copy_from_slice(&1u64.to_le_bytes());
    table[64..72].copy_from_slice(&104u64.to_le_bytes());
    table[72..80].copy_from_slice(&120u64.to_le_bytes());
    table[80..88].copy_from_slice(&136u64.to_le_bytes());
    table[88..96].copy_from_slice(&152u64.to_le_bytes());
    table[96..104].copy_from_slice(&168u64.to_le_bytes());

    for (offset, meta_iblock) in [
        (104usize, 0u64),
        (120usize, 1u64),
        (136usize, 2u64),
        (152usize, 2u64),
        (168usize, 3u64),
    ] {
        table[offset..offset + 8].copy_from_slice(&meta_iblock.to_le_bytes());
        table[offset + 8..offset + 12].copy_from_slice(&1u32.to_le_bytes());
        table[offset + 12..offset + 16].copy_from_slice(&0u32.to_le_bytes());
    }
    image[start..start + table.len()].copy_from_slice(&table);
    Ok(())
}

fn write_superblocks(image: &mut [u8], volume_blocks: u64) -> Result<()> {
    let mut sb = vec![0u8; 512];
    sb[0..4].copy_from_slice(b"VDFS");
    sb[4..8].copy_from_slice(b"2007");
    sb[8..16].copy_from_slice(&volume_blocks.to_le_bytes());
    sb[164] = 12;
    sb[165] = 12;
    sb[166] = 3;
    sb[167] = 0;
    sb[168] = 1;
    for offset in [0usize, 512, 1024] {
        image[offset..offset + 512].copy_from_slice(&sb);
    }

    let mut exsb = vec![0u8; 2560];
    exsb[16..24].copy_from_slice(&0u64.to_le_bytes());
    exsb[24..32].copy_from_slice(&volume_blocks.to_le_bytes());
    exsb[72..80].copy_from_slice(&TABLE_BLOCK.to_le_bytes());
    exsb[80..88].copy_from_slice(&1u64.to_le_bytes());
    exsb[88..96].copy_from_slice(&META_START.to_le_bytes());
    exsb[96..104].copy_from_slice(&4u64.to_le_bytes());
    exsb[1640..1648].copy_from_slice(&volume_blocks.to_le_bytes());
    image[1536..1536 + 2560].copy_from_slice(&exsb);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_round_trip_vdfs4_image() {
        let root = std::env::temp_dir().join(format!("vdfs-tool-writer-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-writer-{}.vdfs", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/hello.txt"), b"hello").unwrap();

        write_image(&root, &image, PackCompression::None).unwrap();
        let bytes = fs::read(&image).unwrap();
        assert_eq!(&bytes[1024..1028], b"VDFS");
        assert_eq!(&bytes[1028..1032], b"2007");

        let reader = super::super::reader::ImageReader::new(&bytes).unwrap();
        assert_eq!(reader.snapshot.tables[0].len(), 2);
        let cat = super::super::cattree::open_cat_tree(&reader).unwrap();
        let records = cat.collect_all_records().unwrap();
        assert!(records.iter().any(|record| {
            record.len() >= CATTREE_KEY_NAME_OFF && cattree_key_name(record) == b"hello.txt"
        }));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
    }

    #[test]
    fn zlib_round_trip_restores_large_file() {
        let root =
            std::env::temp_dir().join(format!("vdfs-tool-zlib-writer-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-zlib-writer-{}.vdfs", std::process::id()));
        let output =
            std::env::temp_dir().join(format!("vdfs-tool-zlib-output-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();
        let data = vec![0x5a; 20 * 1024];
        fs::write(root.join("large.bin"), &data).unwrap();

        write_image(&root, &image, PackCompression::Zlib).unwrap();
        let bytes = fs::read(&image).unwrap();
        let reader = super::super::reader::ImageReader::new(&bytes).unwrap();
        super::super::unpacker::unpack_filesystem(&reader, &output, &[]).unwrap();
        assert_eq!(fs::read(output.join("large.bin")).unwrap(), data);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
    }
}
