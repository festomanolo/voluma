//! MBR and GPT parsing. Parsing is strictly read-only and never trusts
//! on-disk values: every offset/length is validated against device size.

use block_device::{BlockDevice, BlockError};
use serde::Serialize;

pub const SECTOR: u64 = 512;

#[derive(Debug, thiserror::Error)]
pub enum PartitionError {
    #[error(transparent)]
    Block(#[from] BlockError),
    #[error("invalid partition table: {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind { None, Mbr, Gpt }

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Partition {
    pub index: u32,
    pub offset: u64,
    pub length: u64,
    /// MBR type byte as hex, or GPT type GUID.
    pub type_id: String,
    pub type_name: Option<&'static str>,
    pub guid: Option<String>,
    pub name: Option<String>,
    pub is_efi_system: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PartitionTable {
    pub kind: TableKind,
    pub disk_guid: Option<String>,
    pub partitions: Vec<Partition>,
}

pub fn guid_to_string(b: &[u8]) -> String {
    let d1 = u32::from_le_bytes(b[0..4].try_into().unwrap());
    let d2 = u16::from_le_bytes(b[4..6].try_into().unwrap());
    let d3 = u16::from_le_bytes(b[6..8].try_into().unwrap());
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{}",
        d1, d2, d3, b[8], b[9],
        b[10..16].iter().map(|x| format!("{:02X}", x)).collect::<String>()
    )
}

const EFI_GUID: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";

fn gpt_type_name(g: &str) -> Option<&'static str> {
    match g {
        EFI_GUID => Some("EFI System"),
        "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7" => Some("Microsoft Basic Data"),
        "0FC63DAF-8483-4772-8E79-3D69D8477DE4" => Some("Linux Filesystem"),
        "48465300-0000-11AA-AA11-00306543ECAC" => Some("Apple HFS+"),
        "7C3457EF-0000-11AA-AA11-00306543ECAC" => Some("Apple APFS"),
        "E3C9E316-0B5C-4DB8-817D-F92DF00215AE" => Some("Microsoft Reserved"),
        _ => None,
    }
}

fn mbr_type_name(t: u8) -> Option<&'static str> {
    match t {
        0x01 => Some("FAT12"), 0x04 | 0x06 | 0x0E => Some("FAT16"),
        0x0B | 0x0C => Some("FAT32"), 0x07 => Some("NTFS/exFAT"),
        0x83 => Some("Linux"), 0xAF => Some("Apple HFS+"), 0xEE => Some("GPT Protective"),
        0xEF => Some("EFI System"), 0x05 | 0x0F => Some("Extended"),
        _ => None,
    }
}

pub fn read_partition_table(dev: &dyn BlockDevice) -> Result<PartitionTable, PartitionError> {
    let size = dev.size();
    if size < SECTOR {
        return Ok(PartitionTable { kind: TableKind::None, disk_guid: None, partitions: vec![] });
    }
    let mut mbr = [0u8; 512];
    dev.read_at(0, &mut mbr)?;
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Ok(PartitionTable { kind: TableKind::None, disk_guid: None, partitions: vec![] });
    }
    let protective = (0..4).any(|i| mbr[446 + i * 16 + 4] == 0xEE);
    if protective {
        return read_gpt(dev, size);
    }
    // A FAT boot sector also ends in 55AA; reject if it looks like a VBR.
    if looks_like_vbr(&mbr) {
        return Ok(PartitionTable { kind: TableKind::None, disk_guid: None, partitions: vec![] });
    }
    let mut parts = Vec::new();
    for i in 0..4 {
        let e = &mbr[446 + i * 16..446 + (i + 1) * 16];
        let t = e[4];
        let lba = u32::from_le_bytes(e[8..12].try_into().unwrap()) as u64;
        let n = u32::from_le_bytes(e[12..16].try_into().unwrap()) as u64;
        if t == 0 || n == 0 { continue; }
        let (offset, length) = (lba * SECTOR, n * SECTOR);
        if offset.checked_add(length).map_or(true, |end| end > size) {
            return Err(PartitionError::Invalid("MBR partition exceeds device size"));
        }
        parts.push(Partition {
            index: i as u32 + 1, offset, length,
            type_id: format!("{:02X}", t), type_name: mbr_type_name(t),
            guid: None, name: None, is_efi_system: t == 0xEF,
        });
    }
    if parts.is_empty() {
        return Ok(PartitionTable { kind: TableKind::None, disk_guid: None, partitions: vec![] });
    }
    Ok(PartitionTable { kind: TableKind::Mbr, disk_guid: None, partitions: parts })
}

fn looks_like_vbr(s: &[u8; 512]) -> bool {
    let jmp = s[0] == 0xEB || s[0] == 0xE9;
    let bps = u16::from_le_bytes([s[11], s[12]]);
    jmp && matches!(bps, 512 | 1024 | 2048 | 4096) && s[13].is_power_of_two()
        || &s[3..11] == b"EXFAT   " || &s[3..11] == b"NTFS    "
}

fn read_gpt(dev: &dyn BlockDevice, size: u64) -> Result<PartitionTable, PartitionError> {
    let mut h = [0u8; 92];
    dev.read_at(SECTOR, &mut h)?;
    if &h[0..8] != b"EFI PART" { return Err(PartitionError::Invalid("missing GPT signature")); }
    let disk_guid = guid_to_string(&h[56..72]);
    let entries_lba = u64::from_le_bytes(h[72..80].try_into().unwrap());
    let count = u32::from_le_bytes(h[80..84].try_into().unwrap()) as u64;
    let esize = u32::from_le_bytes(h[84..88].try_into().unwrap()) as u64;
    if esize < 128 || esize > 4096 || count > 4096 {
        return Err(PartitionError::Invalid("implausible GPT entry geometry"));
    }
    let total = count * esize;
    let start = entries_lba.checked_mul(SECTOR).ok_or(PartitionError::Invalid("GPT overflow"))?;
    if start.checked_add(total).map_or(true, |e| e > size) {
        return Err(PartitionError::Invalid("GPT entry array exceeds device"));
    }
    let mut buf = vec![0u8; total as usize];
    dev.read_at(start, &mut buf)?;
    let mut parts = Vec::new();
    for i in 0..count as usize {
        let e = &buf[i * esize as usize..(i + 1) * esize as usize];
        if e[0..16].iter().all(|&b| b == 0) { continue; }
        let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
        let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
        if last < first { return Err(PartitionError::Invalid("GPT entry end before start")); }
        let offset = first.checked_mul(SECTOR).ok_or(PartitionError::Invalid("GPT overflow"))?;
        let length = (last - first + 1).checked_mul(SECTOR).ok_or(PartitionError::Invalid("GPT overflow"))?;
        if offset.checked_add(length).map_or(true, |end| end > size) {
            return Err(PartitionError::Invalid("GPT partition exceeds device size"));
        }
        let ty = guid_to_string(&e[0..16]);
        let name: Vec<u16> = e[56..128].chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        parts.push(Partition {
            index: i as u32 + 1, offset, length,
            type_name: gpt_type_name(&ty), is_efi_system: ty == EFI_GUID,
            type_id: ty, guid: Some(guid_to_string(&e[16..32])),
            name: Some(String::from_utf16_lossy(&name)),
        });
    }
    Ok(PartitionTable { kind: TableKind::Gpt, disk_guid: Some(disk_guid), partitions: parts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use block_device::ImageDevice;
    use std::io::Write;

    fn dev_from(bytes: Vec<u8>) -> (tempfile::NamedTempFile, ImageDevice) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&bytes).unwrap();
        let d = ImageDevice::open_read_only(f.path()).unwrap();
        (f, d)
    }

    #[test]
    fn empty_disk_has_no_table() {
        let (_f, d) = dev_from(vec![0; 1 << 20]);
        assert_eq!(read_partition_table(&d).unwrap().kind, TableKind::None);
    }

    #[test]
    fn parses_mbr() {
        let mut img = vec![0u8; 4 << 20];
        let e = 446;
        img[e + 4] = 0x83;
        img[e + 8..e + 12].copy_from_slice(&2048u32.to_le_bytes());
        img[e + 12..e + 16].copy_from_slice(&1024u32.to_le_bytes());
        img[510] = 0x55; img[511] = 0xAA;
        let (_f, d) = dev_from(img);
        let t = read_partition_table(&d).unwrap();
        assert_eq!(t.kind, TableKind::Mbr);
        assert_eq!(t.partitions[0].offset, 2048 * 512);
        assert_eq!(t.partitions[0].type_name, Some("Linux"));
    }

    #[test]
    fn rejects_mbr_partition_past_end() {
        let mut img = vec![0u8; 1 << 20];
        img[446 + 4] = 0x83;
        img[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
        img[446 + 12..446 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        img[510] = 0x55; img[511] = 0xAA;
        let (_f, d) = dev_from(img);
        assert!(read_partition_table(&d).is_err());
    }

    fn gpt_image() -> Vec<u8> {
        let mut img = vec![0u8; 4 << 20];
        img[446 + 4] = 0xEE; img[510] = 0x55; img[511] = 0xAA;
        let h = 512;
        img[h..h + 8].copy_from_slice(b"EFI PART");
        img[h + 72..h + 80].copy_from_slice(&2u64.to_le_bytes());
        img[h + 80..h + 84].copy_from_slice(&128u32.to_le_bytes());
        img[h + 84..h + 88].copy_from_slice(&128u32.to_le_bytes());
        let e = 1024;
        // EFI System GUID, mixed endian
        img[e..e + 16].copy_from_slice(&[0x28,0x73,0x2A,0xC1,0x1F,0xF8,0xD2,0x11,0xBA,0x4B,0x00,0xA0,0xC9,0x3E,0xC9,0x3B]);
        img[e + 16] = 1;
        img[e + 32..e + 40].copy_from_slice(&2048u64.to_le_bytes());
        img[e + 40..e + 48].copy_from_slice(&4095u64.to_le_bytes());
        for (i, c) in "ESP".encode_utf16().enumerate() {
            img[e + 56 + i * 2..e + 58 + i * 2].copy_from_slice(&c.to_le_bytes());
        }
        img
    }

    #[test]
    fn parses_gpt_efi() {
        let (_f, d) = dev_from(gpt_image());
        let t = read_partition_table(&d).unwrap();
        assert_eq!(t.kind, TableKind::Gpt);
        let p = &t.partitions[0];
        assert!(p.is_efi_system);
        assert_eq!(p.name.as_deref(), Some("ESP"));
        assert_eq!(p.offset, 2048 * 512);
        assert_eq!(p.length, 2048 * 512);
    }

    #[test]
    fn rejects_malicious_gpt_geometry() {
        let mut img = gpt_image();
        img[512 + 84..512 + 88].copy_from_slice(&u32::MAX.to_le_bytes());
        let (_f, d) = dev_from(img);
        assert!(read_partition_table(&d).is_err());
    }
}
