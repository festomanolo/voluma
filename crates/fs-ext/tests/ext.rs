//! ext2/3/4 driver judged against real e2fsprogs output: images are built by `mke2fs -d` from a
//! host directory (so the host tree is the ground truth), checked clean by `e2fsck -fn`, then the
//! driver's whole tree is compared with the source. Set VOLUMA_E2FSPROGS to the directory holding
//! `mke2fs`/`e2fsck` (see tests/fixtures/build_e2fsprogs.sh); without them these tests SKIP loudly.
use block_device::{BlockDevice, ImageDevice};
use fs_core::{EntryKind, Filesystem, FsError};
use fs_ext::ExtFs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn tool(name: &str) -> Option<PathBuf> {
    if let Ok(d) = std::env::var("VOLUMA_E2FSPROGS") {
        for sub in ["", "sbin", "bin"] { let p = Path::new(&d).join(sub).join(name); if p.exists() { return Some(p); } }
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|p| p.exists()))
}

fn pattern(n: usize, seed: u8) -> Vec<u8> { (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }

fn build_source(root: &Path) {
    use std::fs;
    fs::create_dir_all(root.join("docs/nested/deep")).unwrap();
    fs::create_dir_all(root.join("empty-dir")).unwrap();
    fs::create_dir_all(root.join("many")).unwrap();
    fs::write(root.join("README.TXT"), b"hello voluma\n").unwrap();
    fs::write(root.join("héllo-日本語.txt"), b"unicode\n").unwrap();
    fs::write(root.join("empty.dat"), b"").unwrap();
    fs::write(root.join("docs/nested/deep/file.bin"), b"nested\n").unwrap();
    for i in 0..700 { fs::write(root.join(format!("many/a-fairly-long-file-name-number-{i}.txt")), format!("file {i}\n")).unwrap(); }
    fs::write(root.join("big.bin"), pattern(3 * 1024 * 1024 + 123, 7)).unwrap();
    // Sparse file: islands of data far apart -> many extents (depth-1 tree) / indirect levels.
    let f = fs::File::create(root.join("sparse.bin")).unwrap();
    f.set_len(90 * 1024 * 1024).unwrap();
    use std::os::unix::fs::FileExt;
    for k in 0..10u64 { f.write_all_at(&pattern(5000, k as u8), k * 1_000_003 + 17).unwrap(); }
    f.write_all_at(&pattern(4096, 99), 80 * 1024 * 1024).unwrap();
    fs::hard_link(root.join("README.TXT"), root.join("docs/hardlink.txt")).unwrap();
    std::os::unix::fs::symlink("README.TXT", root.join("short-link")).unwrap();
    let long = format!("docs/nested/deep/{}", "x".repeat(80));
    std::os::unix::fs::symlink(&long, root.join("long-link")).unwrap();
}

struct Img { _dir: tempfile::TempDir, path: PathBuf, src: PathBuf }

fn make(fstype: &str, bs: u32, extra: &[&str], mb: u32) -> Option<Img> {
    let (Some(mke2fs), Some(fsck)) = (tool("mke2fs"), tool("e2fsck")) else { eprintln!("SKIPPED: e2fsprogs not found (set VOLUMA_E2FSPROGS)"); return None };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    build_source(&src);
    let path = dir.path().join("t.img");
    let o = Command::new(mke2fs).args(["-q", "-t", fstype, "-b", &bs.to_string(), "-L", "FIXTURE", "-d"]).arg(&src)
        .args(extra).arg(&path).arg(format!("{mb}M")).output().unwrap();
    assert!(o.status.success(), "mke2fs: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let c = Command::new(fsck).args(["-fn"]).arg(&path).output().unwrap();
    assert!(c.status.success(), "fixture itself is not clean: {}", String::from_utf8_lossy(&c.stdout));
    Some(Img { _dir: dir, path, src })
}

fn open(p: &Path) -> Result<ExtFs, FsError> {
    let d: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(p).unwrap());
    ExtFs::open(d)
}
fn read_all(fs: &ExtFs, e: &fs_core::DirEntry) -> Vec<u8> {
    let (mut out, mut buf) = (Vec::new(), vec![0u8; 100_003]);
    loop { let n = fs.read_at(e, out.len() as u64, &mut buf).unwrap(); if n == 0 { break; } out.extend_from_slice(&buf[..n]); }
    out
}

fn compare(fs: &ExtFs, vpath: &str, host: &Path, count: &mut usize) {
    let mut ours = fs.read_dir(vpath).unwrap();
    ours.retain(|e| !(vpath == "/" && e.name == "lost+found"));
    ours.sort_by(|a, b| a.name.cmp(&b.name));
    let mut theirs: Vec<String> = std::fs::read_dir(host).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    theirs.sort();
    assert_eq!(ours.iter().map(|e| e.name.clone()).collect::<Vec<_>>(), theirs, "listing of {vpath}");
    for e in &ours {
        let hp = host.join(&e.name);
        let child = format!("{}/{}", vpath.trim_end_matches('/'), e.name);
        let md = std::fs::symlink_metadata(&hp).unwrap();
        *count += 1;
        if md.is_dir() { assert_eq!(e.kind, EntryKind::Directory, "{child}"); compare(fs, &child, &hp, count); }
        else {
            let want = if md.file_type().is_symlink() { std::fs::read_link(&hp).unwrap().to_string_lossy().into_owned().into_bytes() } else { std::fs::read(&hp).unwrap() };
            assert_eq!(e.size, want.len() as u64, "size of {child}");
            assert!(read_all(fs, e) == want, "bytes of {child}");
        }
    }
}

fn whole_tree(fstype: &str, bs: u32, extra: &[&str], mb: u32, expect: &str) {
    let Some(img) = make(fstype, bs, extra, mb) else { return };
    let fs = open(&img.path).unwrap();
    assert_eq!(fs.name(), expect);
    assert_eq!(fs.label().as_deref(), Some("FIXTURE"));
    assert!(!fs.capabilities().write);
    let mut n = 0;
    compare(&fs, "/", &img.src, &mut n);
    assert!(n > 700, "only walked {n}");
    assert_eq!(fs.stat("/README.TXT").unwrap().size, 13);
    assert!(matches!(fs.stat("/readme.txt"), Err(FsError::NotFound(_))), "ext is case-sensitive");
    assert!(matches!(fs.create_dir("/x"), Err(FsError::Unsupported(_))));
}

#[test] fn ext2_1k_indirect_blocks() { whole_tree("ext2", 1024, &[], 160, "ext2"); }
#[test] fn ext3_4k() { whole_tree("ext3", 4096, &[], 160, "ext3"); }
#[test] fn ext4_4k_extents() { whole_tree("ext4", 4096, &[], 160, "ext4"); }
#[test] fn ext4_1k_extents() { whole_tree("ext4", 1024, &[], 160, "ext4"); }
#[test] fn ext4_64bit_meta_bg_no_flex() { whole_tree("ext4", 4096, &["-O", "64bit,meta_bg,^resize_inode,^flex_bg"], 160, "ext4"); }

#[test]
fn refuses_unsafe_or_unknown_volumes() {
    let Some(img) = make("ext4", 4096, &[], 16) else { return };
    let patch = |bit: u32| {
        let p = img.path.with_extension("p");
        let mut b = std::fs::read(&img.path).unwrap();
        let o = 1024 + 96;
        let v = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) | bit;
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        std::fs::write(&p, b).unwrap(); p
    };
    assert!(matches!(open(&patch(0x4)), Err(FsError::Refused(_))), "needs_recovery must be refused");
    assert!(matches!(open(&patch(0x1)), Err(FsError::Unsupported(_))), "compression");
    assert!(matches!(open(&patch(0x8000_0000)), Err(FsError::Unsupported(_))), "unknown feature");
}

#[test]
fn corruption_never_panics() {
    let Some(img) = make("ext4", 1024, &[], 16) else { return };
    let clean = std::fs::read(&img.path).unwrap();
    let mut seed = 0xBEEF_1234u32;
    for round in 0..60 {
        let mut b = clean.clone();
        for _ in 0..40 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let off = match round % 3 { 0 => 1024 + seed as usize % 1024, 1 => 2048 + seed as usize % 2048, _ => seed as usize % (2 << 20) };
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
