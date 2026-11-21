use anyhow::{Context, Result, bail};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::disk::*;

pub struct SuperblockInfo {
    pub layout: Vdfs4LayoutSb,
    pub block_size: usize,
    pub block_size_shift: u32,
    pub log_super_page_size: u32,
    pub log_blocks_in_leb: u32,
    pub volume_blocks_count: u64,
    pub case_insensitive: bool,
    pub superblock_signature: Option<SuperblockSignatureStatus>,
}

#[derive(Clone, Copy, Debug)]
pub struct SuperblockSignatureStatus {
    pub sign_type: u8,
    pub hash_type: u8,
    pub verified: Option<bool>,
}

impl SuperblockInfo {
    pub fn super_page_size(&self) -> usize {
        1 << self.log_super_page_size as usize
    }

    pub fn blocks_per_super_page(&self) -> usize {
        1 << self.log_blocks_in_leb as usize
    }
}

pub fn read_superblock_with_keys(
    data: &[u8],
    rsa_public_keys: &[rsa::RsaPublicKey],
) -> Result<SuperblockInfo> {
    if data.len() < SECTOR_SIZE * 8 {
        bail!("Image too small for superblock");
    }

    let sb_offset = SECTOR_SIZE * 2;

    let sb: Vdfs4SuperBlock =
        unsafe { read_packed(data, sb_offset).context("Failed to read superblock")? };

    if sb.signature != *VDFS4_SB_SIGNATURE {
        let sig = String::from_utf8_lossy(&sb.signature);
        bail!("Bad superblock signature: {:?} (expected VDFS)", sig);
    }

    let layout_ver = &sb.layout_version;
    if layout_ver != VDFS4_LAYOUT_VERSION_2007 && layout_ver != VDFS4_LAYOUT_VERSION_2006 {
        let ver = String::from_utf8_lossy(layout_ver);
        bail!("Unsupported layout version: {}", ver);
    }

    if sb.image_crc32_present != 0 {
        let calculated = crc32fast::hash(&data[sb_offset..sb_offset + SECTOR_SIZE - 4]);
        let sb_checksum = sb.checksum;
        if calculated != sb_checksum {
            bail!(
                "Superblock CRC mismatch: stored={:#x}, calculated={:#x}",
                sb_checksum,
                calculated
            );
        }
    }

    let superblock_signature = if sb.sign_type != 0 {
        let (signature_len, signature) = match sb.sign_type {
            1 => (128usize, &sb.sb_hash[128..256]),
            2 => (256usize, sb.sb_hash.as_slice()),
            _ => bail!(
                "Unsupported VDFS4 superblock signature type {}",
                sb.sign_type
            ),
        };
        let signed_len = SECTOR_SIZE
            .checked_sub(4)
            .and_then(|len| len.checked_sub(signature_len))
            .context("VDFS4 superblock signature range overflow")?;
        let signed = data
            .get(sb_offset..sb_offset + signed_len)
            .context("VDFS4 superblock signed range is out of bounds")?;
        let digest: Vec<u8> = match sb.hash_type {
            1 => Sha1::digest(signed).to_vec(),
            2 => Sha256::digest(signed).to_vec(),
            3 => Md5::digest(signed).to_vec(),
            _ => bail!("Unsupported VDFS4 superblock hash type {}", sb.hash_type),
        };
        let verified = if rsa_public_keys.is_empty() {
            None
        } else {
            let mut verified = false;
            let mut errors = Vec::new();
            for public_key in rsa_public_keys {
                match super::authentication::verify_digest_signature(public_key, &digest, signature)
                {
                    Ok(()) => {
                        verified = true;
                        break;
                    }
                    Err(error) => errors.push(format!("{error:#}")),
                }
            }
            if !verified {
                bail!(
                    "VDFS4 superblock RSA signature did not verify against any trusted key: {}",
                    errors.join("; ")
                );
            }
            Some(true)
        };
        Some(SuperblockSignatureStatus {
            sign_type: sb.sign_type,
            hash_type: sb.hash_type,
            verified,
        })
    } else {
        None
    };

    if sb.log_block_size >= 32 {
        bail!("Unsupported block size order: {}", sb.log_block_size);
    }
    let block_size = 1u32 << sb.log_block_size;
    if !matches!(block_size, 512 | 1024 | 2048 | 4096) {
        bail!("Unsupported block size: {}", block_size);
    }

    if sb.log_super_page_size < sb.log_block_size {
        bail!(
            "Super page order ({}) < block size order ({})",
            sb.log_super_page_size,
            sb.log_block_size
        );
    }
    if sb.log_super_page_size >= 32 {
        bail!(
            "Unsupported super page size order: {}",
            sb.log_super_page_size
        );
    }

    let exsb_offset = SECTOR_SIZE * 3;
    let exsb_size = SECTOR_SIZE * 5;
    let exsb: Vdfs4ExtendedSuperBlock =
        unsafe { read_packed(data, exsb_offset).context("Failed to read extended superblock")? };

    if sb.image_crc32_present != 0 {
        let exsb_calc = crc32fast::hash(&data[exsb_offset..exsb_offset + exsb_size - 4]);
        let exsb_checksum = exsb.checksum;
        if exsb_calc != exsb_checksum {
            bail!(
                "Extended SB CRC mismatch: stored={:#x}, calculated={:#x}",
                exsb_checksum,
                exsb_calc
            );
        }
    }

    let layout: Vdfs4LayoutSb =
        unsafe { read_packed(data, 0).context("Failed to read full layout superblock")? };

    let info = SuperblockInfo {
        layout,
        block_size: block_size as usize,
        block_size_shift: sb.log_block_size as u32,
        log_super_page_size: sb.log_super_page_size as u32,
        log_blocks_in_leb: sb.log_super_page_size as u32 - sb.log_block_size as u32,
        volume_blocks_count: exsb.volume_blocks_count,
        case_insensitive: sb.case_insensitive != 0,
        superblock_signature,
    };

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_header() -> Vec<u8> {
        let mut image = vec![0u8; SECTOR_SIZE * 8];
        let sb_offset = SECTOR_SIZE * 2;
        image[sb_offset..sb_offset + 4].copy_from_slice(VDFS4_SB_SIGNATURE);
        image[sb_offset + 4..sb_offset + 8].copy_from_slice(VDFS4_LAYOUT_VERSION_2007);
        let block_order = std::mem::offset_of!(Vdfs4SuperBlock, log_block_size);
        let page_order = std::mem::offset_of!(Vdfs4SuperBlock, log_super_page_size);
        image[sb_offset + block_order] = 12;
        image[sb_offset + page_order] = 14;
        let exsb = sb_offset + SECTOR_SIZE;
        let layout = exsb + SECTOR_SIZE;
        let _ = (layout, exsb);
        image
    }

    #[test]
    fn rejects_impossible_block_order_before_shift() {
        let mut image = build_header();
        let sb_offset = SECTOR_SIZE * 2;
        let block_order = std::mem::offset_of!(Vdfs4SuperBlock, log_block_size);
        image[sb_offset + block_order] = u8::MAX;

        let error = read_superblock_with_keys(&image, &[])
            .err()
            .expect("expected read error");
        assert!(error.to_string().contains("block size order"));
    }

    #[test]
    fn rejects_impossible_super_page_order() {
        let mut image = build_header();
        let sb_offset = SECTOR_SIZE * 2;
        let page_order = std::mem::offset_of!(Vdfs4SuperBlock, log_super_page_size);
        image[sb_offset + page_order] = u8::MAX;

        let error = read_superblock_with_keys(&image, &[])
            .err()
            .expect("expected read error");
        assert!(error.to_string().contains("super page size order"));
    }

    fn rsa_sign_superblock(
        image: &mut [u8],
        private_key: &rsa::RsaPrivateKey,
        sign_type: u8,
        hash_type: u8,
    ) {
        use md5::Md5;
        use rsa::traits::{PrivateKeyParts, PublicKeyParts};
        use sha1::Sha1;
        use sha2::{Digest, Sha256};

        let sb_offset = SECTOR_SIZE * 2;
        let sign_type_offset = sb_offset + std::mem::offset_of!(Vdfs4SuperBlock, sign_type);
        let hash_type_offset = sb_offset + std::mem::offset_of!(Vdfs4SuperBlock, hash_type);
        let sb_hash_offset = sb_offset + std::mem::offset_of!(Vdfs4SuperBlock, sb_hash);
        image[sign_type_offset] = sign_type;
        image[hash_type_offset] = hash_type;

        let signature_len = match sign_type {
            1 => 128,
            2 => 256,
            _ => panic!("unsupported test signature type"),
        };
        let signed_len = SECTOR_SIZE - 4 - signature_len;
        let signed = &image[sb_offset..sb_offset + signed_len];
        let digest = match hash_type {
            1 => Sha1::digest(signed).to_vec(),
            2 => Sha256::digest(signed).to_vec(),
            3 => Md5::digest(signed).to_vec(),
            _ => panic!("unsupported test hash type"),
        };

        let digest_start = signature_len - digest.len();
        let mut encoded = vec![0u8; signature_len];
        encoded[0] = 0;
        encoded[1] = 1;
        encoded[2..digest_start - 1].fill(0xff);
        encoded[digest_start - 1] = 0;
        encoded[digest_start..].copy_from_slice(&digest);
        let em = rsa::BigUint::from_bytes_be(&encoded);
        let signature = em.modpow(private_key.d(), private_key.n());
        let mut signature = signature.to_bytes_be();
        assert!(signature.len() <= signature_len);
        if signature.len() < signature_len {
            let mut padded = vec![0u8; signature_len - signature.len()];
            padded.extend_from_slice(&signature);
            signature = padded;
        }

        let signature_offset = if sign_type == 1 {
            sb_hash_offset + 128
        } else {
            sb_hash_offset
        };
        image[signature_offset..signature_offset + signature_len].copy_from_slice(&signature);
    }

    #[test]
    fn verifies_signed_superblock_against_trusted_key() {
        let mut rng = rand::thread_rng();
        for (bits, sign_type, hash_type) in [(1024, 1, 1), (2048, 2, 2), (2048, 2, 3)] {
            let private_key = rsa::RsaPrivateKey::new(&mut rng, bits).unwrap();
            let public_key = rsa::RsaPublicKey::from(&private_key);
            let mut image = build_header();
            rsa_sign_superblock(&mut image, &private_key, sign_type, hash_type);

            assert!(read_superblock_with_keys(&image, std::slice::from_ref(&public_key)).is_ok());
        }
    }

    #[test]
    fn reports_signed_superblock_unverified_without_trust() {
        let mut rng = rand::thread_rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let mut image = build_header();
        rsa_sign_superblock(&mut image, &private_key, 2, 2);

        let info = read_superblock_with_keys(&image, &[]).unwrap();
        assert_eq!(info.superblock_signature.unwrap().verified, None);
    }

    #[test]
    fn rejects_signed_superblock_from_wrong_key() {
        let mut rng = rand::thread_rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let wrong_key = rsa::RsaPublicKey::from(&rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap());
        let mut image = build_header();
        rsa_sign_superblock(&mut image, &private_key, 2, 2);

        let error = read_superblock_with_keys(&image, std::slice::from_ref(&wrong_key))
            .err()
            .expect("wrong trust key should fail");
        assert!(error.to_string().contains("did not verify"));
    }
}
