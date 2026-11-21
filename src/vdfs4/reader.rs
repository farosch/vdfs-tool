use anyhow::{Context, Result, bail};

use super::disk::*;
use super::snapshot::SnapshotInfo;
use super::snapshot::{get_meta_iblock, meta_iblock_to_block};
use super::superblock::SuperblockInfo;

pub struct ImageReader<'a> {
    pub image: &'a [u8],
    pub sbi: SuperblockInfo,
    pub snapshot: SnapshotInfo,
    /// 16-byte AES-128 key for encrypted chunks; `None` if no key was supplied.
    pub aes_key: Option<[u8; 16]>,
    /// RSA public keys trusted for VDFS4 descriptor and superblock signatures.
    pub rsa_public_keys: Vec<rsa::RsaPublicKey>,
}

impl<'a> ImageReader<'a> {
    pub fn new_with_keys(image: &'a [u8], rsa_public_keys: &[rsa::RsaPublicKey]) -> Result<Self> {
        let sbi = super::superblock::read_superblock_with_keys(image, rsa_public_keys)?;
        let snapshot = super::snapshot::build_snapshot_manager(&sbi, image)?;
        Ok(Self {
            image,
            sbi,
            snapshot,
            aes_key: None,
            rsa_public_keys: rsa_public_keys.to_vec(),
        })
    }

    pub fn read_meta_page(&self, ino: u64, page_index: usize) -> Result<Vec<u8>> {
        let meta_iblock = get_meta_iblock(&self.snapshot, &self.sbi, ino, page_index)
            .context("get_meta_iblock failed")?;

        let block_nr =
            meta_iblock_to_block(&self.sbi, meta_iblock).context("meta_iblock_to_block failed")?;
        let byte_offset = block_nr
            .checked_mul(self.sbi.block_size as u64)
            .context("Meta page byte offset overflows")?;
        let node_size = (self.sbi.block_size as u64)
            .checked_mul(self.sbi.blocks_per_super_page() as u64)
            .context("Meta page size overflows")?;
        let byte_end = byte_offset
            .checked_add(node_size)
            .context("Meta page range overflows")?;

        if byte_end > self.image.len() as u64 {
            bail!(
                "Meta page out of bounds: block={}, byte_offset={}, node_size={}",
                block_nr,
                byte_offset,
                node_size
            );
        }
        let start = usize::try_from(byte_offset).context("Meta page byte offset is too large")?;
        let end = usize::try_from(byte_end).context("Meta page end is too large")?;
        Ok(self.image[start..end].to_vec())
    }

    pub fn read_bnode_data(&self, tree_ino: u64, node_id: u32) -> Result<Vec<u8>> {
        if node_id == 0 {
            return self.read_meta_page(tree_ino, 0);
        }

        let blocks_per_node = self.sbi.blocks_per_super_page();
        let pages_per_node = blocks_per_node * self.sbi.block_size / 4096;

        let node_page = node_id as usize * pages_per_node;
        self.read_meta_page(tree_ino, node_page)
    }

    pub fn read_extent_data(&self, extent: &Vdfs4Extent) -> Result<Vec<u8>> {
        if extent.is_empty() {
            return Ok(Vec::new());
        }
        let begin = extent.begin;
        let length = extent.length;
        if length > self.sbi.volume_blocks_count + 16 {
            bail!(
                "Extent length too large: begin={}, length={}",
                begin,
                length
            );
        }
        let offset = match begin
            .checked_mul(self.sbi.block_size as u64)
            .and_then(|v| usize::try_from(v).ok())
        {
            Some(v) => v,
            None => bail!("Extent begin too large: begin={}", begin),
        };
        let len = match length
            .checked_mul(self.sbi.block_size as u64)
            .and_then(|v| usize::try_from(v).ok())
        {
            Some(v) => v,
            None => bail!("Extent length too large: length={}", length),
        };
        if offset
            .checked_add(len)
            .map(|end| end > self.image.len())
            .unwrap_or(true)
        {
            bail!("Extent out of bounds: begin={}, length={}", begin, length);
        }
        Ok(self.image[offset..offset + len].to_vec())
    }
}
