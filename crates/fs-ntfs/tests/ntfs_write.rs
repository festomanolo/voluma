//! NTFS write tests. Every mutation is judged by ntfs-3g: `ntfsresize -i -f` must find the volume
//! consistent (MFT records, attributes, cluster bitmap), `ntfscat` must read each file back through
//! ntfs-3g's own B-tree search (so index ordering is verified), and the host source tree stays the oracle.
mod common;
use common::*;
use fs_core::{EntryKind, Filesystem, FsError};
use std::io::Cursor;

fn same(a: &[u8], b: impl AsRef<[u8]>, what: &str)
{ let b = b.as_ref(); same2(a, b, what) }
fn same2(a: &[u8], b: &[u8], what: &str) {
    if a != b {
        let i = a.iter().zip(b).position(|(x, y)| x != y);
        panic!("{what}: lengths {} vs {}, first difference at {:?}", a.len(), b.len(), i);
    }
}
fn put(fs: &fs_ntfs::NtfsFs, p: &str, d: &[u8]) { fs.write_file(p, &mut Cursor::new(d.to_vec()), false).unwrap(); }

#[test]
fn create_write_and_read_back_with_ntfs3g() {
    let Some(img) = make(96, 4096) else { return };
    let big = pattern(2_500_000, 9);
    {
        let fs = rw(&img.path);
        assert!(fs.capabilities().write, "{:?}", fs.write_blocked_reason());
        fs.create_dir("/new dir").unwrap();
        fs.create_dir("/new dir/inner").unwrap();
        put(&fs, "/new dir/hello.txt", b"written by voluma\n");
        put(&fs, "/Mixed Case — ünïcode 日本.txt", b"u");
        put(&fs, "/new dir/inner/big.bin", &big);
        put(&fs, "/empty2.dat", b"");
        put(&fs, "/resident.bin", &pattern(300, 1));
        put(&fs, "/boundary.bin", &pattern(900, 2));
        put(&fs, "/nonresident.bin", &pattern(1000, 3));
        assert!(matches!(fs.write_file("/README.TXT", &mut Cursor::new(vec![1]), false), Err(FsError::AlreadyExists(_))));
        assert!(matches!(fs.create_dir("/new dir"), Err(FsError::AlreadyExists(_))));
        assert!(matches!(fs.create_dir("/bad|name"), Err(FsError::InvalidName(_))));
        assert!(matches!(fs.remove("/docs"), Err(FsError::NotEmpty(_))));
        assert!(matches!(fs.stat("/new dir").unwrap().kind, EntryKind::Directory));
        same(&read_all(&fs, &fs.stat("/new dir/inner/big.bin").unwrap()), &big, "driver read-back");
    }
    check_consistent(&img.path);
    same(&ntfscat(&img.path, "/new dir/hello.txt"), b"written by voluma\n", "/new dir/hello.txt");
    same(&ntfscat(&img.path, "/new dir/inner/big.bin"), &big, "/new dir/inner/big.bin");
    same(&ntfscat(&img.path, "/Mixed Case — ünïcode 日本.txt"), b"u", "/Mixed Case — ünïcode 日本.txt");
    same(&ntfscat(&img.path, "/resident.bin"), &pattern(300, 1), "/resident.bin");
    same(&ntfscat(&img.path, "/boundary.bin"), &pattern(900, 2), "/boundary.bin");
    same(&ntfscat(&img.path, "/nonresident.bin"), &pattern(1000, 3), "/nonresident.bin");
    same(&ntfscat(&img.path, "/empty2.dat"), b"", "/empty2.dat");
    // original content untouched
    same(&ntfscat(&img.path, "/README.TXT"), b"hello voluma\n", "/README.TXT");
    let fs = open(&img.path).unwrap();
    same(&read_all(&fs, &fs.stat("/boundary.bin").unwrap()), pattern(900, 2), "boundary");
}

#[test]
fn many_inserts_split_the_index_and_stay_ordered() {
    let Some(img) = make(96, 4096) else { return };
    {
        let fs = rw(&img.path);
        // interleave names with the 3000 existing ones so splits happen all over the tree
        for i in 0..400 { put(&fs, &format!("/many/a-fairly-long-file-name-for-the-index-number-{i}-new.txt"), format!("n{i}").as_bytes()); }
        for i in 0..60 { put(&fs, &format!("/many/Z{i:03}"), b"z"); }
        for i in 0..60 { put(&fs, &format!("/many/!first{i:03}"), b"f"); }
        assert_eq!(fs.verify_dir_index("/many").unwrap(), 3000 + 400 + 120);
    }
    check_consistent(&img.path);
    let ls = String::from_utf8(run_tool("ntfsls", &["-p", "/many"], &img.path)).unwrap();
    assert_eq!(ls.lines().filter(|l| *l != "." && *l != "..").count(), 3000 + 400 + 120);
    same(&ntfscat(&img.path, "/many/a-fairly-long-file-name-for-the-index-number-399-new.txt"), b"n399", "/many/a-fairly-long-file-name-for-the-index-number-399-new.txt");
    same(&ntfscat(&img.path, "/many/Z059"), b"z", "/many/Z059");
    same(&ntfscat(&img.path, "/many/!first000"), b"f", "/many/!first000");
    same(&ntfscat(&img.path, "/many/a-fairly-long-file-name-for-the-index-number-1234.txt"), b"file 1234\n", "/many/a-fairly-long-file-name-for-the-index-number-1234.txt");
}

#[test]
fn small_directory_grows_from_root_into_blocks() {
    let Some(img) = make(64, 4096) else { return };
    {
        let fs = rw(&img.path);
        fs.create_dir("/grow").unwrap();
        for i in 0..250 { put(&fs, &format!("/grow/file-with-a-moderately-long-name-{i:04}.dat"), format!("{i}").as_bytes()); }
        assert_eq!(fs.verify_dir_index("/grow").unwrap(), 250);
    }
    check_consistent(&img.path);
    for i in [0, 1, 124, 249] { assert_eq!(ntfscat(&img.path, &format!("/grow/file-with-a-moderately-long-name-{i:04}.dat")), format!("{i}").as_bytes()); }
}

#[test]
fn overwrite_resizes_between_resident_and_nonresident() {
    let Some(img) = make(64, 4096) else { return };
    {
        let fs = rw(&img.path);
        fs.write_file("/README.TXT", &mut Cursor::new(pattern(200_000, 4)), true).unwrap();
        fs.write_file("/docs/small-resident.bin", &mut Cursor::new(pattern(50_000, 6)), true).unwrap(); // resident -> non-resident
        fs.write_file("/big.bin", &mut Cursor::new(b"shrunk\n".to_vec()), true).unwrap(); // non-resident -> resident, frees clusters
        fs.write_file("/docs/just-over-resident.bin", &mut Cursor::new(pattern(1000, 3)), true).unwrap();
        assert!(matches!(fs.write_file("/docs", &mut Cursor::new(vec![1]), true), Err(FsError::IsADirectory(_))));
    }
    check_consistent(&img.path);
    same(&ntfscat(&img.path, "/README.TXT"), &pattern(200_000, 4), "/README.TXT");
    same(&ntfscat(&img.path, "/docs/small-resident.bin"), &pattern(50_000, 6), "/docs/small-resident.bin");
    same(&ntfscat(&img.path, "/big.bin"), b"shrunk\n", "/big.bin");
    let fs = open(&img.path).unwrap();
    assert_eq!(fs.stat("/big.bin").unwrap().size, 7);
}

#[test]
fn remove_files_dirs_and_index_entries() {
    let Some(img) = make(96, 4096) else { return };
    let removed: Vec<usize> = (0..3000).filter(|i| i % 3 == 0 || (1000..1100).contains(i)).collect();
    {
        let fs = rw(&img.path);
        fs.remove("/big.bin").unwrap();
        fs.remove("/empty.dat").unwrap();
        fs.remove("/docs/nested/deep/file.bin").unwrap();
        fs.remove("/docs/nested/deep").unwrap();
        fs.remove("/docs/nested").unwrap();
        fs.remove("/empty-dir").unwrap();
        assert!(matches!(fs.remove("/nope"), Err(FsError::NotFound(_))));
        assert!(matches!(fs.remove("/many"), Err(FsError::NotEmpty(_))));
        for &i in &removed { fs.remove(&format!("/many/a-fairly-long-file-name-for-the-index-number-{i}.txt")).unwrap(); }
        assert_eq!(fs.verify_dir_index("/many").unwrap(), 3000 - removed.len());
        // the freed MFT records and clusters are reusable
        put(&fs, "/reuse.bin", &pattern(100_000, 1));
        put(&fs, "/many/after-delete.txt", b"x");
    }
    check_consistent(&img.path);
    assert!(std::process::Command::new(tool("ntfscat").unwrap()).args(["-f"]).arg(&img.path).arg("/big.bin").output().unwrap().stdout.is_empty());
    for i in [1, 2, 4, 998, 1100, 2999] { assert_eq!(ntfscat(&img.path, &format!("/many/a-fairly-long-file-name-for-the-index-number-{i}.txt")), format!("file {i}\n").as_bytes()); }
    same(&ntfscat(&img.path, "/reuse.bin"), &pattern(100_000, 1), "/reuse.bin");
    let fs = open(&img.path).unwrap();
    assert!(matches!(fs.stat("/many/a-fairly-long-file-name-for-the-index-number-0.txt"), Err(FsError::NotFound(_))));
    assert!(matches!(fs.stat("/big.bin"), Err(FsError::NotFound(_))));
}

#[test]
fn rename_and_move() {
    let Some(img) = make(64, 4096) else { return };
    {
        let fs = rw(&img.path);
        fs.rename("/README.TXT", "/renamed readme with a longer name.txt").unwrap();
        fs.rename("/docs/nested", "/moved-nested").unwrap();           // directory with children moves
        fs.rename("/docs/small-resident.bin", "/many/zz-moved.bin").unwrap();
        assert!(matches!(fs.rename("/docs", "/docs/inside"), Err(FsError::InvalidName(_))));
        assert!(matches!(fs.rename("/empty.dat", "/docs"), Err(FsError::AlreadyExists(_))));
        assert!(matches!(fs.rename("/nope", "/x"), Err(FsError::NotFound(_))));
        assert_eq!(fs.verify_dir_index("/many").unwrap(), 3001);
    }
    check_consistent(&img.path);
    same(&ntfscat(&img.path, "/renamed readme with a longer name.txt"), b"hello voluma\n", "/renamed readme with a longer name.txt");
    same(&ntfscat(&img.path, "/moved-nested/deep/file.bin"), b"nested\n", "/moved-nested/deep/file.bin");
    same(&ntfscat(&img.path, "/many/zz-moved.bin"), &pattern(300, 5), "/many/zz-moved.bin");
    let fs = open(&img.path).unwrap();
    assert!(matches!(fs.stat("/README.TXT"), Err(FsError::NotFound(_))));
    assert!(matches!(fs.stat("/docs/nested"), Err(FsError::NotFound(_))));
}

#[test]
fn refuses_unsafe_states() {
    let Some(img) = make(48, 4096) else { return };
    // read-only device
    let fs = open(&img.path).unwrap();
    assert!(!fs.capabilities().write);
    assert!(matches!(fs.create_dir("/x"), Err(FsError::Refused(_))));
    assert!(matches!(fs.remove("/README.TXT"), Err(FsError::Refused(_))));
    // a clean volume is writable, and a completed operation leaves it clean again
    let fs = rw(&img.path);
    assert_eq!(fs.write_blocked(), None);
    fs.create_dir("/ok").unwrap();
    drop(fs);
    assert_eq!(open(&img.path).unwrap().write_blocked(), None, "dirty bit must be cleared after an operation");
    // system files are protected
    let fs = rw(&img.path);
    assert!(matches!(fs.remove("/$MFT"), Err(_)));
    // a crashed writer left the dirty bit set -> refuse until chkdsk
    fs.debug_set_volume_dirty().unwrap();
    drop(fs);
    let fs = rw(&img.path);
    assert!(fs.write_blocked().unwrap().contains("dirty"));
    assert!(!fs.capabilities().write);
    assert!(matches!(fs.create_dir("/blocked"), Err(FsError::Refused(_))));
    assert!(matches!(fs.write_file("/blocked.txt", &mut Cursor::new(vec![1]), false), Err(FsError::Refused(_))));
    // reading still works
    assert_eq!(read_all(&fs, &fs.stat("/README.TXT").unwrap()), b"hello voluma\n");
    // ntfsfix -d clears the dirty bit, after which writing is allowed again
    let o = std::process::Command::new(tool("ntfsfix").unwrap()).args(["-d"]).arg(&img.path).output().unwrap();
    assert!(o.status.success());
    assert_eq!(open(&img.path).unwrap().write_blocked(), None);
}

#[test]
fn refuses_unclean_logfile_and_hibernation() {
    // Unclean $LogFile (RSTR page without the clean flag)
    let Some(img) = make(48, 4096) else { return };
    let off = open(&img.path).unwrap().debug_logfile_offset().unwrap() as usize;
    let mut raw = std::fs::read(&img.path).unwrap();
    raw[off..off + 4].copy_from_slice(b"RSTR");
    raw[off + 0x18..off + 0x1A].copy_from_slice(&0x30u16.to_le_bytes());
    raw[off + 0x30 + 0x0E..off + 0x30 + 0x10].copy_from_slice(&0u16.to_le_bytes());
    std::fs::write(&img.path, &raw).unwrap();
    let fs = rw(&img.path);
    assert!(fs.write_blocked().unwrap().contains("LogFile"), "{:?}", fs.write_blocked());
    assert!(matches!(fs.create_dir("/x"), Err(FsError::Refused(_))));
    // the same RSTR page flagged clean is accepted
    raw[off + 0x30 + 0x0E..off + 0x30 + 0x10].copy_from_slice(&2u16.to_le_bytes());
    std::fs::write(&img.path, &raw).unwrap();
    assert_eq!(rw(&img.path).write_blocked(), None);

    // Windows hibernation (Fast Startup) marker in hiberfil.sys
    let Some(img) = make_with(48, 4096, |src| std::fs::write(src.join("hiberfil.sys"), b"hibr\0\0\0\0 hibernation image").unwrap()) else { return };
    let fs = rw(&img.path);
    assert!(fs.write_blocked().unwrap().contains("hibernated"), "{:?}", fs.write_blocked());
    assert!(matches!(fs.create_dir("/x"), Err(FsError::Refused(_))));
    // an ordinary file with that name but different content does not block
    let Some(img) = make_with(48, 4096, |src| std::fs::write(src.join("hiberfil.sys"), b"not a hibernation file").unwrap()) else { return };
    assert_eq!(rw(&img.path).write_blocked(), None);
}

#[test]
fn out_of_space_rolls_back_cleanly() {
    let Some(img) = make(24, 4096) else { return };
    let before = std::fs::read(&img.path).unwrap();
    let fs = rw(&img.path);
    let mut big = Cursor::new(vec![7u8; 40 * 1024 * 1024]);
    assert!(matches!(fs.write_file("/huge.bin", &mut big, false), Err(FsError::NoSpace)));
    assert!(matches!(fs.stat("/huge.bin"), Err(FsError::NotFound(_))));
    drop(fs);
    check_consistent(&img.path);
    // Cluster bitmap is identical to before: nothing leaked.
    let usage = |p: &std::path::Path| String::from_utf8_lossy(&run_tool("ntfsresize", &["-i", "-f"], p)).lines().find(|l| l.contains("Space in use")).unwrap().to_string();
    let a = tempfile::NamedTempFile::new().unwrap(); std::fs::write(a.path(), before).unwrap();
    assert_eq!(usage(&img.path), usage(a.path()));
}

#[test]
fn corrupted_volumes_never_panic_on_writes() {
    let Some(img) = make(32, 4096) else { return };
    let clean = std::fs::read(&img.path).unwrap();
    let mut seed = 0xFEED_5EEDu32;
    for round in 0..150 {
        let mut b = clean.clone();
        for _ in 0..30 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            // boot sector / the MFT (LCN 4) / the first 4 MiB / anywhere
            let off = match round % 4 { 0 => seed as usize % 512, 1 => 16384 + (seed as usize) % (96 * 1024), 2 => (seed as usize) % (4 << 20), _ => (seed as usize) % b.len() };
            b[off] = (seed >> 24) as u8;
        }
        let p = img.path.with_extension(format!("w{round}"));
        std::fs::write(&p, &b).unwrap();
        let d: std::sync::Arc<dyn block_device::BlockDevice> = std::sync::Arc::new(block_device::ImageDevice::open_read_write(&p).unwrap());
        if let Ok(fs) = fs_ntfs::NtfsFs::open(d) {
            let _ = fs.create_dir("/fuzz");
            let _ = fs.write_file("/fuzz/a.bin", &mut Cursor::new(pattern(5000, 1)), false);
            let _ = fs.write_file("/README.TXT", &mut Cursor::new(pattern(50, 2)), true);
            let _ = fs.rename("/docs", "/docs2");
            let _ = fs.remove("/empty.dat");
            let _ = fs.remove("/many");
        }
        std::fs::remove_file(&p).unwrap();
    }
}

#[test]
fn run_lengths_with_the_high_bit_set_are_encoded_signed() {
    // 700000 bytes = 171 clusters = 0xAB: a one-byte length would read as negative to ntfs-3g/Windows.
    let Some(img) = make(40, 4096) else { return };
    {
        let fs = rw(&img.path);
        fs.create_dir("/pasted").unwrap();
        fs.write_file("/pasted/big.bin", &mut Cursor::new(vec![0x5Au8; 700_000]), false).unwrap();
        fs.rename("/pasted/big.bin", "/pasted/renamed.bin").unwrap();
        fs.write_file("/pasted/128.bin", &mut Cursor::new(vec![1u8; 128 * 4096]), false).unwrap();
        fs.write_file("/pasted/255.bin", &mut Cursor::new(vec![2u8; 255 * 4096 - 7]), false).unwrap();
    }
    check_consistent(&img.path);
    same(&ntfscat(&img.path, "/pasted/renamed.bin"), vec![0x5Au8; 700_000], "171-cluster file");
    same(&ntfscat(&img.path, "/pasted/128.bin"), vec![1u8; 128 * 4096], "128-cluster file");
    same(&ntfscat(&img.path, "/pasted/255.bin"), vec![2u8; 255 * 4096 - 7], "255-cluster file");
}
