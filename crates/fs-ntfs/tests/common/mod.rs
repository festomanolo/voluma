#![allow(dead_code)]
//! Shared helpers. NTFS driver judged by ntfs-3g (an independent implementation): images are made by `mkntfs`,
//! populated by the real ntfs-3g library (`tests/fixtures/ntfs_populate.c`), and `ntfsresize -i -f`
//! must call them consistent. The source host directory is the ground truth. Set VOLUMA_NTFS3G to
//! the install prefix from tests/fixtures/build_ntfs3g.sh; without it these tests SKIP loudly.
use block_device::{BlockDevice, ImageDevice};
use fs_core::{EntryKind, Filesystem, FsError};
use fs_ntfs::NtfsFs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

pub fn tool(name: &str) -> Option<PathBuf> {
    let d = std::env::var("VOLUMA_NTFS3G").ok()?;
    ["", "bin", "sbin"].iter().map(|s| Path::new(&d).join(s).join(name)).find(|p| p.is_file())
}
pub fn pattern(n: usize, seed: u8) -> Vec<u8> { (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }

pub fn build_source(root: &Path) {
    use std::fs;
    fs::create_dir_all(root.join("docs/nested/deep")).unwrap();
    fs::create_dir_all(root.join("empty-dir")).unwrap();
    fs::create_dir_all(root.join("many")).unwrap();
    fs::write(root.join("README.TXT"), b"hello voluma\n").unwrap();
    fs::write(root.join("héllo-日本語.txt"), b"unicode\n").unwrap();
    fs::write(root.join("empty.dat"), b"").unwrap();
    fs::write(root.join("docs/nested/deep/file.bin"), b"nested\n").unwrap();
    for i in 0..3000 { fs::write(root.join(format!("many/a-fairly-long-file-name-for-the-index-number-{i}.txt")), format!("file {i}\n")).unwrap(); }
    fs::write(root.join("big.bin"), pattern(3 * 1024 * 1024 + 123, 7)).unwrap();
    fs::write(root.join("docs/just-over-resident.bin"), pattern(1000, 3)).unwrap(); // MFT record is 1 KiB
    fs::write(root.join("docs/small-resident.bin"), pattern(300, 5)).unwrap();
}

pub struct Img { pub _dir: tempfile::TempDir, pub path: PathBuf, pub src: PathBuf }

pub fn make(mb: u32, cluster: u32) -> Option<Img> { make_with(mb, cluster, |_| {}) }

pub fn make_with(mb: u32, cluster: u32, extra: impl FnOnce(&Path)) -> Option<Img> {
    let (Some(mkntfs), Some(pop), Some(_)) = (tool("mkntfs"), tool("ntfs_populate"), tool("ntfsresize")) else {
        eprintln!("SKIPPED: ntfs-3g tools not found (set VOLUMA_NTFS3G)"); return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    build_source(&src);
    extra(&src);
    let path = dir.path().join("t.img");
    std::fs::File::create(&path).unwrap().set_len(mb as u64 * 1024 * 1024).unwrap();
    let o = Command::new(mkntfs).args(["-F", "-Q", "-L", "FIXTURE", "-c", &cluster.to_string()]).arg(&path).output().unwrap();
    assert!(o.status.success(), "mkntfs: {}", String::from_utf8_lossy(&o.stderr));
    let o = Command::new(pop).arg(&path).arg(&src).output().unwrap();
    assert!(o.status.success(), "ntfs_populate: {}", String::from_utf8_lossy(&o.stderr));
    check_consistent(&path);
    Some(Img { _dir: dir, path, src })
}

/// `ntfsresize -i -f` walks every MFT record and cross-checks the cluster bitmap.
pub fn check_consistent(img: &Path) {
    let resize = tool("ntfsresize").unwrap();
    let o = Command::new(resize).args(["-i", "-f"]).arg(img).output().unwrap();
    if !o.status.success() { if let Ok(d) = std::env::var("VOLUMA_KEEP_DIR") { let _ = std::fs::copy(img, Path::new(&d).join("failed.img")); } }
    let txt = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(o.status.success() && txt.contains("Checking filesystem consistency"), "ntfsresize reports problems:\n{txt}");
    assert!(!txt.to_lowercase().contains("error"), "ntfsresize reports problems:\n{txt}");
}

pub fn open(p: &Path) -> Result<NtfsFs, FsError> {
    let d: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(p).unwrap());
    NtfsFs::open(d)
}
pub fn read_all(fs: &NtfsFs, e: &fs_core::DirEntry) -> Vec<u8> {
    let (mut out, mut buf) = (Vec::new(), vec![0u8; 100_003]);
    loop { let n = fs.read_at(e, out.len() as u64, &mut buf).unwrap(); if n == 0 { break; } out.extend_from_slice(&buf[..n]); }
    out
}

pub fn compare(fs: &NtfsFs, vpath: &str, host: &Path, count: &mut usize) {
    let mut ours = fs.read_dir(vpath).unwrap();
    ours.sort_by(|a, b| a.name.cmp(&b.name));
    let mut theirs: Vec<String> = std::fs::read_dir(host).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    theirs.sort();
    let visible: Vec<String> = ours.iter().filter(|e| !e.hidden).map(|e| e.name.clone()).collect();
    assert_eq!(visible, theirs, "listing of {vpath}");
    for e in ours.iter().filter(|e| !e.hidden) {
        let hp = host.join(&e.name);
        let child = format!("{}/{}", vpath.trim_end_matches('/'), e.name);
        *count += 1;
        if hp.is_dir() { assert_eq!(e.kind, EntryKind::Directory, "{child}"); compare(fs, &child, &hp, count); }
        else {
            let want = std::fs::read(&hp).unwrap();
            assert_eq!(e.size, want.len() as u64, "size of {child}");
            assert!(read_all(fs, e) == want, "bytes of {child}");
        }
    }
}


/// Runs an ntfs-3g read tool against the image.
pub fn run_tool(name: &str, args: &[&str], img: &Path) -> Vec<u8> {
    let o = Command::new(tool(name).unwrap()).args(args).arg(img).output().unwrap();
    if !o.status.success() { if let Ok(d) = std::env::var("VOLUMA_KEEP_DIR") { let _ = std::fs::copy(img, Path::new(&d).join("failed_tool.img")); } }
    assert!(o.status.success(), "{name} failed ({:?}): {}", o.status.code(), String::from_utf8_lossy(&o.stderr));
    o.stdout
}
/// `ntfscat IMAGE PATH` (looks the path up through ntfs-3g's own index search).
pub fn ntfscat(img: &Path, path: &str) -> Vec<u8> {
    let o = Command::new(tool("ntfscat").unwrap()).args(["-f"]).arg(img).arg(path).output().unwrap();
    assert!(o.status.success(), "ntfscat {path}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}
pub fn rw(p: &Path) -> NtfsFs {
    let d: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_write(p).unwrap());
    NtfsFs::open(d).unwrap()
}
