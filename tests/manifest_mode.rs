use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/small-zlib.vdfs"
);

fn run(name: &str, input: &Path, extra: &[&Path]) -> PathBuf {
    let out = std::env::temp_dir().join(format!(
        "vdfs-unpack-manifest-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&out);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vdfs-unpack"));
    cmd.arg(input).arg("-o").arg(&out);
    for path in extra {
        cmd.arg("--manifest-dir").arg(path);
    }
    let output = cmd.output().expect("failed to run vdfs-unpack");
    assert!(
        output.status.success(),
        "vdfs-unpack exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    out
}

fn tree(base: &Path, dir: &Path, acc: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let rel = path
            .strip_prefix(base)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let kind = fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_symlink() {
            acc.push(format!("{rel} -> link"));
        } else if kind.is_dir() {
            acc.push(format!("{rel}/"));
            tree(base, &path, acc);
        } else {
            acc.push(rel);
        }
    }
}

fn listing(root: &Path) -> Vec<String> {
    let mut acc = Vec::new();
    tree(root, root, &mut acc);
    acc.sort();
    acc
}

#[test]
fn manifest_mode_extracts_the_same_tree_as_normal_mode() {
    let normal = run("normal", Path::new(FIXTURE), &[]);
    let manifest_dir =
        std::env::temp_dir().join(format!("vdfs-unpack-mdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&manifest_dir);
    let manifest = run("manifest", Path::new(FIXTURE), &[&manifest_dir]);

    assert_eq!(listing(&normal), listing(&manifest));
    assert!(listing(&manifest).contains(&"root/link.txt -> link".to_string()));

    let link = manifest.join("root/link.txt");
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("hello.txt"));

    let entries = fs::read_to_string(manifest_dir.join("entries.tsv")).unwrap();
    assert!(
        entries.contains("LINK\troot/link.txt\t68656c6c6f2e747874"),
        "manifest is missing the symlink row: {entries}"
    );

    fs::remove_dir_all(&normal).unwrap();
    fs::remove_dir_all(&manifest).unwrap();
    fs::remove_dir_all(&manifest_dir).unwrap();
}

fn special_node_image(mode: u16, payload: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut image = fs::read(FIXTURE).unwrap();
    // Catalog key: type at +24, name length at +25, name at +26.
    let matches: Vec<usize> = image
        .windows(35)
        .enumerate()
        .filter(|(_, bytes)| bytes[24] == 2 && bytes[25] == 9 && &bytes[26..] == b"hello.txt")
        .map(|(offset, _)| offset)
        .collect();
    assert_eq!(matches.len(), 1, "expected one hello.txt file record");
    let start = matches[0];
    let key_len = u16::from_le_bytes(image[start + 4..start + 6].try_into().unwrap()) as usize;
    let record_len = u16::from_le_bytes(image[start + 6..start + 8].try_into().unwrap()) as usize;
    let value = start + key_len;
    image[value..value + 4].fill(0);
    image[value + 32..value + 34].copy_from_slice(&(mode | 0o600).to_le_bytes());
    // Replace the fork (value +80) with an empty or nonempty inline body.
    let fork = value + 80;
    image[fork..start + record_len].fill(0);
    image[fork..fork + 8].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    image[fork + 16..fork + 16 + payload.len()].copy_from_slice(payload);
    let record = image[start..start + record_len].to_vec();
    (image, record)
}

#[test]
fn special_nodes_are_regular_files_in_both_modes() {
    let work =
        std::env::temp_dir().join(format!("vdfs-unpack-special-nodes-{}", std::process::id()));
    fs::create_dir(&work).unwrap();
    let payloads: &[&[u8]] = &[b"", b"\x00node\xff"];
    for (kind, mode) in [
        ("fifo", 0x1000),
        ("character", 0x2000),
        ("block", 0x6000),
        ("socket", 0xc000),
    ] {
        for &payload in payloads {
            let name = format!("{kind}-{}", payload.len());
            let (image, record) = special_node_image(mode, payload);
            let input = work.join(format!("{name}.vdfs"));
            fs::write(&input, image).unwrap();
            let manifest_dir = work.join(format!("{name}-metadata"));
            let normal = run(&format!("{name}-normal"), &input, &[]);
            let manifest = run(&format!("{name}-manifest"), &input, &[&manifest_dir]);

            for output in [&normal, &manifest] {
                let node = output.join("root/hello.txt");
                assert!(
                    fs::symlink_metadata(&node).unwrap().file_type().is_file(),
                    "{name}: must create a regular file, never a host special node"
                );
                assert_eq!(fs::read(&node).unwrap(), payload, "{name}: fork bytes");
            }
            assert_eq!(listing(&normal), listing(&manifest), "{name}: output tree");

            let entries = fs::read_to_string(manifest_dir.join("entries.tsv")).unwrap();
            let entry = entries
                .lines()
                .find(|line| line.starts_with("ENTRY\troot/hello.txt\t"))
                .expect("special node must retain its manifest entry");
            let fields: Vec<&str> = entry.split('\t').collect();
            let offset: usize = fields[2].parse().unwrap();
            let length: usize = fields[3].parse().unwrap();
            let catalog = fs::read(manifest_dir.join("catalog.bin")).unwrap();
            assert_eq!(
                &catalog[offset..offset + length],
                record,
                "{name}: metadata"
            );

            fs::remove_dir_all(&normal).unwrap();
            fs::remove_dir_all(&manifest).unwrap();
        }
    }
    fs::remove_dir_all(&work).unwrap();
}
