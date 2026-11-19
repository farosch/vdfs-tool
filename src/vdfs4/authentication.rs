use anyhow::{Context, Result, bail};
use md5::Md5;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::{RsaPublicKey, traits::PublicKeyParts};
use sha1::Sha1;
use sha2::{Digest, Sha256};

pub(crate) fn read_public_key(path: &std::path::Path) -> Result<RsaPublicKey> {
    let pem = std::fs::read(path)
        .with_context(|| format!("Failed to read RSA public key: {}", path.display()))?;
    let pem = std::str::from_utf8(&pem).context("RSA public key must be PEM text")?;
    RsaPublicKey::from_public_key_pem(pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(pem))
        .with_context(|| format!("Failed to parse RSA public key: {}", path.display()))
}

pub(crate) fn verify_signature(
    public_key: &RsaPublicKey,
    message: &[u8],
    signature: &[u8],
    prefix: u8,
) -> Result<()> {
    let digest: Vec<u8> = match prefix {
        b'I' => Md5::digest(message).to_vec(),
        b'H' => Sha1::digest(message).to_vec(),
        b'h' => Sha256::digest(message).to_vec(),
        _ => bail!("Unsupported hashed descriptor authentication prefix"),
    };

    let modulus = public_key.n();
    let exponent = public_key.e();
    let signature_int = rsa::BigUint::from_bytes_be(signature);
    if &signature_int >= modulus {
        bail!("RSA signature is out of range");
    }

    let em = signature_int.modpow(exponent, modulus).to_bytes_be();
    let mut encoded = vec![0u8; signature.len()];
    if em.len() <= encoded.len() {
        let start = encoded.len() - em.len();
        encoded[start..].copy_from_slice(&em);
    } else {
        bail!("RSA encoded message overflow");
    }

    if encoded.len() < digest.len() + 11 {
        bail!("RSA encoded message is too short");
    }
    if encoded[0] != 0 || encoded[1] != 1 {
        bail!("RSA signature has invalid PKCS#1 v1.5 prefix");
    }
    let digest_start = encoded.len() - digest.len();
    let separator = digest_start
        .checked_sub(1)
        .context("RSA signature overflow")?;
    if separator < 2 {
        bail!("RSA signature has invalid padding");
    }
    if encoded[2..separator].iter().any(|byte| *byte != 0xff) {
        bail!("RSA signature has invalid PKCS#1 v1.5 padding");
    }
    if encoded[separator] != 0 {
        bail!("RSA signature is missing PKCS#1 v1.5 separator");
    }
    if encoded[digest_start..] != digest {
        bail!("RSA signature digest mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::traits::PrivateKeyParts;

    fn sign_unprefixed(private_key: &rsa::RsaPrivateKey, digest: &[u8], width: usize) -> Vec<u8> {
        assert!(width >= digest.len() + 11);
        let mut em = vec![0u8; width];
        em[0] = 0;
        em[1] = 1;
        let digest_start = width - digest.len();
        let separator = digest_start - 1;
        em[2..separator].fill(0xff);
        em[separator] = 0;
        em[digest_start..].copy_from_slice(digest);
        let em_int = rsa::BigUint::from_bytes_be(&em);
        let signature = em_int.modpow(private_key.d(), private_key.n());
        let mut result = signature.to_bytes_be();
        if result.len() < width {
            let mut padded = vec![0u8; width - result.len()];
            padded.extend_from_slice(&result);
            result = padded;
        }
        result
    }

    #[test]
    fn verifies_all_supported_unprefixed_pkcs1v1_5_digests() {
        let mut rng = rand::thread_rng();
        for (bits, width) in [(1024, 128), (2048, 256)] {
            let private_key = rsa::RsaPrivateKey::new(&mut rng, bits).unwrap();
            let public_key = rsa::RsaPublicKey::from(&private_key);
            let message = b"vdfs hash table";
            for prefix in *b"IHh" {
                let digest: Vec<u8> = match prefix {
                    b'I' => Md5::digest(message).to_vec(),
                    b'H' => Sha1::digest(message).to_vec(),
                    b'h' => Sha256::digest(message).to_vec(),
                    _ => unreachable!(),
                };
                let signature = sign_unprefixed(&private_key, &digest, width);
                assert!(verify_signature(&public_key, message, &signature, prefix).is_ok());
            }
        }
    }

    #[test]
    fn rejects_wrong_key_and_mutated_signed_message() {
        let mut rng = rand::thread_rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let wrong_key = rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let public_key = rsa::RsaPublicKey::from(&private_key);
        let wrong_public_key = rsa::RsaPublicKey::from(&wrong_key);
        let message = b"vdfs hash table";
        let digest = Sha256::digest(message).to_vec();
        let signature = sign_unprefixed(&private_key, &digest, 128);

        assert!(verify_signature(&wrong_public_key, message, &signature, b'h').is_err());
        assert!(verify_signature(&public_key, b"mutated", &signature, b'h').is_err());
    }

    #[test]
    fn rejects_non_pkcs1v1_5_prefix() {
        let public_key = rsa::RsaPublicKey::new(
            rsa::BigUint::from_bytes_be(&[0x01; 128]),
            rsa::BigUint::from(65537u32),
        )
        .unwrap();
        let result = verify_signature(&public_key, b"message", &[0u8; 128], b'h');
        assert!(result.is_err());
    }
}
