use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::{EnrollTrustArgs, TrustArgs, TrustCommand};
use crate::vdfs4::authentication;

pub(crate) fn default_store() -> Result<PathBuf> {
    let base = if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        bail!("Cannot determine local trust store: set XDG_CONFIG_HOME or HOME");
    };
    Ok(base.join("vdfs-tool").join("trust"))
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 128
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

pub(crate) fn run(args: TrustArgs) -> Result<()> {
    match args.command {
        TrustCommand::Enroll(args) => enroll(args),
        TrustCommand::List(args) => list(args.store),
    }
}

fn enroll(args: EnrollTrustArgs) -> Result<()> {
    if !valid_label(&args.label) {
        bail!("Invalid trust label; use only ASCII letters, digits, '.', '-' and '_'");
    }
    let key = authentication::read_public_key(&args.public_key)?;
    let _ = key;
    let store = args.store.map(Ok).unwrap_or_else(default_store)?;
    fs::create_dir_all(&store)
        .with_context(|| format!("Failed to create trust store: {}", store.display()))?;
    let destination = store.join(format!("{}.pem", args.label));
    if destination.exists() && !args.replace {
        bail!(
            "Trust anchor already exists: {} (use --replace)",
            destination.display()
        );
    }
    let pem = fs::read(&args.public_key)
        .with_context(|| format!("Failed to read public key: {}", args.public_key.display()))?;
    fs::write(&destination, pem)
        .with_context(|| format!("Failed to write trust anchor: {}", destination.display()))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "label": args.label,
            "path": destination.display().to_string(),
        }))?
    );
    Ok(())
}

fn list(store: Option<PathBuf>) -> Result<()> {
    let store = store.map(Ok).unwrap_or_else(default_store)?;
    let mut anchors = Vec::new();
    if store.exists() {
        for entry in fs::read_dir(&store)
            .with_context(|| format!("Failed to read trust store: {}", store.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "pem") {
                match authentication::read_public_key(&path) {
                    Ok(_) => anchors.push(serde_json::json!({
                        "label": path.file_stem().and_then(|name| name.to_str()).unwrap_or(""),
                        "path": path.display().to_string(),
                        "valid": true,
                    })),
                    Err(error) => anchors.push(serde_json::json!({
                        "label": path.file_stem().and_then(|name| name.to_str()).unwrap_or(""),
                        "path": path.display().to_string(),
                        "valid": false,
                        "error": format!("{error:#}"),
                    })),
                }
            }
        }
    }
    anchors.sort_by_key(|anchor| anchor["label"].as_str().unwrap_or("").to_owned());
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "store": store.display().to_string(),
            "anchors": anchors,
        }))?
    );
    Ok(())
}

pub(crate) fn load_store(store: &Path) -> Result<Vec<rsa::RsaPublicKey>> {
    if !store.exists() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in fs::read_dir(store)
        .with_context(|| format!("Failed to read trust store: {}", store.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "pem") {
            keys.push(
                authentication::read_public_key(&path)
                    .with_context(|| format!("Invalid trust anchor: {}", path.display()))?,
            );
        }
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::EncodePublicKey;

    fn temp_store(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vdfs-tool-trust-{name}-{}", std::process::id()))
    }

    #[test]
    fn enrolls_lists_and_loads_pem_public_key() {
        let store = temp_store("enroll");
        let _ = fs::remove_dir_all(&store);
        let mut rng = rand::thread_rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let public_key = rsa::RsaPublicKey::from(&private_key);
        let source = store.with_extension("pub.pem");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(
            &source,
            public_key
                .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
                .unwrap(),
        )
        .unwrap();

        enroll(EnrollTrustArgs {
            label: "vendor.test".to_owned(),
            public_key: source.clone(),
            store: Some(store.clone()),
            replace: false,
        })
        .unwrap();

        assert_eq!(load_store(&store).unwrap().len(), 1);
        fs::remove_dir_all(&store).unwrap();
        let _ = fs::remove_file(&source);
    }

    #[test]
    fn rejects_invalid_label_and_duplicate_enrollment() {
        let store = temp_store("labels");
        let _ = fs::remove_dir_all(&store);
        let mut rng = rand::thread_rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let public_key = rsa::RsaPublicKey::from(&private_key);
        let source = temp_store("source").with_extension("pem");
        fs::write(
            &source,
            public_key
                .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
                .unwrap(),
        )
        .unwrap();

        assert!(
            enroll(EnrollTrustArgs {
                label: "../escape".to_owned(),
                public_key: source.clone(),
                store: Some(store.clone()),
                replace: false,
            })
            .is_err()
        );
        enroll(EnrollTrustArgs {
            label: "vendor".to_owned(),
            public_key: source.clone(),
            store: Some(store.clone()),
            replace: false,
        })
        .unwrap();
        assert!(
            enroll(EnrollTrustArgs {
                label: "vendor".to_owned(),
                public_key: source.clone(),
                store: Some(store.clone()),
                replace: false,
            })
            .is_err()
        );

        fs::remove_dir_all(&store).unwrap();
        let _ = fs::remove_file(&source);
    }
}
