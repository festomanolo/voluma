//! C ABI consumed by the SwiftUI app. Only JSON strings cross the boundary;
//! the frontend never receives a raw device handle. Images open read-only.

use block_device::{BlockDevice, ImageDevice, PartitionWindow};
use serde::Serialize;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_reports_error_json() {
        let v: serde_json::Value = serde_json::from_str(&inspect_image_json("/nonexistent/x.img")).unwrap();
        assert!(v.get("error").is_some());
    }
}
