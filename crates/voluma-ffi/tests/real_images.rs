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
