//! C ABI consumed by the SwiftUI app. Only JSON strings cross the boundary;
//! the frontend never receives a raw device handle. Images open read-only.

use block_device::{BlockDevice, ImageDevice, PartitionWindow};
use filesystem_detection::FsKind;
use fs_core::{EntryKind, Filesystem};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;

#[derive(Serialize)]
struct PartitionReport {
    #[serde(flatten)]
    partition: partition_manager::Partition,
    filesystem: filesystem_detection::Detection,
}

#[derive(Serialize)]
struct ImageReport {
    path: String,
    size: u64,
    read_only: bool,
    table: partition_manager::PartitionTable,
    /// Filesystem detected directly at offset 0 (superfloppy/no partition table).
    whole_disk_filesystem: Option<filesystem_detection::Detection>,
    partitions: Vec<PartitionReport>,
}

pub fn inspect_image_json(path: &str) -> String {
    match inspect(path) {
        Ok(r) => serde_json::to_string(&serde_json::json!({ "ok": r })).unwrap(),
        Err(e) => serde_json::to_string(&serde_json::json!({ "error": e })).unwrap(),
    }
}

fn inspect(path: &str) -> Result<ImageReport, String> {
    let dev: Arc<dyn BlockDevice> = Arc::new(ImageDevice::open_read_only(path).map_err(|e| e.to_string())?);
    let table = partition_manager::read_partition_table(dev.as_ref()).map_err(|e| e.to_string())?;
    let mut partitions = Vec::new();
    for p in &table.partitions {
        let w = PartitionWindow::new(dev.clone(), p.offset, p.length).map_err(|e| e.to_string())?;
        partitions.push(PartitionReport { partition: p.clone(), filesystem: filesystem_detection::detect(&w) });
    }
    let whole = (partitions.is_empty()).then(|| filesystem_detection::detect(dev.as_ref()));
    Ok(ImageReport { path: path.into(), size: dev.size(), read_only: true, table, whole_disk_filesystem: whole, partitions })
}

/// # Safety
/// `path` must be a valid NUL-terminated UTF-8 string. Free the result with `voluma_free_string`.
#[no_mangle]
pub unsafe extern "C" fn voluma_inspect_image(path: *const c_char) -> *mut c_char {
    let out = if path.is_null() {
        r#"{"error":"null path"}"#.to_string()
    } else {
        match CStr::from_ptr(path).to_str() {
            Ok(p) => inspect_image_json(p),
            Err(_) => r#"{"error":"path is not valid UTF-8"}"#.to_string(),
        }
    };
    CString::new(out).unwrap().into_raw()
}

/// # Safety
/// `s` must come from a `voluma_*` function and be freed at most once.
#[no_mangle]
pub unsafe extern "C" fn voluma_free_string(s: *mut c_char) {
    if !s.is_null() { drop(CString::from_raw(s)); }
}

/// Writes are never implicit: `writable` must be requested by the caller for this call only,
/// and the drivers additionally refuse dirty volumes.
fn open_fs(image: &str, part: i32, writable: bool) -> Result<Box<dyn Filesystem>, String> {
    let dev: Arc<dyn BlockDevice> = Arc::new(if writable { ImageDevice::open_read_write(image) } else { ImageDevice::open_read_only(image) }
        .map_err(|e| e.to_string())?);
    let vol: Arc<dyn BlockDevice> = if part < 0 { dev } else {
        let t = partition_manager::read_partition_table(dev.as_ref()).map_err(|e| e.to_string())?;
        let p = t.partitions.iter().find(|p| p.index as i32 == part).ok_or("no such partition")?;
        Arc::new(PartitionWindow::new(dev, p.offset, p.length).map_err(|e| e.to_string())?)
    };
    let d = filesystem_detection::detect(vol.as_ref());
    match d.kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 =>
            Ok(Box::new(fs_fat::FatFs::open(vol).map_err(|e| e.to_string())?)),
        FsKind::ExFat => Ok(Box::new(fs_exfat::ExFatFs::open(vol).map_err(|e| e.to_string())?)),
        // Read-only drivers: refuse up front instead of letting a write attempt reach them.
        FsKind::HfsPlus | FsKind::Ext | FsKind::Apfs if writable =>
            Err(format!("The {:?} driver is read-only; writing is not supported", d.kind)),
        FsKind::HfsPlus => Ok(Box::new(fs_hfsplus::HfsPlusFs::open(vol).map_err(|e| e.to_string())?)),
        FsKind::Ext => Ok(Box::new(fs_ext::ExtFs::open(vol).map_err(|e| e.to_string())?)),
        FsKind::Apfs => Ok(Box::new(fs_apfs::ApfsFs::open(vol).map_err(|e| e.to_string())?)),
        FsKind::Ntfs => Ok(Box::new(fs_ntfs::NtfsFs::open(vol).map_err(|e| e.to_string())?)),
        k => Err(format!("No driver yet for {k:?}; this filesystem is detected but cannot be browsed")),
    }
}

fn wrap<T: Serialize>(r: Result<T, String>) -> String {
    match r {
        Ok(v) => serde_json::to_string(&serde_json::json!({ "ok": v })).unwrap(),
        Err(e) => serde_json::to_string(&serde_json::json!({ "error": e })).unwrap(),
    }
}

/// `part < 0` browses the whole image (no partition table).
pub fn list_directory_json(image: &str, part: i32, path: &str) -> String {
    wrap((|| {
        let fs = open_fs(image, part, false)?;
        let mut entries = fs.read_dir(path).map_err(|e| e.to_string())?;
        entries.sort_by(|a, b| (a.kind != EntryKind::Directory, a.name.to_lowercase())
            .cmp(&(b.kind != EntryKind::Directory, b.name.to_lowercase())));
        Ok(serde_json::json!({
            "filesystem": fs.name(), "label": fs.label(),
            "capabilities": fs.capabilities(), "entries": entries,
            // Whether the driver CAN write (the listing itself is opened read-only).
            "driver_supports_write": matches!(fs.name(), "FAT12" | "FAT16" | "FAT32" | "exFAT" | "NTFS"),
            // Set when the volume's own state (dirty, hibernated, unclean log) forbids writing.
            "write_blocked": fs.write_blocked(),
        }))
    })())
}

/// Streams a file out of the image into a NEW host file (never overwrites).
pub fn export_file_json(image: &str, part: i32, fs_path: &str, dest: &str) -> String {
    wrap((|| {
        let fs = open_fs(image, part, false)?;
        let e = fs.stat(fs_path).map_err(|e| e.to_string())?;
        if e.kind == EntryKind::Directory { return Err("cannot export a directory as a file".to_string()); }
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(dest)
            .map_err(|e| format!("cannot create destination: {e}"))?;
        let mut buf = vec![0u8; 1 << 20];
        let (mut off, mut h) = (0u64, Sha256::new());
        let res: Result<(), String> = (|| loop {
            let n = fs.read_at(&e, off, &mut buf).map_err(|e| e.to_string())?;
            if n == 0 { return Ok(()); }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            h.update(&buf[..n]); off += n as u64;
        })();
        if let Err(msg) = res { drop(out); let _ = std::fs::remove_file(dest); return Err(msg); }
        out.sync_all().map_err(|e| e.to_string())?;
        if off != e.size { let _ = std::fs::remove_file(dest); return Err("short read".into()); }
        Ok(serde_json::json!({ "bytes": off, "sha256": h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>() }))
    })())
}

/// One mutation, always on an explicitly writable open. `op`: "mkdir" | "delete" | "rename" | "import".
/// `a` is the target path; `b` is the new path (rename) or the host source file (import).
pub fn mutate_json(image: &str, part: i32, op: &str, a: &str, b: &str, overwrite: bool) -> String {
    wrap((|| {
        let fs = open_fs(image, part, true)?;
        if let Some(why) = fs.write_blocked() { return Err(format!("this volume cannot be written safely: {why}")); }
        if !fs.capabilities().write { return Err("this filesystem driver is read-only".to_string()); }
        let m = |e: fs_core::FsError| e.to_string();
        match op {
            "mkdir" => { fs.create_dir(a).map_err(m)?; Ok(serde_json::json!({ "done": op })) }
            "delete" => { fs.remove(a).map_err(m)?; Ok(serde_json::json!({ "done": op })) }
            "rename" => { fs.rename(a, b).map_err(m)?; Ok(serde_json::json!({ "done": op })) }
            "import" => {
                let mut f = std::fs::File::open(b).map_err(|e| format!("cannot open source: {e}"))?;
                let e = fs.write_file(a, &mut f, overwrite).map_err(m)?;
                Ok(serde_json::json!({ "done": op, "bytes": e.size }))
            }
            _ => Err(format!("unknown operation {op}")),
        }
    })())
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> { if p.is_null() { None } else { CStr::from_ptr(p).to_str().ok() } }
fn out(s: String) -> *mut c_char { CString::new(s).unwrap().into_raw() }
const BAD_ARG: &str = r#"{"error":"invalid argument"}"#;

/// # Safety
/// All pointers must be valid NUL-terminated UTF-8 strings. Free result with `voluma_free_string`.
#[no_mangle]
pub unsafe extern "C" fn voluma_list_directory(image: *const c_char, part: i32, path: *const c_char) -> *mut c_char {
    match (cstr(image), cstr(path)) { (Some(i), Some(p)) => out(list_directory_json(i, part, p)), _ => out(BAD_ARG.into()) }
}

/// # Safety
/// As `voluma_list_directory`. Opens the image read-write for this single call.
#[no_mangle]
pub unsafe extern "C" fn voluma_mutate(image: *const c_char, part: i32, op: *const c_char, a: *const c_char, b: *const c_char, overwrite: i32) -> *mut c_char {
    match (cstr(image), cstr(op), cstr(a), cstr(b)) {
        (Some(i), Some(o), Some(a), Some(b)) => out(mutate_json(i, part, o, a, b, overwrite != 0)),
        _ => out(BAD_ARG.into()),
    }
}

/// # Safety
/// As `voluma_list_directory`.
#[no_mangle]
pub unsafe extern "C" fn voluma_export_file(image: *const c_char, part: i32, fs_path: *const c_char, dest: *const c_char) -> *mut c_char {
    match (cstr(image), cstr(fs_path), cstr(dest)) {
        (Some(i), Some(p), Some(d)) => out(export_file_json(i, part, p, d)),
        _ => out(BAD_ARG.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_reports_error_json() {
        let v: serde_json::Value = serde_json::from_str(&inspect_image_json("/nonexistent/x.img")).unwrap();
        assert!(v.get("error").is_some());
    }
}
