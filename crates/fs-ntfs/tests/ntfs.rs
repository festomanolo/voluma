//! Read-path tests (see common/mod.rs for how images are made and judged).
mod common;
use common::*;
use fs_core::{Filesystem, FsError};
#[test]
fn ntfs_whole_tree_matches_source() {
    let Some(img) = make(96, 4096) else { return };
    let fs = open(&img.path).unwrap();
    assert_eq!(fs.name(), "NTFS");
    assert_eq!(fs.label().as_deref(), Some("FIXTURE"));
    assert!(!fs.capabilities().write, "device is read-only");
    let mut n = 0;
    compare(&fs, "/", &img.src, &mut n);
    assert!(n > 3000, "only walked {n}");
    // case-insensitive lookup via $UpCase, incl. non-ASCII
    assert_eq!(fs.stat("/readme.txt").unwrap().size, 13);
    assert_eq!(fs.stat("/HÉLLO-日本語.TXT").unwrap().size, 8);
    assert!(matches!(fs.stat("/nope"), Err(FsError::NotFound(_))));
    assert!(matches!(fs.read_dir("/README.TXT"), Err(FsError::NotADirectory(_))));
    // $-metafiles are not listed in the root
    assert!(fs.read_dir("/").unwrap().iter().all(|e| !e.name.starts_with('$')));
}

#[test]
fn ntfs_small_clusters_and_resident_boundary() {
    let Some(img) = make(64, 512) else { return };
    let fs = open(&img.path).unwrap();
    let mut n = 0;
    compare(&fs, "/", &img.src, &mut n);
}

#[test]
fn rejects_non_ntfs() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("z.img");
    std::fs::write(&p, vec![0u8; 1 << 20]).unwrap();
    assert!(matches!(open(&p), Err(FsError::Corrupt(_))));
}

#[test]
fn corruption_never_panics() {
    let Some(img) = make(48, 4096) else { return };
    let clean = std::fs::read(&img.path).unwrap();
    let mut seed = 0xC0DE_1234u32;
    for round in 0..80 {
        let mut b = clean.clone();
        for _ in 0..40 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            // metadata lives at the start of the volume: bias toward it
            let off = match round % 3 { 0 => seed as usize % 512, 1 => (seed as usize) % (4 << 20), _ => (seed as usize) % b.len() };
            b[off] = (seed >> 24) as u8;
        }
        let p = img.path.with_extension(format!("c{round}"));
        std::fs::write(&p, &b).unwrap();
        if let Ok(fs) = open(&p) {
            for dir in ["/", "/many", "/docs/nested"] {
                if let Ok(l) = fs.read_dir(dir) { for e in l.iter().take(30) { let mut buf = [0u8; 8192]; let _ = fs.read_at(e, 0, &mut buf); } }
            }
        }
        std::fs::remove_file(&p).unwrap();
    }
}
