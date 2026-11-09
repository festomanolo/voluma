//! Detection + browsing against an OS-generated FAT32 image.
//! On macOS a generator failure panics (no silent skips).
use std::process::Command;

fn fixture() -> Option<(tempfile::TempDir, String)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: needs macOS hdiutil"); return None; }
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("fat32.img");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/make_fat_fixture.sh");
    let o = Command::new("sh").args([script, img.to_str().unwrap(), "MS-DOS FAT32", "64"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let p = img.to_str().unwrap().to_string();
    Some((d, p))
}

fn json(s: String) -> serde_json::Value { serde_json::from_str(&s).unwrap() }

#[test]
fn detects_fat32_generated_by_macos() {
    let Some((_d, img)) = fixture() else { return };
    let v = json(voluma_ffi::inspect_image_json(&img));
    let fs = &v["ok"]["whole_disk_filesystem"];
    assert_eq!(fs["kind"], "fat32", "{v}");
    assert_eq!(fs["label"], "FIXTURE", "{v}");
}

#[test]
fn list_and_export_roundtrip() {
    let Some((d, img)) = fixture() else { return };
    let v = json(voluma_ffi::list_directory_json(&img, -1, "/docs"));
    assert_eq!(v["ok"]["driver_supports_write"], true, "{v}");
    assert_eq!(v["ok"]["capabilities"]["write"], false, "listing itself must be read-only: {v}");
    assert!(v["ok"]["entries"].as_array().unwrap().iter().any(|e| e["name"] == "nested"), "{v}");

    let dest = d.path().join("out.bin");
    let v = json(voluma_ffi::export_file_json(&img, -1, "/big.bin", dest.to_str().unwrap()));
    assert_eq!(v["ok"]["bytes"], 3 * 1024 * 1024, "{v}");
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), 3 * 1024 * 1024);

    // Never silently overwrite.
    let again = json(voluma_ffi::export_file_json(&img, -1, "/big.bin", dest.to_str().unwrap()));
    assert!(again.get("error").is_some(), "{again}");
    // Directories can't be exported as files.
    let dir = json(voluma_ffi::export_file_json(&img, -1, "/docs", d.path().join("x").to_str().unwrap()));
    assert!(dir.get("error").is_some());
}

#[test]
fn mutate_requires_driver_support_and_roundtrips() {
    let Some((d, img)) = fixture() else { return };
    let src = d.path().join("src.txt");
    std::fs::write(&src, b"imported via ffi").unwrap();
    let s = src.to_str().unwrap();
    assert_eq!(json(voluma_ffi::mutate_json(&img, -1, "mkdir", "/made", "", false))["ok"]["done"], "mkdir");
    assert_eq!(json(voluma_ffi::mutate_json(&img, -1, "import", "/made/in.txt", s, false))["ok"]["bytes"], 16);
    assert!(json(voluma_ffi::mutate_json(&img, -1, "import", "/made/in.txt", s, false)).get("error").is_some(), "no silent overwrite");
    assert!(json(voluma_ffi::mutate_json(&img, -1, "rename", "/made/in.txt", "/made/out.txt", false)).get("ok").is_some());
    let l = json(voluma_ffi::list_directory_json(&img, -1, "/made"));
    assert_eq!(l["ok"]["entries"][0]["name"], "out.txt", "{l}");
    assert!(json(voluma_ffi::mutate_json(&img, -1, "delete", "/made", "", false)).get("error").is_some(), "non-empty dir");
    assert!(json(voluma_ffi::mutate_json(&img, -1, "delete", "/made/out.txt", "", false)).get("ok").is_some());
    assert!(json(voluma_ffi::mutate_json(&img, -1, "bogus", "/", "", false)).get("error").is_some());
}

fn mac_fixture(fs: &str, mb: &str) -> Option<(tempfile::TempDir, String)> {
    if !cfg!(target_os = "macos") { eprintln!("SKIPPED: needs macOS hdiutil"); return None; }
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("t.img");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/make_mac_fixture.sh");
    let o = Command::new("sh").args([script, img.to_str().unwrap(), fs, mb]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let p = img.to_str().unwrap().to_string();
    Some((d, p))
}

#[test]
fn browse_and_export_hfsplus_and_apfs_read_only() {
    for (fs, kind, name) in [("HFS+", "hfs_plus", "HFS+"), ("APFS", "apfs", "APFS")] {
        let Some((d, img)) = mac_fixture(fs, "64") else { return };
        let v = json(voluma_ffi::inspect_image_json(&img));
        assert_eq!(v["ok"]["whole_disk_filesystem"]["kind"], kind, "{v}");
        let l = json(voluma_ffi::list_directory_json(&img, -1, "/docs"));
        assert_eq!(l["ok"]["filesystem"], name, "{l}");
        assert_eq!(l["ok"]["driver_supports_write"], false, "{l}");
        assert!(l["ok"]["entries"].as_array().unwrap().iter().any(|e| e["name"] == "nested"), "{l}");
        let dest = d.path().join("big.out");
        let e = json(voluma_ffi::export_file_json(&img, -1, "/big.bin", dest.to_str().unwrap()));
        assert_eq!(e["ok"]["bytes"], 3 * 1024 * 1024, "{e}");
        // Writes are refused before the image is even opened writable by a driver.
        let m = json(voluma_ffi::mutate_json(&img, -1, "mkdir", "/x", "", false));
        assert!(m["error"].as_str().unwrap().contains("read-only"), "{m}");
    }
}

fn ntfs_tool(name: &str) -> Option<std::path::PathBuf> {
    let d = std::env::var("VOLUMA_NTFS3G").ok()?;
    ["", "bin", "sbin"].iter().map(|s| std::path::Path::new(&d).join(s).join(name)).find(|p| p.is_file())
}

#[test]
fn ntfs_browse_export_and_mutate_through_ffi() {
    let (Some(mkntfs), Some(pop), Some(resize)) = (ntfs_tool("mkntfs"), ntfs_tool("ntfs_populate"), ntfs_tool("ntfsresize")) else {
        eprintln!("SKIPPED: ntfs-3g tools not found (set VOLUMA_NTFS3G)"); return;
    };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("README.TXT"), b"hello voluma\n").unwrap();
    std::fs::write(src.join("docs/a.txt"), b"a").unwrap();
    std::fs::write(src.join("big.bin"), vec![0xABu8; 1_500_000]).unwrap();
    let img = d.path().join("n.img");
    std::fs::File::create(&img).unwrap().set_len(32 << 20).unwrap();
    assert!(Command::new(mkntfs).args(["-F", "-Q", "-L", "NTFSVOL"]).arg(&img).output().unwrap().status.success());
    assert!(Command::new(pop).arg(&img).arg(&src).output().unwrap().status.success());
    let i = img.to_str().unwrap();

    let v = json(voluma_ffi::inspect_image_json(i));
    assert_eq!(v["ok"]["whole_disk_filesystem"]["kind"], "ntfs", "{v}");
    let l = json(voluma_ffi::list_directory_json(i, -1, "/"));
    assert_eq!(l["ok"]["filesystem"], "NTFS", "{l}");
    assert_eq!(l["ok"]["driver_supports_write"], true, "{l}");
    assert!(l["ok"]["write_blocked"].is_null(), "clean volume: {l}");
    assert_eq!(l["ok"]["capabilities"]["write"], false, "listing itself stays read-only: {l}");

    let dest = d.path().join("big.out");
    let e = json(voluma_ffi::export_file_json(i, -1, "/big.bin", dest.to_str().unwrap()));
    assert_eq!(e["ok"]["bytes"], 1_500_000, "{e}");
    assert_eq!(std::fs::read(&dest).unwrap(), vec![0xABu8; 1_500_000]);

    let f = d.path().join("in.txt");
    std::fs::write(&f, b"imported via ffi into ntfs").unwrap();
    assert_eq!(json(voluma_ffi::mutate_json(i, -1, "mkdir", "/made", "", false))["ok"]["done"], "mkdir");
    assert_eq!(json(voluma_ffi::mutate_json(i, -1, "import", "/made/in.txt", f.to_str().unwrap(), false))["ok"]["bytes"], 26);
    assert!(json(voluma_ffi::mutate_json(i, -1, "import", "/made/in.txt", f.to_str().unwrap(), false)).get("error").is_some(), "no silent overwrite");
    assert!(json(voluma_ffi::mutate_json(i, -1, "rename", "/made/in.txt", "/made/out.txt", false)).get("ok").is_some());
    assert!(json(voluma_ffi::mutate_json(i, -1, "delete", "/made", "", false)).get("error").is_some(), "non-empty dir");
    let cat = Command::new(ntfs_tool("ntfscat").unwrap()).arg("-f").arg(&img).arg("/made/out.txt").output().unwrap();
    assert_eq!(cat.stdout, b"imported via ffi into ntfs");
    assert!(json(voluma_ffi::mutate_json(i, -1, "delete", "/made/out.txt", "", false)).get("ok").is_some());
    assert!(json(voluma_ffi::mutate_json(i, -1, "delete", "/made", "", false)).get("ok").is_some());
    let o = Command::new(resize).args(["-i", "-f"]).arg(&img).output().unwrap();
    assert!(o.status.success(), "ntfsresize: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}
