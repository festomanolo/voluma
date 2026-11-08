//! exFAT tests against OS-generated volumes; writes are judged by macOS (`fsck_exfat -n`
//! and a real read-only mount). On macOS a generator failure panics rather than skipping.

use block_device::{BlockDevice, ImageDevice};
use fs_core::{EntryKind, Filesystem, FsError};
use fs_exfat::ExFatFs;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn make(mb: &str, tag: &str) -> Option<(tempfile::TempDir, PathBuf)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: needs macOS"); return None; }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join(format!("{tag}.img"));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/make_fat_fixture.sh");
    let out = Command::new("sh").args([script, img.to_str().unwrap(), "ExFAT", mb]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some((dir, img))
}
fn open(img: &Path, rw: bool) -> ExFatFs {
    let d: Arc<dyn BlockDevice> = Arc::new(if rw { ImageDevice::open_read_write(img) } else { ImageDevice::open_read_only(img) }.unwrap());
    ExFatFs::open(d).unwrap()
}
fn pattern(n: usize, seed: u8) -> Vec<u8> { (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }
fn read_all(fs: &ExFatFs, path: &str) -> Vec<u8> {
    let e = fs.stat(path).unwrap();
    let (mut out, mut buf) = (Vec::new(), vec![0u8; 7001]);
    loop { let n = fs.read_at(&e, out.len() as u64, &mut buf).unwrap(); if n == 0 { break; } out.extend_from_slice(&buf[..n]); }
    assert_eq!(out.len() as u64, e.size);
    out
}

fn os_verify(img: &Path, check: impl FnOnce(&Path)) {
    let out = Command::new("hdiutil").args(["attach", "-imagekey", "diskimage-class=CRawDiskImage", "-readonly", "-nomount"]).arg(img).output().unwrap();
    assert!(out.status.success(), "attach: {}", String::from_utf8_lossy(&out.stderr));
    let dev = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string();
    let f = Command::new("fsck_exfat").args(["-n", &dev.replace("/dev/disk", "/dev/rdisk")]).output().unwrap();
    let txt = format!("{}{}", String::from_utf8_lossy(&f.stdout), String::from_utf8_lossy(&f.stderr));
    let _ = Command::new("hdiutil").args(["detach", &dev]).output();
    assert!(f.status.success(), "fsck_exfat reported problems:\n{txt}");
    let mnt = tempfile::tempdir().unwrap();
    let a = Command::new("hdiutil").args(["attach", "-imagekey", "diskimage-class=CRawDiskImage", "-readonly", "-nobrowse", "-mountpoint"]).arg(mnt.path()).arg(img).output().unwrap();
    assert!(a.status.success(), "mount: {}", String::from_utf8_lossy(&a.stderr));
    check(mnt.path());
    let _ = Command::new("hdiutil").args(["detach", mnt.path().to_str().unwrap(), "-force"]).output();
}

#[test]
fn exfat_browse_and_read() {
    let Some((_d, img)) = make("64", "r") else { return };
    let fs = open(&img, false);
    assert_eq!(fs.label().as_deref(), Some("FIXTURE"));
    assert!(!fs.capabilities().write);
    let names: Vec<_> = fs.read_dir("/").unwrap().into_iter().map(|e| e.name).collect();
    for n in ["README.TXT", "A Rather Long File Name With Spaces.txt", "héllo-日本語.txt", "lowercase.txt", "docs", "empty-dir", "big.bin"] {
        assert!(names.iter().any(|x| x == n), "missing {n} in {names:?}");
    }
    assert_eq!(read_all(&fs, "/README.TXT"), b"hello voluma\n");
    assert_eq!(read_all(&fs, "/readme.txt"), b"hello voluma\n");
    assert_eq!(read_all(&fs, "/HÉLLO-日本語.TXT"), b"unicode\n"); // up-case table folding
    assert_eq!(read_all(&fs, "/docs/nested/deep/file.bin"), b"nested\n");
    assert!(fs.read_dir("/empty-dir").unwrap().is_empty());
    assert_eq!(fs.read_dir("/docs").unwrap().iter().filter(|e| e.name.starts_with('f')).count(), 40);
    let data = read_all(&fs, "/big.bin");
    let mut h = Sha256::new();
    for i in 0..(3 * 1024 * 1024 / 16) { h.update(format!("{i:015}\n").as_bytes()); }
    assert_eq!(Sha256::digest(&data), h.finalize());
    assert!(matches!(fs.stat("/nope"), Err(FsError::NotFound(_))));
    assert!(matches!(fs.read_dir("/README.TXT"), Err(FsError::NotADirectory(_))));
    assert_eq!(fs.stat("/docs").unwrap().kind, EntryKind::Directory);
}

#[test]
fn exfat_write_roundtrip_verified_by_macos() {
    let Some((_d, img)) = make("64", "w") else { return };
    let big = pattern(2_500_000, 9);
    {
        let fs = open(&img, true);
        assert!(fs.capabilities().write);
        fs.create_dir("/new dir").unwrap();
        fs.create_dir("/new dir/inner").unwrap();
        fs.write_file("/new dir/hello.txt", &mut Cursor::new(b"written by voluma\n".to_vec()), false).unwrap();
        fs.write_file("/Mixed Case Long Name — ünïcode 日本 that is quite a bit longer than fifteen characters.txt", &mut Cursor::new(b"u".to_vec()), false).unwrap();
        fs.write_file("/new dir/inner/big.bin", &mut Cursor::new(big.clone()), false).unwrap();
        fs.write_file("/empty.dat", &mut Cursor::new(vec![]), false).unwrap();
        assert!(matches!(fs.write_file("/README.TXT", &mut Cursor::new(vec![1]), false), Err(FsError::AlreadyExists(_))));
        fs.write_file("/README.TXT", &mut Cursor::new(b"replaced!\n".to_vec()), true).unwrap();
        fs.write_file("/big.bin", &mut Cursor::new(b"shrunk\n".to_vec()), true).unwrap(); // frees a NoFatChain run
        fs.remove("/lowercase.txt").unwrap();
        fs.rename("/docs/f1.txt", "/new dir/moved.txt").unwrap();
        fs.rename("/empty-dir", "/renamed-dir").unwrap();
        fs.rename("/docs/nested", "/renamed-dir/nested-moved").unwrap();
        assert!(matches!(fs.remove("/docs"), Err(FsError::NotEmpty(_))));
        assert!(fs.rename("/renamed-dir", "/renamed-dir/x/y").is_err());
        assert!(matches!(fs.create_dir("/bad|name"), Err(FsError::InvalidName(_))));
        // Enough files to force directory growth, including growing a pre-existing (possibly NoFatChain) dir.
        for i in 0..400 { fs.write_file(&format!("/docs/grow-file-number-{i}.txt"), &mut Cursor::new(format!("{i}").into_bytes()), false).unwrap(); }
    }
    {
        let fs = open(&img, false);
        assert_eq!(fs.stat("/new dir/inner/big.bin").unwrap().size, big.len() as u64);
        assert_eq!(read_all(&fs, "/new dir/inner/big.bin"), big);
        assert!(matches!(fs.stat("/lowercase.txt"), Err(FsError::NotFound(_))));
    }
    os_verify(&img, |m| {
        assert_eq!(std::fs::read(m.join("new dir/hello.txt")).unwrap(), b"written by voluma\n");
        assert_eq!(std::fs::read(m.join("new dir/inner/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(m.join("Mixed Case Long Name — ünïcode 日本 that is quite a bit longer than fifteen characters.txt")).unwrap(), b"u");
        assert_eq!(std::fs::read(m.join("README.TXT")).unwrap(), b"replaced!\n");
        assert_eq!(std::fs::read(m.join("big.bin")).unwrap(), b"shrunk\n");
        assert_eq!(std::fs::metadata(m.join("empty.dat")).unwrap().len(), 0);
        assert!(!m.join("lowercase.txt").exists());
        assert_eq!(std::fs::read(m.join("new dir/moved.txt")).unwrap(), b"file 1\n");
        assert_eq!(std::fs::read(m.join("renamed-dir/nested-moved/deep/file.bin")).unwrap(), b"nested\n");
        for i in 0..400 { assert_eq!(std::fs::read(m.join(format!("docs/grow-file-number-{i}.txt"))).unwrap(), format!("{i}").into_bytes()); }
        assert_eq!(std::fs::read(m.join("docs/f2.txt")).unwrap(), b"file 2\n");
    });
}

#[test]
fn exfat_space_exhaustion_and_interruption_roll_back() {
    let Some((_d, img)) = make("8", "full") else { return };
    let fs = open(&img, true);
    let r = fs.write_file("/huge.bin", &mut Cursor::new(vec![1u8; 16 << 20]), false);
    assert!(matches!(r, Err(FsError::NoSpace)), "{r:?}");
    assert!(matches!(fs.stat("/huge.bin"), Err(FsError::NotFound(_))));
    struct Fail(usize);
    impl std::io::Read for Fail {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.0 == 0 { return Err(std::io::Error::other("unplugged")); }
            let n = b.len().min(self.0); b[..n].fill(7); self.0 -= n; Ok(n)
        }
    }
    assert!(matches!(fs.write_file("/p.bin", &mut Fail(300_000), false), Err(FsError::Source(_))));
    assert!(matches!(fs.stat("/p.bin"), Err(FsError::NotFound(_))));
    drop(fs);
    os_verify(&img, |m| assert!(!m.join("p.bin").exists() && !m.join("huge.bin").exists()));
    open(&img, true).write_file("/ok.bin", &mut Cursor::new(vec![2u8; 200_000]), false).unwrap();
}

#[test]
fn exfat_guards_and_window_confinement() {
    let Some((_d, img)) = make("64", "guard") else { return };
    let ro = open(&img, false);
    assert!(matches!(ro.create_dir("/x"), Err(FsError::Refused(_))));
    assert!(matches!(ro.remove("/README.TXT"), Err(FsError::Refused(_))));
    drop(ro);
    // Dirty flag (VolumeFlags bit 1 at offset 106).
    let clean = std::fs::read(&img).unwrap();
    let mut dirty = clean.clone();
    dirty[106] |= 0x02;
    let dp = img.with_file_name("dirty.img");
    std::fs::write(&dp, &dirty).unwrap();
    let fs = open(&dp, true);
    assert!(matches!(fs.write_file("/y", &mut Cursor::new(vec![1]), false), Err(FsError::Refused(_))));
    assert!(fs.read_dir("/").is_ok());
    // Partition window: nothing outside the window may change.
    let pad = 1 << 20;
    let mut disk = vec![0xA5u8; pad];
    disk.extend_from_slice(&clean);
    disk.extend(std::iter::repeat(0x5Au8).take(pad));
    let big = img.with_file_name("disk.img");
    std::fs::write(&big, &disk).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_write(&big).unwrap());
    let w = block_device::PartitionWindow::new(parent, pad as u64, clean.len() as u64).unwrap();
    let fs = ExFatFs::open(Arc::new(w)).unwrap();
    for i in 0..40 { fs.write_file(&format!("/w{i}.bin"), &mut Cursor::new(pattern(60_000, i as u8)), false).unwrap(); }
    drop(fs);
    let after = std::fs::read(&big).unwrap();
    assert!(after[..pad].iter().all(|&b| b == 0xA5) && after[pad + clean.len()..].iter().all(|&b| b == 0x5A));
}

#[test]
fn exfat_corrupt_images_never_panic() {
    let Some((_d, img)) = make("64", "cor") else { return };
    let clean = std::fs::read(&img).unwrap();
    let mut x = 0x9E3779B97F4A7C15u64;
    // Corrupt regions that matter: boot sector, FAT head, and the cluster heap head (bitmap/root dir).
    let heap = u32::from_le_bytes(clean[88..92].try_into().unwrap()) as usize * 512;
    let fat = u32::from_le_bytes(clean[80..84].try_into().unwrap()) as usize * 512;
    for round in 0..200 {
        let mut b = clean.clone();
        for _ in 0..10 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let base = match x % 3 { 0 => 0, 1 => fat, _ => heap };
            let at = base + (x >> 8) as usize % 40_000;
            b[at] = (x >> 40) as u8;
        }
        let p = img.with_file_name(format!("c{round}.img"));
        std::fs::write(&p, &b).unwrap();
        let d: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(&p).unwrap());
        if let Ok(fs) = ExFatFs::open(d) {
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
