//! Write-path tests. After mutating an image with VOLUMA, macOS itself is the judge:
//! `fsck_msdos -n` must pass and a read-only OS mount must show identical content.

use block_device::{BlockDevice, ImageDevice};
use fs_core::{EntryKind, Filesystem, FsError};
use fs_fat::FatFs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn make(fs: &str, mb: &str, tag: &str) -> Option<(tempfile::TempDir, PathBuf)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: needs macOS"); return None; }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join(format!("{tag}.img"));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/make_fat_fixture.sh");
    let out = Command::new("sh").args([script, img.to_str().unwrap(), fs, mb]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some((dir, img))
}

fn open_rw(img: &Path) -> FatFs {
    let dev: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_write(img).unwrap());
    FatFs::open(dev).unwrap()
}

fn pattern(n: usize, seed: u8) -> Vec<u8> { (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }

/// Attach read-only (no mount), run the OS FAT checker, then mount and hand the tree to `check`.
fn os_verify(img: &Path, fsck: &str, check: impl FnOnce(&Path)) {
    let out = Command::new("hdiutil").args(["attach", "-imagekey", "diskimage-class=CRawDiskImage", "-readonly", "-nomount"])
        .arg(img).output().unwrap();
    assert!(out.status.success(), "attach: {}", String::from_utf8_lossy(&out.stderr));
    let dev = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string();
    let rdev = dev.replace("/dev/disk", "/dev/rdisk");
    let f = Command::new(fsck).args(["-n", &rdev]).output().unwrap();
    let ftxt = format!("{}{}", String::from_utf8_lossy(&f.stdout), String::from_utf8_lossy(&f.stderr));
    let _ = Command::new("hdiutil").args(["detach", &dev]).output();
    assert!(f.status.success(), "{fsck} reported problems:\n{ftxt}");

    let mnt = tempfile::tempdir().unwrap();
    let a = Command::new("hdiutil").args(["attach", "-imagekey", "diskimage-class=CRawDiskImage", "-readonly", "-nobrowse", "-mountpoint"])
        .arg(mnt.path()).arg(img).output().unwrap();
    assert!(a.status.success(), "mount: {}", String::from_utf8_lossy(&a.stderr));
    check(mnt.path());
    let _ = Command::new("hdiutil").args(["detach", mnt.path().to_str().unwrap(), "-force"]).output();
}

#[test]
fn fat32_write_roundtrip_verified_by_macos() {
    let Some((_d, img)) = make("MS-DOS FAT32", "64", "w32") else { return };
    let big = pattern(2_500_000, 7);
    {
        let fs = open_rw(&img);
        assert!(fs.capabilities().write);
        fs.create_dir("/new dir").unwrap();
        fs.create_dir("/new dir/inner").unwrap();
        fs.write_file("/new dir/hello.txt", &mut Cursor::new(b"written by voluma\n".to_vec()), false).unwrap();
        fs.write_file("/Mixed Case Long Name — ünïcode 日本.txt", &mut Cursor::new(b"u".to_vec()), false).unwrap();
        fs.write_file("/new dir/inner/big.bin", &mut Cursor::new(big.clone()), false).unwrap();
        fs.write_file("/empty.dat", &mut Cursor::new(vec![]), false).unwrap();
        // Overwrite existing with different size.
        assert!(matches!(fs.write_file("/README.TXT", &mut Cursor::new(b"x".to_vec()), false), Err(FsError::AlreadyExists(_))));
        fs.write_file("/README.TXT", &mut Cursor::new(b"replaced!\n".to_vec()), true).unwrap();
        // Delete & rename.
        fs.remove("/lowercase.txt").unwrap();
        fs.rename("/docs/f1.txt", "/new dir/moved.txt").unwrap();
        fs.rename("/empty-dir", "/renamed-dir").unwrap();
        fs.rename("/docs/nested", "/renamed-dir/nested-moved").unwrap(); // dir move across parents
        assert!(matches!(fs.remove("/docs"), Err(FsError::NotEmpty(_))));
        assert!(matches!(fs.rename("/renamed-dir", "/renamed-dir/x/y"), Err(_)));
        assert!(matches!(fs.create_dir("/bad:name"), Err(FsError::InvalidName(_))));
        // Many files force the directory to grow by a cluster (512-byte clusters => 16 slots).
        for i in 0..60 { fs.write_file(&format!("/docs/grow-{i}.txt"), &mut Cursor::new(format!("{i}").into_bytes()), false).unwrap(); }
    }
    // Re-read with a fresh driver instance.
    {
        let dev: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(&img).unwrap());
        let fs = FatFs::open(dev).unwrap();
        let e = fs.stat("/new dir/inner/big.bin").unwrap();
        assert_eq!(e.size, big.len() as u64);
        assert!(matches!(fs.stat("/lowercase.txt"), Err(FsError::NotFound(_))));
        assert_eq!(fs.stat("/renamed-dir/nested-moved").unwrap().kind, EntryKind::Directory);
        // ".." of the moved dir must point at its new parent for the OS checker (below).
    }
    os_verify(&img, "fsck_msdos", |m| {
        assert_eq!(std::fs::read(m.join("new dir/hello.txt")).unwrap(), b"written by voluma\n");
        assert_eq!(std::fs::read(m.join("new dir/inner/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(m.join("Mixed Case Long Name — ünïcode 日本.txt")).unwrap(), b"u");
        assert_eq!(std::fs::read(m.join("README.TXT")).unwrap(), b"replaced!\n");
        assert_eq!(std::fs::metadata(m.join("empty.dat")).unwrap().len(), 0);
        assert!(!m.join("lowercase.txt").exists());
        assert_eq!(std::fs::read(m.join("new dir/moved.txt")).unwrap(), b"file 1\n");
        assert_eq!(std::fs::read(m.join("renamed-dir/nested-moved/deep/file.bin")).unwrap(), b"nested\n");
        for i in 0..60 { assert_eq!(std::fs::read(m.join(format!("docs/grow-{i}.txt"))).unwrap(), format!("{i}").into_bytes()); }
        // Untouched pre-existing content survives.
        assert_eq!(std::fs::read(m.join("docs/f2.txt")).unwrap(), b"file 2\n");
        assert_eq!(std::fs::metadata(m.join("big.bin")).unwrap().len(), 3 * 1024 * 1024);
    });
}

#[test]
fn fat16_and_fat12_write_verified_by_macos() {
    for (fs_name, mb, tag) in [("MS-DOS FAT16", "16", "w16"), ("MS-DOS FAT12", "2", "w12")] {
        let Some((_d, img)) = make(fs_name, mb, tag) else { return };
        {
            let fs = open_rw(&img);
            fs.create_dir("/sub").unwrap();
            fs.write_file("/sub/a.txt", &mut Cursor::new(pattern(20_000, 3)), false).unwrap();
            fs.write_file("/b.TXT", &mut Cursor::new(b"b".to_vec()), false).unwrap();
            fs.remove("/docs/f5.txt").unwrap();
            fs.rename("/b.TXT", "/sub/b renamed.txt").unwrap();
        }
        os_verify(&img, "fsck_msdos", |m| {
            assert_eq!(std::fs::read(m.join("sub/a.txt")).unwrap(), pattern(20_000, 3));
            assert_eq!(std::fs::read(m.join("sub/b renamed.txt")).unwrap(), b"b");
            assert!(!m.join("docs/f5.txt").exists());
        });
    }
}

#[test]
fn space_exhaustion_rolls_back_cleanly() {
    let Some((_d, img)) = make("MS-DOS FAT12", "2", "full") else { return };
    let fs = open_rw(&img);
    let r = fs.write_file("/huge.bin", &mut Cursor::new(vec![1u8; 4 << 20]), false);
    assert!(matches!(r, Err(FsError::NoSpace)), "{r:?}");
    assert!(matches!(fs.stat("/huge.bin"), Err(FsError::NotFound(_))));
    drop(fs);
    // Rollback released every cluster: macOS must see a consistent volume with no orphans.
    os_verify(&img, "fsck_msdos", |_| {});
    // And a normal file still fits afterwards.
    let fs = open_rw(&img);
    fs.write_file("/ok.bin", &mut Cursor::new(vec![2u8; 100_000]), false).unwrap();
}

struct FailingReader(usize);
impl std::io::Read for FailingReader {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.0 == 0 { return Err(std::io::Error::other("disk unplugged")); }
        let n = b.len().min(self.0); b[..n].fill(9); self.0 -= n; Ok(n)
    }
}

#[test]
fn interrupted_source_leaves_no_trace() {
    let Some((_d, img)) = make("MS-DOS FAT16", "16", "intr") else { return };
    let fs = open_rw(&img);
    let r = fs.write_file("/partial.bin", &mut FailingReader(100_000), false);
    assert!(matches!(r, Err(FsError::Source(_))));
    assert!(matches!(fs.stat("/partial.bin"), Err(FsError::NotFound(_))));
    drop(fs);
    os_verify(&img, "fsck_msdos", |m| assert!(!m.join("partial.bin").exists()));
}

#[test]
fn read_only_device_and_dirty_volume_refuse_writes() {
    let Some((_d, img)) = make("MS-DOS FAT32", "64", "guard") else { return };
    // 1. Read-only device.
    let dev: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(&img).unwrap());
    let fs = FatFs::open(dev).unwrap();
    assert!(!fs.capabilities().write);
    assert!(matches!(fs.create_dir("/x"), Err(FsError::Refused(_))));
    assert!(matches!(fs.remove("/README.TXT"), Err(FsError::Refused(_))));
    drop(fs);
    // 2. Dirty volume: clear the FAT32 clean-shutdown bit (FAT[1] bit 27) and expect refusal.
    let mut bytes = std::fs::read(&img).unwrap();
    let reserved = u16::from_le_bytes([bytes[14], bytes[15]]) as usize * 512;
    let mut e1 = u32::from_le_bytes(bytes[reserved + 4..reserved + 8].try_into().unwrap());
    e1 &= !0x0800_0000;
    bytes[reserved + 4..reserved + 8].copy_from_slice(&e1.to_le_bytes());
    std::fs::write(&img, &bytes).unwrap();
    let fs = open_rw(&img);
    assert!(matches!(fs.write_file("/y", &mut Cursor::new(vec![1]), false), Err(FsError::Refused(_))));
    // Reading a dirty volume is still allowed.
    assert!(fs.read_dir("/").is_ok());
}

#[test]
fn partition_window_confines_writes() {
    // The driver is handed only a window; verify bytes outside it never change.
    let Some((_d, img)) = make("MS-DOS FAT16", "16", "win") else { return };
    let fat = std::fs::read(&img).unwrap();
    let pad = 1 << 20;
    let mut disk = vec![0xA5u8; pad];
    disk.extend_from_slice(&fat);
    disk.extend(std::iter::repeat(0x5Au8).take(pad));
    let big = img.with_file_name("disk.img");
    std::fs::write(&big, &disk).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_write(&big).unwrap());
    let w = block_device::PartitionWindow::new(parent, pad as u64, fat.len() as u64).unwrap();
    let fs = FatFs::open(Arc::new(w)).unwrap();
    for i in 0..30 { fs.write_file(&format!("/w{i}.bin"), &mut Cursor::new(pattern(50_000, i as u8)), false).unwrap(); }
    drop(fs);
    let after = std::fs::read(&big).unwrap();
    assert!(after[..pad].iter().all(|&b| b == 0xA5), "bytes before the partition changed");
    assert!(after[pad + fat.len()..].iter().all(|&b| b == 0x5A), "bytes after the partition changed");
}
