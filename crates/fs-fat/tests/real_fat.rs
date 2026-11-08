//! Tests against images generated and populated by macOS's own FAT implementation
//! (tests/fixtures/make_fat_fixture.sh). On macOS a generator failure PANICS;
//! elsewhere the tests are skipped with a loud message.

use block_device::{BlockDevice, ImageDevice};
use fs_core::{EntryKind, Filesystem, FsError};
use fs_fat::{FatFs, FatKind};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn make(fs: &str, mb: &str, tag: &str) -> Option<(tempfile::TempDir, PathBuf)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: fixtures need macOS hdiutil"); return None; }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join(format!("{tag}.img"));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/make_fat_fixture.sh");
    let out = Command::new("sh").args([script, img.to_str().unwrap(), fs, mb]).output().unwrap();
    assert!(out.status.success(), "fixture generation failed: {}", String::from_utf8_lossy(&out.stderr));
    Some((dir, img))
}

fn open(img: &PathBuf) -> FatFs {
    let dev: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(img).unwrap());
    FatFs::open(dev).unwrap()
}

fn read_all(fs: &FatFs, path: &str) -> Vec<u8> {
    let e = fs.stat(path).unwrap();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 7001]; // deliberately odd size, crosses cluster boundaries
    loop {
        let n = fs.read_at(&e, out.len() as u64, &mut buf).unwrap();
        if n == 0 { break; }
        out.extend_from_slice(&buf[..n]);
    }
    assert_eq!(out.len() as u64, e.size);
    out
}

fn names(fs: &FatFs, p: &str) -> Vec<String> {
    let mut v: Vec<_> = fs.read_dir(p).unwrap().into_iter().map(|e| e.name).collect();
    v.sort(); v
}

#[test]
fn fat32_browse_and_read() {
    let Some((_d, img)) = make("MS-DOS FAT32", "64", "f32") else { return };
    let fs = open(&img);
    assert_eq!(fs.kind(), FatKind::Fat32);
    assert_eq!(fs.label().as_deref(), Some("FIXTURE"));
    let root = names(&fs, "/");
    for n in ["README.TXT", "A Rather Long File Name With Spaces.txt", "héllo-日本語.txt", "lowercase.txt", "docs", "empty-dir", "big.bin"] {
        assert!(root.iter().any(|r| r.eq_ignore_ascii_case(n)), "missing {n} in {root:?}");
    }
    assert_eq!(read_all(&fs, "/README.TXT"), b"hello voluma\n");
    assert_eq!(read_all(&fs, "/readme.txt"), b"hello voluma\n"); // case-insensitive
    assert_eq!(read_all(&fs, "/héllo-日本語.txt"), b"unicode\n");
    assert_eq!(read_all(&fs, "/docs/nested/deep/file.bin"), b"nested\n");
    assert!(fs.read_dir("/empty-dir").unwrap().is_empty());
    assert_eq!(fs.read_dir("/docs").unwrap().iter().filter(|e| e.name.starts_with('f')).count(), 40);
    assert_eq!(fs.stat("/docs").unwrap().kind, EntryKind::Directory);
}

#[test]
fn fat32_large_file_content_is_exact() {
    let Some((_d, img)) = make("MS-DOS FAT32", "64", "f32b") else { return };
    let fs = open(&img);
    let data = read_all(&fs, "/big.bin");
    assert_eq!(data.len(), 3 * 1024 * 1024);
    // Expected stream is regenerated independently here: 16-byte records "%015d\n".
    let mut h = Sha256::new();
    for i in 0..(3 * 1024 * 1024 / 16) { h.update(format!("{i:015}\n").as_bytes()); }
    assert_eq!(Sha256::digest(&data), h.finalize());
}

#[test]
fn fat16_and_fat12() {
    let Some((_d, img)) = make("MS-DOS FAT16", "16", "f16") else { return };
    let fs = open(&img);
    assert_eq!(fs.kind(), FatKind::Fat16);
    assert_eq!(read_all(&fs, "/README.TXT"), b"hello voluma\n");
    assert_eq!(read_all(&fs, "/docs/nested/deep/file.bin"), b"nested\n");

    let Some((_d2, img2)) = make("MS-DOS FAT12", "2", "f12") else { return };
    let fs = open(&img2);
    assert_eq!(fs.kind(), FatKind::Fat12);
    assert_eq!(read_all(&fs, "/README.TXT"), b"hello voluma\n");
    assert_eq!(names(&fs, "/docs").len(), 41); // 40 files + nested
}

#[test]
fn errors_are_typed_and_writes_unsupported() {
    let Some((_d, img)) = make("MS-DOS FAT16", "16", "err") else { return };
    let fs = open(&img);
    assert!(matches!(fs.stat("/nope"), Err(FsError::NotFound(_))));
    assert!(matches!(fs.read_dir("/README.TXT"), Err(FsError::NotADirectory(_))));
    assert!(matches!(fs.read_dir("/../.."), Err(FsError::NotFound(_))));
    let dir = fs.stat("/docs").unwrap();
    assert!(matches!(fs.read_at(&dir, 0, &mut [0; 4]), Err(FsError::IsADirectory(_))));
    assert!(!fs.capabilities().write);
}

#[test]
fn corrupt_images_never_panic() {
    let Some((_d, img)) = make("MS-DOS FAT16", "16", "cor") else { return };
    let clean = std::fs::read(&img).unwrap();
    // Deterministic xorshift corruption of the first 64 KiB (BPB, FAT, root dir).
    let mut x = 0x2545F4914F6CDD1Du64;
    for round in 0..200 {
        let mut b = clean.clone();
        for _ in 0..8 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let at = (x % 65536) as usize;
            b[at] = (x >> 32) as u8;
        }
        let p = img.with_file_name(format!("c{round}.img"));
        std::fs::write(&p, &b).unwrap();
        let dev: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(&p).unwrap());
        if let Ok(fs) = FatFs::open(dev) {
            if let Ok(es) = fs.read_dir("/") {
                for e in es.iter().take(20) {
                    let _ = fs.read_dir(&format!("/{}", e.name));
                    let _ = fs.read_at(e, 0, &mut [0; 4096]);
                }
            }
        }
        std::fs::remove_file(p).ok();
    }
}
