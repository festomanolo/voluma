//! Driver micro-benchmarks, run by tests/bench/run.sh against throwaway images.
//!   bench read  IMG KIND PATH           stream a file out in 1 MiB reads, print MB/s
//!   bench write IMG KIND PATH SRC       stream a host file into the volume, print MB/s
//!   bench many  IMG KIND N              create N small files in /bench, print files/s
//! KIND: fat | exfat | hfs | apfs | ext | ntfs. Output is one JSON object per run.
use block_device::{BlockDevice, ImageDevice};
use fs_core::Filesystem;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Instant;

fn open(img: &str, kind: &str, rw: bool) -> Box<dyn Filesystem> {
    let d: Arc<dyn BlockDevice> = Arc::new(if rw { ImageDevice::open_read_write(img) } else { ImageDevice::open_read_only(img) }.unwrap());
    match kind {
        "fat" => Box::new(fs_fat::FatFs::open(d).unwrap()),
        "exfat" => Box::new(fs_exfat::ExFatFs::open(d).unwrap()),
        "hfs" => Box::new(fs_hfsplus::HfsPlusFs::open(d).unwrap()),
        "apfs" => Box::new(fs_apfs::ApfsFs::open(d).unwrap()),
        "ext" => Box::new(fs_ext::ExtFs::open(d).unwrap()),
        "ntfs" => Box::new(fs_ntfs::NtfsFs::open(d).unwrap()),
        k => panic!("unknown kind {k}"),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (mode, img, kind) = (a[1].as_str(), a[2].as_str(), a[3].as_str());
    match mode {
        "read" => {
            let fs = open(img, kind, false);
            let e = fs.stat(&a[4]).unwrap();
            let mut buf = vec![0u8; 1 << 20];
            let t = Instant::now();
            let (mut off, mut sum) = (0u64, 0u64);
            loop { let n = fs.read_at(&e, off, &mut buf).unwrap(); if n == 0 { break; } off += n as u64; sum = sum.wrapping_add(buf[0] as u64); }
            let s = t.elapsed().as_secs_f64();
            println!("{{\"mode\":\"read\",\"fs\":\"{kind}\",\"bytes\":{off},\"seconds\":{s:.4},\"mb_s\":{:.1},\"c\":{sum}}}", off as f64 / 1e6 / s);
        }
        "write" => {
            let fs = open(img, kind, true);
            let mut f = std::fs::File::open(&a[5]).unwrap();
            let len = f.metadata().unwrap().len();
            let t = Instant::now();
            fs.write_file(&a[4], &mut f, false).unwrap();
            let s = t.elapsed().as_secs_f64();
            println!("{{\"mode\":\"write\",\"fs\":\"{kind}\",\"bytes\":{len},\"seconds\":{s:.4},\"mb_s\":{:.1}}}", len as f64 / 1e6 / s);
        }
        "many" => {
            let n: usize = a[4].parse().unwrap();
            let fs = open(img, kind, true);
            fs.create_dir("/bench").unwrap();
            let t = Instant::now();
            for i in 0..n { fs.write_file(&format!("/bench/file-{i:05}.txt"), &mut Cursor::new(format!("small file {i}\n").into_bytes()), false).unwrap(); }
            let s = t.elapsed().as_secs_f64();
            println!("{{\"mode\":\"many\",\"fs\":\"{kind}\",\"files\":{n},\"seconds\":{s:.4},\"files_s\":{:.0}}}", n as f64 / s);
        }
        _ => panic!("mode"),
    }
}
