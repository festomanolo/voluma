//! Signature-based filesystem identification. Never uses names or paths.
//! Detection only; says nothing about whether a driver can read or write it.

use block_device::BlockDevice;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FsKind { Fat12, Fat16, Fat32, ExFat, Ntfs, Ext, Apfs, HfsPlus, Xfs, Btrfs, Iso9660, Unknown }

#[derive(Debug, Clone, Serialize)]
pub struct Detection {
    pub kind: FsKind,
    pub label: Option<String>,
    /// Free-form detail, e.g. "ext4" / "ext3" for the ext family.
    pub detail: Option<String>,
}

fn unknown() -> Detection { Detection { kind: FsKind::Unknown, label: None, detail: None } }

fn trim_label(b: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(b);
    let s = s.trim_matches(|c: char| c == '\0' || c == ' ').to_string();
    if s.is_empty() || s == "NO NAME" { None } else { Some(s) }
}

pub fn detect(dev: &dyn BlockDevice) -> Detection {
    let size = dev.size();
    let mut s0 = [0u8; 512];
    if size < 512 || dev.read_at(0, &mut s0).is_err() { return unknown(); }

    if &s0[3..11] == b"NTFS    " && s0[510] == 0x55 && s0[511] == 0xAA {
        return Detection { kind: FsKind::Ntfs, label: None, detail: None };
    }
    if &s0[3..11] == b"EXFAT   " && s0[510] == 0x55 && s0[511] == 0xAA {
        return Detection { kind: FsKind::ExFat, label: None, detail: None };
    }
    if &s0[32..36] == b"NXSB" {
        return Detection { kind: FsKind::Apfs, label: None, detail: Some("container".into()) };
    }
    if &s0[0..4] == b"XFSB" {
        return Detection { kind: FsKind::Xfs, label: None, detail: None };
    }
    if let Some(d) = detect_fat(&s0) { return d; }

    let mut b = [0u8; 2048];
    if size >= 1024 + 2048 && dev.read_at(1024, &mut b).is_ok() {
        // HFS+ / HFSX volume header at 1024
        if &b[0..2] == b"H+" || &b[0..2] == b"HX" {
            return Detection { kind: FsKind::HfsPlus, label: None, detail: None };
        }
        // ext superblock at 1024, magic at +56
        if b[56] == 0x53 && b[57] == 0xEF {
            let compat = u32::from_le_bytes(b[92..96].try_into().unwrap());
            let incompat = u32::from_le_bytes(b[96..100].try_into().unwrap());
            let ro = u32::from_le_bytes(b[100..104].try_into().unwrap());
            let detail = if incompat & 0x40 != 0 || ro & 0x40 != 0 || incompat & 0x200 != 0 { "ext4" }
                else if compat & 0x4 != 0 { "ext3" } else { "ext2" };
            return Detection { kind: FsKind::Ext, label: trim_label(&b[120..136]), detail: Some(detail.into()) };
        }
    }
    let mut m = [0u8; 8];
    if size >= 0x10040 + 8 && dev.read_at(0x10040, &mut m).is_ok() && &m == b"_BHRfS_M" {
        return Detection { kind: FsKind::Btrfs, label: None, detail: None };
    }
    let mut iso = [0u8; 6];
    if size >= 0x8000 + 6 && dev.read_at(0x8000, &mut iso).is_ok() && &iso[1..6] == b"CD001" {
        return Detection { kind: FsKind::Iso9660, label: None, detail: None };
    }
    unknown()
}

fn detect_fat(s: &[u8; 512]) -> Option<Detection> {
    if s[510] != 0x55 || s[511] != 0xAA { return None; }
    let bps = u16::from_le_bytes([s[11], s[12]]) as u32;
    let spc = s[13] as u32;
    let reserved = u16::from_le_bytes([s[14], s[15]]) as u32;
    let nfats = s[16] as u32;
    if !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two()
        || reserved == 0 || nfats == 0 || nfats > 2 { return None; }
    let root_entries = u16::from_le_bytes([s[17], s[18]]) as u32;
    let total16 = u16::from_le_bytes([s[19], s[20]]) as u32;
    let total32 = u32::from_le_bytes(s[32..36].try_into().unwrap());
    let fat16 = u16::from_le_bytes([s[22], s[23]]) as u32;
    let fat32 = u32::from_le_bytes(s[36..40].try_into().unwrap());
    let fat_sz = if fat16 != 0 { fat16 } else { fat32 };
    let total = if total16 != 0 { total16 } else { total32 };
    if fat_sz == 0 || total == 0 { return None; }
    let root_secs = (root_entries * 32 + bps - 1) / bps;
    let data = total.checked_sub(reserved + nfats * fat_sz + root_secs)?;
    let clusters = data / spc;
    // Spec-defined cluster-count thresholds (never use the type string).
    let (kind, lbl) = if clusters < 4085 { (FsKind::Fat12, &s[43..54]) }
        else if clusters < 65525 { (FsKind::Fat16, &s[43..54]) }
        else { (FsKind::Fat32, &s[71..82]) };
    Some(Detection { kind, label: trim_label(lbl), detail: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use block_device::ImageDevice;
    use std::io::Write;

    fn det(img: Vec<u8>) -> Detection {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&img).unwrap();
        detect(&ImageDevice::open_read_only(f.path()).unwrap())
    }

    #[test]
    fn zeros_are_unknown() { assert_eq!(det(vec![0; 1 << 20]).kind, FsKind::Unknown); }

    #[test]
    fn detects_ntfs() {
        let mut i = vec![0u8; 1 << 20];
        i[3..11].copy_from_slice(b"NTFS    "); i[510] = 0x55; i[511] = 0xAA;
        assert_eq!(det(i).kind, FsKind::Ntfs);
    }

    #[test]
    fn detects_ext4() {
        let mut i = vec![0u8; 1 << 20];
        i[1024 + 56] = 0x53; i[1024 + 57] = 0xEF;
        i[1024 + 96..1024 + 100].copy_from_slice(&0x40u32.to_le_bytes());
        i[1024 + 120..1024 + 124].copy_from_slice(b"data");
        let d = det(i);
        assert_eq!(d.kind, FsKind::Ext);
        assert_eq!(d.detail.as_deref(), Some("ext4"));
        assert_eq!(d.label.as_deref(), Some("data"));
    }

    #[test]
    fn detects_fat32_by_cluster_count() {
        let mut i = vec![0u8; 1 << 20];
        i[11..13].copy_from_slice(&512u16.to_le_bytes());
        i[13] = 1; i[14..16].copy_from_slice(&32u16.to_le_bytes()); i[16] = 2;
        i[32..36].copy_from_slice(&200_000u32.to_le_bytes());
        i[36..40].copy_from_slice(&1600u32.to_le_bytes());
        i[71..82].copy_from_slice(b"USB STICK  ");
        i[510] = 0x55; i[511] = 0xAA;
        let d = det(i);
        assert_eq!(d.kind, FsKind::Fat32);
        assert_eq!(d.label.as_deref(), Some("USB STICK"));
    }

    #[test]
    fn fat_type_string_is_not_trusted() {
        let mut i = vec![0u8; 1 << 20];
        i[82..90].copy_from_slice(b"FAT32   "); i[510] = 0x55; i[511] = 0xAA;
        assert_eq!(det(i).kind, FsKind::Unknown);
    }
}
