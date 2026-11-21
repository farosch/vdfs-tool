use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};

use super::disk::*;
use crate::cli::{PackCompression, PackLayout};

const BLOCK_SIZE: u64 = 4096;
const TABLE_BLOCK: u64 = 6;
const META_START: u64 = 11;
const DATA_START: u64 = 1024;

#[derive(Clone)]
struct HostEntry {
    path: PathBuf,
    object_id: u64,
    parent_id: u64,
    mode: u16,
    uid: u32,
    gid: u32,
    creation_time: Vdfs4Timespec,
    modification_time: Vdfs4Timespec,
    access_time: Vdfs4Timespec,
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
    hardlink_target: Option<u64>,
}

struct CatalogMetadata {
    mode: u16,
    uid: u32,
    gid: u32,
    creation_time: Vdfs4Timespec,
    modification_time: Vdfs4Timespec,
    access_time: Vdfs4Timespec,
}

pub(crate) fn write_image(
    input: &Path,
    output: &Path,
    compression: PackCompression,
    layout: PackLayout,
) -> Result<()> {
    if !input.is_dir() {
        bail!("Input directory does not exist: {}", input.display());
    }
    if output.exists() {
        bail!("Output image already exists: {}", output.display());
    }

    let mut entries = Vec::new();
    let mut hardlinks = HashMap::new();
    collect_entries(input, 1, &mut entries, &mut 8u64, &mut hardlinks)?;

    let mut image = vec![0u8; (META_START as usize + 4) * BLOCK_SIZE as usize];
    let mut next_block = 4u64;
    let mut catalog = Vec::new();
    let mut xattrs = Vec::new();
    let mut overflow = Vec::new();

    for entry in &entries {
        let metadata = fs::symlink_metadata(&entry.path)
            .with_context(|| format!("Failed to read metadata: {}", entry.path.display()))?;
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name_bytes = name.as_bytes();

        let (record_type, fork) =
            if entry.hardlink_target.is_some() && entry.hardlink_target != Some(entry.object_id) {
                (VDFS4_CATALOG_HLINK_RECORD, None)
            } else if metadata.is_dir() {
                (VDFS4_CATALOG_FOLDER_RECORD, Some(Vdfs4Fork::default()))
            } else if metadata.file_type().is_symlink() {
                let target = fs::read_link(&entry.path)
                    .with_context(|| format!("Failed to read symlink: {}", entry.path.display()))?;
                let target = target.as_os_str().to_string_lossy();
                (
                    VDFS4_CATALOG_FILE_RECORD,
                    Some(inline_fork(target.as_bytes())),
                )
            } else if metadata.is_file() {
                let data = fs::read(&entry.path)
                    .with_context(|| format!("Failed to read file: {}", entry.path.display()))?;
                (
                    VDFS4_CATALOG_FILE_RECORD,
                    Some(build_fork(
                        &data,
                        entry.object_id,
                        compression,
                        layout,
                        &mut image,
                        &mut next_block,
                        &mut overflow,
                    )?),
                )
            } else {
                bail!("Unsupported special file type: {}", entry.path.display());
            };
        let file_flags = if record_type == VDFS4_CATALOG_FILE_RECORD
            && !metadata.is_dir()
            && compression != PackCompression::None
        {
            VDFS4_COMPRESSED_FILE
        } else {
            0
        };

        if record_type == VDFS4_CATALOG_HLINK_RECORD {
            let target = entry.hardlink_target.context("Missing hard-link target")?;
            catalog.push(build_hlink_record(entry.parent_id, target, name_bytes));
        } else {
            let record = build_catalog_record(
                entry.parent_id,
                entry.object_id,
                record_type,
                name_bytes,
                CatalogMetadata {
                    mode: entry.mode,
                    uid: entry.uid,
                    gid: entry.gid,
                    creation_time: entry.creation_time,
                    modification_time: entry.modification_time,
                    access_time: entry.access_time,
                },
                file_flags,
                fork,
            );
            catalog.push(record);
            catalog.push(build_ilink_record(
                entry.parent_id,
                entry.object_id,
                name_bytes,
            ));
        }

        for (name, value) in &entry.xattrs {
            xattrs.push(build_xattr_record(entry.object_id, name, value)?);
        }
    }

    catalog.push(build_catalog_root_record());
    catalog.sort_by(|left, right| catalog_cmp(left, right));
    let (head_page, catalog_pages, cat_meta_pages) = build_catalog_layout(&catalog)?;
    let extents_head = build_head_page(1)?;
    let extents_page = build_leaf_page(&overflow, 1, 0)?;
    let xattrs_head = build_head_page(1)?;
    let xattrs_page = build_leaf_page(&xattrs, 1, 0)?;

    let extents_meta_iblock = cat_meta_pages;
    let xattrs_meta_iblock = cat_meta_pages + 2;
    let meta_pages = cat_meta_pages + 4;
    if image.len() < ((META_START + meta_pages as u64) as usize) * BLOCK_SIZE as usize {
        image.resize(
            ((META_START + meta_pages as u64) as usize) * BLOCK_SIZE as usize,
            0,
        );
    }
    copy_page(&mut image, META_START, &head_page)?;
    for (index, page) in catalog_pages.iter().enumerate() {
        copy_page(&mut image, META_START + 1 + index as u64, page)?;
    }
    copy_page(
        &mut image,
        META_START + extents_meta_iblock as u64,
        &extents_head,
    )?;
    copy_page(
        &mut image,
        META_START + extents_meta_iblock as u64 + 1,
        &extents_page,
    )?;
    copy_page(
        &mut image,
        META_START + xattrs_meta_iblock as u64,
        &xattrs_head,
    )?;
    copy_page(
        &mut image,
        META_START + xattrs_meta_iblock as u64 + 1,
        &xattrs_page,
    )?;

    write_base_table(
        &mut image,
        cat_meta_pages,
        extents_meta_iblock,
        extents_meta_iblock + 1,
        xattrs_meta_iblock,
        xattrs_meta_iblock + 1,
    )?;
    let volume_blocks = (image.len() as u64).div_ceil(BLOCK_SIZE);
    write_superblocks(&mut image, volume_blocks, meta_pages, layout)?;
    fs::write(output, &image)
        .with_context(|| format!("Failed to write image: {}", output.display()))
}

fn collect_entries(
    directory: &Path,
    parent_id: u64,
    entries: &mut Vec<HostEntry>,
    next_id: &mut u64,
    hardlinks: &mut HashMap<u64, u64>,
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
        let (mode, uid, gid, creation_time, modification_time, access_time) =
            host_metadata(&metadata);
        let xattrs = host_xattrs(&path);
        let hardlink_target = host_hardlink_target(&metadata, object_id, hardlinks);

        entries.push(HostEntry {
            path: path.clone(),
            object_id,
            parent_id,
            mode,
            uid,
            gid,
            creation_time,
            modification_time,
            access_time,
            xattrs,
            hardlink_target,
        });

        if metadata.is_dir() {
            collect_entries(&path, object_id, entries, next_id, hardlinks)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn host_metadata(
    metadata: &std::fs::Metadata,
) -> (u16, u32, u32, Vdfs4Timespec, Vdfs4Timespec, Vdfs4Timespec) {
    use std::os::unix::fs::MetadataExt;
    (
        metadata.mode() as u16,
        metadata.uid(),
        metadata.gid(),
        seconds_timespec(metadata.ctime(), metadata.ctime_nsec()),
        seconds_timespec(metadata.mtime(), metadata.mtime_nsec()),
        seconds_timespec(metadata.atime(), metadata.atime_nsec()),
    )
}

#[cfg(not(unix))]
fn host_metadata(
    _metadata: &std::fs::Metadata,
) -> (u16, u32, u32, Vdfs4Timespec, Vdfs4Timespec, Vdfs4Timespec) {
    (
        0o100755,
        0,
        0,
        Vdfs4Timespec::default(),
        Vdfs4Timespec::default(),
        Vdfs4Timespec::default(),
    )
}

fn seconds_timespec(seconds: i64, nanoseconds: i64) -> Vdfs4Timespec {
    let seconds = u64::try_from(seconds).unwrap_or(0);
    Vdfs4Timespec {
        seconds: (seconds & 0xffff_ffff) as u32,
        seconds_high: (seconds >> 32) as u32,
        nanoseconds: u32::try_from(nanoseconds).unwrap_or(0),
    }
}

#[cfg(unix)]
fn host_hardlink_target(
    metadata: &std::fs::Metadata,
    object_id: u64,
    hardlinks: &mut HashMap<u64, u64>,
) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    if !metadata.is_file() || metadata.nlink() <= 1 {
        return None;
    }
    match hardlinks.entry(metadata.ino()) {
        std::collections::hash_map::Entry::Occupied(entry) => Some(*entry.get()),
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(object_id);
            Some(object_id)
        }
    }
}

#[cfg(not(unix))]
fn host_hardlink_target(
    _metadata: &std::fs::Metadata,
    _object_id: u64,
    _hardlinks: &mut HashMap<u64, u64>,
) -> Option<u64> {
    None
}

#[cfg(unix)]
fn host_xattrs(path: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt;
    let mut attrs = Vec::new();
    if let Ok(names) = xattr::list(path) {
        for name in names {
            if let Ok(Some(value)) = xattr::get(path, &name) {
                attrs.push((name.as_bytes().to_vec(), value));
            }
        }
    }
    attrs.sort();
    attrs
}

#[cfg(not(unix))]
fn host_xattrs(_path: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    Vec::new()
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
    metadata: CatalogMetadata,
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
        file_mode: metadata.mode,
        pad: 0,
        uid: metadata.uid,
        gid: metadata.gid,
        creation_time: metadata.creation_time,
        modification_time: metadata.modification_time,
        access_time: metadata.access_time,
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

fn build_catalog_root_record() -> Vec<u8> {
    build_catalog_record(
        VDFS4_ROOTDIR_OBJ_ID,
        1,
        VDFS4_CATALOG_FOLDER_RECORD,
        b"root",
        CatalogMetadata {
            mode: 0o040755,
            uid: 0,
            gid: 0,
            creation_time: Vdfs4Timespec::default(),
            modification_time: Vdfs4Timespec::default(),
            access_time: Vdfs4Timespec::default(),
        },
        0,
        None,
    )
}

fn build_hlink_record(parent_id: u64, target_object_id: u64, name: &[u8]) -> Vec<u8> {
    let key_len = align8(CATTREE_KEY_NAME_OFF + name.len());
    let mut record = vec![0u8; key_len];
    record[0..4].copy_from_slice(b"catt");
    record[4..6].copy_from_slice(&(key_len as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(key_len as u16).to_le_bytes());
    record[8..16].copy_from_slice(&parent_id.to_le_bytes());
    record[16..24].copy_from_slice(&target_object_id.to_le_bytes());
    record[24] = VDFS4_CATALOG_HLINK_RECORD;
    record[25] = name.len() as u8;
    record[26..26 + name.len()].copy_from_slice(name);
    record
}

fn build_xattr_record(object_id: u64, name: &[u8], value: &[u8]) -> Result<Vec<u8>> {
    if name.len() > 200 {
        bail!("VDFS4 xattr name exceeds 200 bytes");
    }
    if value.len() > 255 {
        bail!("VDFS4 xattr value exceeds 255 bytes");
    }
    let name_len = name.len().min(200);
    let key = align8(17 + name_len);
    let size = key.checked_add(align8(1 + value.len())).unwrap_or(key);
    let mut record = vec![0u8; size];
    record[0..4].copy_from_slice(b"XAre");
    record[4..6].copy_from_slice(&(key as u16).to_le_bytes());
    record[6..8].copy_from_slice(&(size as u16).to_le_bytes());
    record[8..16].copy_from_slice(&object_id.to_le_bytes());
    record[16] = name_len as u8;
    record[17..17 + name_len].copy_from_slice(&name[..name_len]);
    record[key] = value.len().min(255) as u8;
    let value_len = value.len().min(255);
    record[key + 1..key + 1 + value_len].copy_from_slice(&value[..value_len]);
    Ok(record)
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
    object_id: u64,
    compression: PackCompression,
    layout: PackLayout,
    image: &mut Vec<u8>,
    next_block: &mut u64,
    overflow: &mut Vec<Vec<u8>>,
) -> Result<Vdfs4Fork> {
    if compression == PackCompression::None {
        if data.len() <= VDFS4_INLINE_DATA_MAX_SIZE {
            return Ok(inline_fork(data));
        }
        return allocate_extent_fork(data, object_id, image, next_block, overflow);
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

    let blob = build_compressed_blob(data, compression, layout)?;
    allocate_blob_fork(&blob, object_id, image, next_block, overflow)
}

fn allocate_extent_fork(
    data: &[u8],
    _object_id: u64,
    image: &mut Vec<u8>,
    next_block: &mut u64,
    _overflow: &mut Vec<Vec<u8>>,
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
        *next_block = DATA_START;
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
        PackCompression::Lzo => Ok(rust_lzo1x::compress(data)),
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

fn build_compressed_blob(
    data: &[u8],
    compression: PackCompression,
    layout: PackLayout,
) -> Result<Vec<u8>> {
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
    let descriptor_size = match layout {
        PackLayout::Layout2006 => 32,
        PackLayout::Layout2007 => 40,
    };
    let mut blob = payload;
    blob.resize(table_offset + extents.len() * 16 + descriptor_size, 0);
    for (index, (start, len, flags)) in extents.iter().enumerate() {
        let at = table_offset + index * 16;
        blob[at..at + 2].copy_from_slice(b"XT");
        blob[at + 2..at + 4].copy_from_slice(&flags.to_le_bytes());
        blob[at + 4..at + 8].copy_from_slice(&(*len as u32).to_le_bytes());
        blob[at + 8..at + 16].copy_from_slice(&(*start as u64).to_le_bytes());
    }

    let descriptor = table_offset + extents.len() * 16;
    match layout {
        PackLayout::Layout2006 => {
            blob[descriptor..descriptor + 4].copy_from_slice(b"C");
            blob[descriptor + 1..descriptor + 4].copy_from_slice(compression_magic(compression));
            blob[descriptor + 4..descriptor + 6]
                .copy_from_slice(&(extents.len() as u16).to_le_bytes());
            blob[descriptor + 6..descriptor + 8]
                .copy_from_slice(&VDFS4_COMPR_LAYOUT_VER_05.to_le_bytes());
            blob[descriptor + 8..descriptor + 16]
                .copy_from_slice(&(data.len() as u64).to_le_bytes());
            blob[descriptor + 20..descriptor + 24].copy_from_slice(&12u32.to_le_bytes());
            let mut descriptor_bytes = blob[descriptor..descriptor + 32].to_vec();
            descriptor_bytes[16..20].fill(0);
            let crc =
                crate::crc::kernel_crc32(&[&blob[table_offset..descriptor], &descriptor_bytes]);
            blob[descriptor + 16..descriptor + 20].copy_from_slice(&crc.to_le_bytes());
        }
        PackLayout::Layout2007 => {
            blob[descriptor + 8..descriptor + 9].copy_from_slice(b"C");
            blob[descriptor + 9..descriptor + 12].copy_from_slice(compression_magic(compression));
            blob[descriptor + 12..descriptor + 14]
                .copy_from_slice(&(extents.len() as u16).to_le_bytes());
            blob[descriptor + 14..descriptor + 16]
                .copy_from_slice(&VDFS4_COMPR_LAYOUT_VER_06.to_le_bytes());
            blob[descriptor + 16..descriptor + 24]
                .copy_from_slice(&(data.len() as u64).to_le_bytes());
            blob[descriptor + 28..descriptor + 32].copy_from_slice(&12u32.to_le_bytes());
        }
    }
    Ok(blob)
}

fn allocate_blob_fork(
    blob: &[u8],
    _object_id: u64,
    image: &mut Vec<u8>,
    next_block: &mut u64,
    _overflow: &mut Vec<Vec<u8>>,
) -> Result<Vdfs4Fork> {
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

fn build_head_page(height: u16) -> Result<Vec<u8>> {
    let mut page = vec![0u8; BLOCK_SIZE as usize];
    page[0..4].copy_from_slice(b"eHND");
    page[4..8].copy_from_slice(&1u32.to_le_bytes());
    page[8..12].copy_from_slice(&0u32.to_le_bytes());
    page[12..16].copy_from_slice(&1u32.to_le_bytes());
    page[16..18].copy_from_slice(&height.to_le_bytes());
    Ok(page)
}

fn build_internal_page(separators: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut page = vec![0u8; BLOCK_SIZE as usize];
    page[0..4].copy_from_slice(b"eHND");
    page[4..8].copy_from_slice(&1u32.to_le_bytes());
    page[8..12].copy_from_slice(&0u32.to_le_bytes());
    page[12..16].copy_from_slice(&0u32.to_le_bytes());
    page[16..18].copy_from_slice(&1u16.to_le_bytes());
    page[18..20].copy_from_slice(&0u16.to_le_bytes());
    page[20..24].copy_from_slice(&1u32.to_le_bytes());
    page[24..28].copy_from_slice(&0u32.to_le_bytes());
    page[28..32].copy_from_slice(&1u32.to_le_bytes());
    page[14..16].copy_from_slice(&(separators.len() as u16).to_le_bytes());

    let mut cursor = 32usize;
    let mut offsets = Vec::new();
    for (index, separator) in separators.iter().enumerate() {
        let key_len = cattree_key_key_len(separator);
        let aligned = (cursor + 7) & !7;
        if aligned + key_len + 4 > BLOCK_SIZE as usize - 4 * (separators.len() + 2) {
            bail!("Catalog internal node exceeds one VDFS4 metadata page");
        }
        page[aligned..aligned + key_len].copy_from_slice(&separator[..key_len]);
        page[aligned + key_len..aligned + key_len + 4]
            .copy_from_slice(&(2u32 + index as u32).to_le_bytes());
        offsets.push(aligned as u32);
        cursor = aligned + key_len + 4;
    }
    offsets.push(cursor as u32);
    for (index, offset) in offsets.iter().enumerate() {
        let at = BLOCK_SIZE as usize - 8 - index * 4;
        page[at..at + 4].copy_from_slice(&offset.to_le_bytes());
    }
    Ok(page)
}

fn build_catalog_layout(records: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<Vec<u8>>, usize)> {
    if let Ok(leaf) = build_leaf_page(records, 1, 0) {
        let head = build_head_page(1)?;
        return Ok((head, vec![leaf], 2));
    }

    let mut leaves = Vec::new();
    let mut separators = Vec::new();
    let mut start = 0usize;
    let mut node_id = 2u32;
    while start < records.len() {
        let mut end = start + 1;
        while end <= records.len() && build_leaf_page(&records[start..end], node_id, 0).is_ok() {
            end += 1;
        }
        if end == start + 1 {
            bail!("Catalog record exceeds one VDFS4 leaf page");
        }
        end -= 1;
        let next = if end < records.len() { node_id + 1 } else { 0 };
        leaves.push(build_leaf_page(&records[start..end], node_id, next)?);
        if end < records.len() {
            separators.push(records[end].clone());
        }
        start = end;
        node_id += 1;
    }
    let internal = build_internal_page(&separators)?;
    let mut pages = vec![internal];
    pages.extend(leaves);
    let head = build_head_page(2)?;
    let meta_pages = 1 + pages.len();
    Ok((head, pages, meta_pages))
}

fn build_leaf_page(records: &[Vec<u8>], node_id: u32, next_node_id: u32) -> Result<Vec<u8>> {
    let mut page = vec![0u8; BLOCK_SIZE as usize];
    page[0..4].copy_from_slice(b"eHND");
    page[4..8].copy_from_slice(&1u32.to_le_bytes());
    page[8..12].copy_from_slice(&0u32.to_le_bytes());
    page[12..16].copy_from_slice(&0u32.to_le_bytes());
    page[16..18].copy_from_slice(&1u16.to_le_bytes());
    page[18..20].copy_from_slice(&0u16.to_le_bytes());
    page[20..24].copy_from_slice(&node_id.to_le_bytes());
    page[24..28].copy_from_slice(&next_node_id.to_le_bytes());
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

fn write_base_table(
    image: &mut [u8],
    cat_meta_pages: usize,
    extents_meta_iblock: usize,
    extents_leaf_iblock: usize,
    xattrs_meta_iblock: usize,
    xattrs_leaf_iblock: usize,
) -> Result<()> {
    let start = TABLE_BLOCK as usize * BLOCK_SIZE as usize;
    let mut table = vec![0u8; BLOCK_SIZE as usize];
    table[0..4].copy_from_slice(b"CoW\x01");
    table[4..8].copy_from_slice(&1u32.to_le_bytes());
    table[16..24].copy_from_slice(&0u64.to_le_bytes());
    table[24..32].copy_from_slice(&((cat_meta_pages.saturating_sub(1)) as u64).to_le_bytes());
    table[32..40].copy_from_slice(&0u64.to_le_bytes());
    table[40..48].copy_from_slice(&1u64.to_le_bytes());
    table[48..56].copy_from_slice(&1u64.to_le_bytes());
    table[56..64].copy_from_slice(&1u64.to_le_bytes());

    let catalog_offset = 104usize;
    let extents_offset = catalog_offset + cat_meta_pages * 16;
    let xattrs_offset = extents_offset + 16;
    table[64..72].copy_from_slice(&(catalog_offset as u64).to_le_bytes());
    table[72..80].copy_from_slice(&0u64.to_le_bytes());
    table[80..88].copy_from_slice(&(extents_offset as u64).to_le_bytes());
    table[88..96].copy_from_slice(&0u64.to_le_bytes());
    table[96..104].copy_from_slice(&(xattrs_offset as u64).to_le_bytes());

    for index in 0..cat_meta_pages {
        let offset = catalog_offset + index * 16;
        table[offset..offset + 8].copy_from_slice(&(index as u64).to_le_bytes());
        table[offset + 8..offset + 12].copy_from_slice(&1u32.to_le_bytes());
        table[offset + 12..offset + 16].copy_from_slice(&0u32.to_le_bytes());
    }
    let offset = extents_offset;
    table[offset..offset + 8].copy_from_slice(&(extents_meta_iblock as u64).to_le_bytes());
    table[offset + 8..offset + 12].copy_from_slice(&1u32.to_le_bytes());
    table[offset + 12..offset + 16].copy_from_slice(&0u32.to_le_bytes());
    table[offset + 16..offset + 24].copy_from_slice(&(extents_leaf_iblock as u64).to_le_bytes());
    table[offset + 24..offset + 28].copy_from_slice(&1u32.to_le_bytes());
    table[offset + 28..offset + 32].copy_from_slice(&0u32.to_le_bytes());
    let offset = xattrs_offset;
    table[offset..offset + 8].copy_from_slice(&(xattrs_meta_iblock as u64).to_le_bytes());
    table[offset + 8..offset + 12].copy_from_slice(&1u32.to_le_bytes());
    table[offset + 12..offset + 16].copy_from_slice(&0u32.to_le_bytes());
    table[offset + 16..offset + 24].copy_from_slice(&(xattrs_leaf_iblock as u64).to_le_bytes());
    table[offset + 24..offset + 28].copy_from_slice(&1u32.to_le_bytes());
    table[offset + 28..offset + 32].copy_from_slice(&0u32.to_le_bytes());
    image[start..start + table.len()].copy_from_slice(&table);
    Ok(())
}

fn write_superblocks(
    image: &mut [u8],
    volume_blocks: u64,
    meta_pages: usize,
    layout: PackLayout,
) -> Result<()> {
    let mut sb = vec![0u8; 512];
    sb[0..4].copy_from_slice(b"VDFS");
    match layout {
        PackLayout::Layout2006 => sb[4..8].copy_from_slice(b"2006"),
        PackLayout::Layout2007 => sb[4..8].copy_from_slice(b"2007"),
    }
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
    exsb[96..104].copy_from_slice(&(meta_pages as u64).to_le_bytes());
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

        write_image(&root, &image, PackCompression::None, PackLayout::Layout2007).unwrap();
        let bytes = fs::read(&image).unwrap();
        assert_eq!(&bytes[1024..1028], b"VDFS");
        assert_eq!(&bytes[1028..1032], b"2007");

        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
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
    fn writes_layout2006_round_trip() {
        let root =
            std::env::temp_dir().join(format!("vdfs-tool-layout2006-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-layout2006-{}.vdfs", std::process::id()));
        let output = std::env::temp_dir().join(format!(
            "vdfs-tool-layout2006-output-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("hello.txt"), b"layout 2006").unwrap();

        write_image(&root, &image, PackCompression::Zlib, PackLayout::Layout2006).unwrap();
        let bytes = fs::read(&image).unwrap();
        assert_eq!(&bytes[1024..1028], b"VDFS");
        assert_eq!(&bytes[1028..1032], b"2006");
        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
        super::super::unpacker::unpack_filesystem(&reader, &output, &[]).unwrap();
        assert_eq!(
            fs::read(output.join("root/hello.txt")).unwrap(),
            b"layout 2006"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
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

        write_image(&root, &image, PackCompression::Zlib, PackLayout::Layout2007).unwrap();
        let bytes = fs::read(&image).unwrap();
        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
        super::super::unpacker::unpack_filesystem(&reader, &output, &[]).unwrap();
        assert_eq!(fs::read(output.join("root/large.bin")).unwrap(), data);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
    }

    fn codec_round_trip(compression: PackCompression) {
        let root = std::env::temp_dir().join(format!(
            "vdfs-tool-{compression:?}-writer-{}",
            std::process::id()
        ));
        let image = std::env::temp_dir().join(format!(
            "vdfs-tool-{compression:?}-writer-{}.vdfs",
            std::process::id()
        ));
        let output = std::env::temp_dir().join(format!(
            "vdfs-tool-{compression:?}-output-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(root.join("sub")).unwrap();

        let data = (0..20 * 1024)
            .map(|index| ((index * 37) % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(root.join("sub/data.bin"), &data).unwrap();

        write_image(&root, &image, compression, PackLayout::Layout2007).unwrap();
        let bytes = fs::read(&image).unwrap();
        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
        super::super::unpacker::unpack_filesystem(&reader, &output, &[]).unwrap();
        assert_eq!(fs::read(output.join("root/sub/data.bin")).unwrap(), data);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&output);
        let _ = fs::remove_file(&image);
    }

    #[test]
    fn gzip_round_trip_restores_large_file() {
        codec_round_trip(PackCompression::Gzip);
    }

    #[test]
    fn zstd_round_trip_restores_large_file() {
        codec_round_trip(PackCompression::Zstd);
    }

    #[test]
    fn lzo_round_trip_restores_large_file() {
        codec_round_trip(PackCompression::Lzo);
    }

    #[test]
    fn packs_large_catalog_across_leaf_pages() {
        let root = std::env::temp_dir().join(format!("vdfs-tool-catalog-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-catalog-{}.vdfs", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();
        for index in 0..500 {
            fs::write(root.join(format!("file-{index:04}.bin")), b"x").unwrap();
        }

        write_image(&root, &image, PackCompression::None, PackLayout::Layout2007).unwrap();
        let bytes = fs::read(&image).unwrap();
        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
        assert!(reader.snapshot.tables[0].len() >= 2);
        let cat = super::super::cattree::open_cat_tree(&reader).unwrap();
        let records = cat.collect_all_records().unwrap();
        assert!(
            records
                .iter()
                .filter(|record| {
                    record.len() >= CATTREE_KEY_NAME_OFF
                        && cattree_key_record_type(record) == VDFS4_CATALOG_FILE_RECORD
                })
                .count()
                >= 500
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifo_and_preserves_hardlink_identity() {
        let root = std::env::temp_dir().join(format!("vdfs-tool-special-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-special-{}.vdfs", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();

        let fifo = root.join("pipe");
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(write_image(&root, &image, PackCompression::None, PackLayout::Layout2007).is_err());
        let _ = fs::remove_file(&fifo);

        fs::write(root.join("a.bin"), b"hardlink").unwrap();
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        write_image(&root, &image, PackCompression::None, PackLayout::Layout2007).unwrap();
        let bytes = fs::read(&image).unwrap();
        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
        let cat = super::super::cattree::open_cat_tree(&reader).unwrap();
        let records = cat.collect_all_records().unwrap();
        assert!(records.iter().any(|record| {
            cattree_key_record_type(record) == VDFS4_CATALOG_HLINK_RECORD
                && cattree_key_name(record) == b"b.bin"
        }));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
    }

    #[cfg(unix)]
    #[test]
    fn writes_host_xattrs_and_large_file_overflow_records() {
        let root = std::env::temp_dir().join(format!("vdfs-tool-xattr-{}", std::process::id()));
        let image =
            std::env::temp_dir().join(format!("vdfs-tool-xattr-{}.vdfs", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("large.bin");
        fs::write(&file, vec![0x33; 4096 * 11 + 17]).unwrap();
        xattr::set(&file, "user.vdfs_test", b"value").unwrap();

        write_image(&root, &image, PackCompression::None, PackLayout::Layout2007).unwrap();
        let bytes = fs::read(&image).unwrap();
        let reader = super::super::reader::ImageReader::new_with_keys(&bytes, &[]).unwrap();
        let cat = super::super::cattree::open_cat_tree(&reader).unwrap();
        let records = cat.collect_all_records().unwrap();
        let file_record = records
            .iter()
            .find(|record| cattree_key_name(record) == b"large.bin")
            .unwrap();
        let object_id = cattree_key_object_id(file_record);
        let key_len = cattree_key_key_len(file_record);
        let file: Vdfs4CatalogFileRecord =
            unsafe { read_packed(&file_record[key_len..], 0).unwrap() };
        let total_blocks = file.fork.total_blocks_count;
        let extent_length = file.fork.extents()[0].extent.length;
        assert_eq!(total_blocks, 12);
        assert_eq!(extent_length, 12);

        let xattr_tree = super::super::btree::Btree::open(&reader, VDFS4_XATTR_TREE_INO).unwrap();
        let xattr_records = xattr_tree.collect_all_records().unwrap();
        assert!(xattr_records.iter().any(|record| {
            record.len() >= 17
                && read_u64_le(record, 8) == object_id
                && record[17..]
                    .windows(b"user.vdfs_test".len())
                    .any(|window| window == b"user.vdfs_test")
        }));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&image);
    }
}
