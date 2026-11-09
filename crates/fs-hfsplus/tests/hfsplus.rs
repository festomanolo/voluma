//! HFS+ driver judged against macOS itself: the image is created and populated by macOS, then
//! the driver's whole tree (names, kinds, sizes, bytes) is compared with a real read-only mount.
use block_device::{BlockDevice, ImageDevice};
use fs_core::{EntryKind, Filesystem, FsError};
use fs_hfsplus::HfsPlusFs;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn make(fs: &str, mb: &str) -> Option<(tempfile::TempDir, PathBuf)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: needs macOS"); return None; }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("t.img");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/make_mac_fixture.sh");
    let out = Command::new("sh").args([script, img.to_str().unwrap(), fs, mb]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some((dir, img))
}
fn open(img: &Path) -> HfsPlusFs {
    let d: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(img).unwrap());
    HfsPlusFs::open(d).unwrap()
}
fn read_all(fs: &HfsPlusFs, e: &fs_core::DirEntry) -> Vec<u8> {
    let (mut out, mut buf) = (Vec::new(), vec![0u8; 9973]);
    loop { let n = fs.read_at(e, out.len() as u64, &mut buf).unwrap(); if n == 0 { break; } out.extend_from_slice(&buf[..n]); }
    out
}
struct Mounted(PathBuf, #[allow(dead_code)] tempfile::TempDir);
impl Drop for Mounted {
    fn drop(&mut self) { let _ = Command::new("hdiutil").args(["detach", self.0.to_str().unwrap(), "-force"]).output(); }
}
fn mount_ro(img: &Path) -> Mounted {
    let mnt = tempfile::tempdir().unwrap();
    let a = Command::new("hdiutil").args(["attach", "-imagekey", "diskimage-class=CRawDiskImage", "-readonly", "-nobrowse", "-mountpoint"])
        .arg(mnt.path()).arg(img).output().unwrap();
    assert!(a.status.success(), "mount: {}", String::from_utf8_lossy(&a.stderr));
    Mounted(mnt.path().to_path_buf(), mnt)
}

fn compare(fs: &HfsPlusFs, vpath: &str, host: &Path, count: &mut usize) {
    let mut ours: Vec<_> = fs.read_dir(vpath).unwrap();
    ours.sort_by(|a, b| a.name.cmp(&b.name));
    let mut theirs: Vec<String> = std::fs::read_dir(host).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    theirs.sort();
    let visible = |n: &String| !n.starts_with('.');
    let a: Vec<_> = ours.iter().map(|e| e.name.clone()).filter(visible).collect();
    let b: Vec<_> = theirs.into_iter().filter(visible).collect();
    assert_eq!(a, b, "listing of {vpath}");
    for e in ours.iter().filter(|e| visible(&e.name)) {
        let hp = host.join(&e.name);
        let child = format!("{}/{}", vpath.trim_end_matches('/'), e.name);
        let md = std::fs::symlink_metadata(&hp).unwrap();
        *count += 1;
        if md.is_dir() {
            assert_eq!(e.kind, EntryKind::Directory, "{child}");
            compare(fs, &child, &hp, count);
        } else {
            assert_eq!(e.kind, EntryKind::File, "{child}");
            let want = if md.file_type().is_symlink() { std::fs::read_link(&hp).unwrap().to_string_lossy().into_owned().into_bytes() } else { std::fs::read(&hp).unwrap() };
            assert_eq!(e.size, want.len() as u64, "size of {child}");
            assert_eq!(read_all(fs, e), want, "bytes of {child}");
            if *count % 50 == 0 { assert_eq!(fs.stat(&child).unwrap().size, e.size); }
        }
    }
}

#[test]
fn hfsplus_whole_tree_matches_macos() {
    let Some((_d, img)) = make("HFS+", "64") else { return };
    let fs = open(&img);
    assert_eq!(fs.name(), "HFS+");
    assert_eq!(fs.label().as_deref(), Some("FIXTURE"));
    assert!(!fs.capabilities().write);
    let m = mount_ro(&img);
    let mut n = 0;
    compare(&fs, "/", &m.0, &mut n);
    assert!(n >= 616, "walked only {n} entries");
}

#[test]
fn hfsplus_specifics() {
    let Some((_d, img)) = make("HFS+", "64") else { return };
    let fs = open(&img);
    assert_eq!(fs.read_dir("/many").unwrap().len(), 600);
    let e = fs.stat("/README.TXT").unwrap();
    assert_eq!(read_all(&fs, &e), b"hello voluma\n");
    // case-insensitive lookup
    assert_eq!(fs.stat("/readme.txt").unwrap().size, 13);
    // hard link resolves to the same bytes
    let h = fs.stat("/docs/hardlink-to-readme.txt").unwrap();
    assert_eq!(read_all(&fs, &h), b"hello voluma\n");
    // fragmented files read back correctly
    let f = fs.stat("/frag1.bin").unwrap();
    assert_eq!(read_all(&fs, &f), vec![b'1'; 60 * 40000]);
    let big = fs.stat("/big.bin").unwrap();
    let mut h = Sha256::new();
    for i in 0..(3 * 1024 * 1024 / 16) { h.update(format!("{i:015}\n").as_bytes()); }
    assert_eq!(Sha256::digest(read_all(&fs, &big)), h.finalize());
    assert!(fs.stat("/empty.dat").unwrap().size == 0);
    assert!(matches!(fs.stat("/nope"), Err(FsError::NotFound(_))));
    assert!(matches!(fs.read_dir("/README.TXT"), Err(FsError::NotADirectory(_))));
    assert!(fs.read_dir("/").unwrap().iter().all(|e| !e.name.starts_with('\0')), "private dir hidden");
    assert!(fs.write_file_is_unsupported());
}

trait Probe { fn write_file_is_unsupported(&self) -> bool; }
impl Probe for HfsPlusFs {
    fn write_file_is_unsupported(&self) -> bool {
        matches!(self.create_dir("/x"), Err(FsError::Unsupported(_)))
            && matches!(self.remove("/x"), Err(FsError::Unsupported(_)))
    }
}

#[test]
fn hfsx_case_sensitive() {
    let Some((_d, img)) = make("Case-sensitive HFS+", "64") else { return };
    let fs = open(&img);
    assert_eq!(fs.name(), "HFSX");
    assert!(fs.stat("/README.TXT").is_ok());
    assert!(matches!(fs.stat("/readme.txt"), Err(FsError::NotFound(_))));
}

#[test]
fn corruption_never_panics() {
    let Some((_d, img)) = make("HFS+", "16") else { return };
    let clean = std::fs::read(&img).unwrap();
    let mut seed = 0x1234_5678u32;
    for round in 0..40 {
        let mut b = clean.clone();
        for _ in 0..30 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            // bias toward the volume header and early (catalog) area
            let off = if round % 2 == 0 { 1024 + (seed as usize % 512) } else { (seed as usize) % b.len().min(2 << 20) };
            b[off] = (seed >> 24) as u8;
        }
        let p = img.with_extension(format!("c{round}"));
        std::fs::write(&p, &b).unwrap();
        let d: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(&p).unwrap());
        if let Ok(fs) = HfsPlusFs::open(d) {
            if let Ok(list) = fs.read_dir("/") {
                for e in list.iter().take(20) { let mut buf = [0u8; 4096]; let _ = fs.read_at(e, 0, &mut buf); }
            }
            let _ = fs.read_dir("/many");
        }
        std::fs::remove_file(&p).unwrap();
    }
}
