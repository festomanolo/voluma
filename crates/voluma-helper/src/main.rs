//! Privileged NTFS helper. macOS mounts NTFS read-only, so writing needs VOLUMA's own driver on the raw
//! partition, and raw device nodes need root. The app unmounts the volume, then starts this once with
//! administrator rights; it serves requests over a Unix socket until told to quit.
//!
//! Hardening (this process may run as root):
//! * it serves exactly ONE device, fixed at launch (`/dev/diskN` or `/dev/diskNsM`);
//! * the socket is mode 0600, owned by the invoking user, and each connection's peer uid is checked;
//! * it only touches host files by bare name inside the staging directory, opens them with O_NOFOLLOW, requires
//!   regular files owned by the invoking user, and copies through a root-private temp directory, so a request can
//!   neither read arbitrary files into the volume nor write arbitrary files out;
//! * it exits when the app exits or after a period without requests.
//! The NTFS driver itself still refuses dirty / hibernated / unclean-log volumes.

use serde_json::{json, Value};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Cfg { device: String, stage: PathBuf, uid: u32, parent: i32, idle: Duration, private: PathBuf }

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let get = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    if a.get(1).map(String::as_str) != Some("serve") { eprintln!("usage: voluma-helper serve --device D --stage DIR --uid N --parent PID [--idle SECS]"); std::process::exit(2); }
    let (Some(device), Some(stage), Some(uid), Some(parent)) = (get("--device"), get("--stage"), get("--uid"), get("--parent")) else { eprintln!("missing arguments"); std::process::exit(2) };
    let idle = get("--idle").and_then(|s| s.parse().ok()).unwrap_or(900u64);
    if let Err(e) = run(device, stage, uid, parent, idle) { eprintln!("voluma-helper: {e}"); std::process::exit(1); }
}

fn is_digits(s: &str) -> bool { !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) }

fn valid_alpha_part(rest: &str) -> bool {
    let alpha = rest.bytes().take_while(|b| b.is_ascii_lowercase()).count();
    alpha > 0 && rest[alpha..].bytes().all(|b| b.is_ascii_digit())
}

fn valid_num_opt_p(rest: &str) -> bool {
    match rest.split_once('p') {
        Some((n, p)) => is_digits(n) && is_digits(p),
        None => is_digits(rest),
    }
}

pub fn valid_device(d: &str) -> bool {
    if d.contains('/') && !d.starts_with("/dev/") { return false; }
    let Some(name) = d.strip_prefix("/dev/") else { return false };
    if name.is_empty() || name.contains('/') || name.contains('.') || name.contains('\0') { return false; }
    if let Some(rest) = name.strip_prefix("disk") {
        let (n, s) = match rest.split_once('s') { Some((n, s)) => (n, Some(s)), None => (rest, None) };
        return is_digits(n) && s.map_or(true, is_digits);
    }
    for pfx in ["sd", "vd", "hd", "xvd"] {
        if let Some(rest) = name.strip_prefix(pfx) { return valid_alpha_part(rest); }
    }
    if let Some(rest) = name.strip_prefix("nvme") {
        if let Some((ctrl, ns_part)) = rest.split_once('n') {
            return is_digits(ctrl) && valid_num_opt_p(ns_part);
        }
        return false;
    }
    for pfx in ["mmcblk", "loop"] {
        if let Some(rest) = name.strip_prefix(pfx) { return valid_num_opt_p(rest); }
    }
    for pfx in ["dm-", "sr", "md"] {
        if let Some(rest) = name.strip_prefix(pfx) { return is_digits(rest); }
    }
    false
}

fn run(device: String, stage: String, uid: String, parent: String, idle: u64) -> Result<(), String> {
    if !valid_device(&device) { return Err("device must be a valid block device (/dev/diskN[sM], /dev/sdX[N], /dev/nvmeXnY[pZ], /dev/mmcblkX[pY], /dev/loopX)".into()); }
    let uid: u32 = uid.parse().map_err(|_| "bad uid")?;
    let parent: i32 = parent.parse().map_err(|_| "bad parent pid")?;
    let stage = fs::canonicalize(&stage).map_err(|e| format!("stage dir: {e}"))?;
    let md = fs::symlink_metadata(&stage).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::MetadataExt;
        if !md.is_dir() || md.uid() != uid { return Err("stage dir must be a directory owned by the invoking user".into()); }
    }
    let private = std::env::temp_dir().join(format!("voluma-helper-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&private).map_err(|e| format!("private dir: {e}"))?;
    let cfg = Cfg { device, stage: stage.clone(), uid, parent, idle: Duration::from_secs(idle), private };
    let sock = stage.join("helper.sock");
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).map_err(|e| format!("bind: {e}"))?;
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    let c = CString::new(sock.to_str().ok_or("socket path")?).unwrap();
    if unsafe { libc::chown(c.as_ptr(), uid, u32::MAX) } != 0 && unsafe { libc::geteuid() } == 0 { return Err("chown socket failed".into()); }
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut last = Instant::now();
    loop {
        if unsafe { libc::kill(cfg.parent, 0) } != 0 { break; }               // the app is gone
        if last.elapsed() > cfg.idle { break; }
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).ok();
                if peer_uid(&s) != Some(cfg.uid) { continue; }                  // only the invoking user
                last = Instant::now();
                if serve(s, &cfg, &mut last) { break; }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let _ = fs::remove_file(&sock);
    let _ = fs::remove_dir_all(&cfg.private);
    Ok(())
}

#[cfg(target_os = "macos")]
fn peer_uid(s: &UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;
    let (mut u, mut g) = (0u32, 0u32);
    if unsafe { libc::getpeereid(s.as_raw_fd(), &mut u, &mut g) } == 0 { Some(u) } else { None }
}

#[cfg(target_os = "linux")]
fn peer_uid(s: &UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc == 0 { Some(cred.uid) } else { None }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_uid(_s: &UnixStream) -> Option<u32> { None }

/// Returns true when the helper should exit.
fn serve(stream: UnixStream, cfg: &Cfg, last: &mut Instant) -> bool {
    let mut out = match stream.try_clone() { Ok(o) => o, Err(_) => return false };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return false };
        *last = Instant::now();
        let (reply, quit) = handle(&line, cfg);
        if writeln!(out, "{reply}").is_err() { return false; }
        if quit { return true; }
    }
    false
}

fn err(m: impl Into<String>) -> String { json!({ "error": m.into() }).to_string() }

fn bare_name(n: &str) -> bool { !n.is_empty() && n != "." && n != ".." && n != "helper.sock" && !n.contains('/') && !n.contains('\0') && n.len() < 200 }

fn handle(line: &str, cfg: &Cfg) -> (String, bool) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return (err("bad request"), false) };
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let part = v.get("part").and_then(Value::as_i64).unwrap_or(-1) as i32;
    let dev = cfg.device.as_str();
    let r = match v.get("op").and_then(Value::as_str).unwrap_or("") {
        "ping" => json!({ "ok": "pong" }).to_string(),
        "quit" => { flush_device(dev); return (json!({ "ok": "bye" }).to_string(), true) }
        "inspect" => voluma_ffi::inspect_image_json(dev),
        "list" => voluma_ffi::list_directory_json(dev, part, &s("path")),
        "mkdir" | "delete" => voluma_ffi::mutate_json(dev, part, &s("op"), &s("path"), "", false),
        "rename" => voluma_ffi::mutate_json(dev, part, "rename", &s("path"), &s("to"), false),
        "import" => import(cfg, part, &s("src"), &s("path")),
        "export" => export(cfg, part, &s("path"), &s("dst")),
        _ => err("unknown operation"),
    };
    // Block-device writes go through the buffer cache: push them to the media before replying.
    if matches!(v.get("op").and_then(Value::as_str), Some("mkdir" | "delete" | "rename" | "import")) { flush_device(dev); }
    (r, false)
}

fn flush_device(dev: &str) {
    use std::os::unix::io::AsRawFd;
    if let Ok(f) = fs::OpenOptions::new().read(true).write(true).open(dev) {
        unsafe {
            libc::fsync(f.as_raw_fd());
            #[cfg(target_os = "macos")]
            libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC);
        }
    }
}

/// stage/<src> -> (private copy) -> NTFS:<path>. `src` is a bare file name; it must be a regular, non-symlink
/// file owned by the invoking user.
fn import(cfg: &Cfg, part: i32, src: &str, path: &str) -> String {
    if !bare_name(src) { return err("invalid staged file name"); }
    let p = CString::new(cfg.stage.join(src).to_str().unwrap_or("")).unwrap();
    let fd = unsafe { libc::open(p.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW) };
    if fd < 0 { return err("cannot open staged file (symlinks are refused)"); }
    let mut f = unsafe { File::from_raw_fd(fd) };
    let md = match f.metadata() { Ok(m) => m, Err(e) => return err(e.to_string()) };
    {
        use std::os::unix::fs::MetadataExt;
        if !md.is_file() || md.uid() != cfg.uid { return err("staged file must be a regular file owned by you"); }
    }
    let tmp = cfg.private.join("in.bin");
    let _ = fs::remove_file(&tmp);
    let res = (|| -> std::io::Result<()> {
        let mut o = fs::OpenOptions::new().write(true).create_new(true).mode_0600().open(&tmp)?;
        std::io::copy(&mut f, &mut o)?; o.sync_all()
    })();
    if let Err(e) = res { let _ = fs::remove_file(&tmp); return err(e.to_string()); }
    let r = voluma_ffi::mutate_json(&cfg.device, part, "import", path, tmp.to_str().unwrap(), false);
    let _ = fs::remove_file(&tmp);
    r
}

fn export(cfg: &Cfg, part: i32, path: &str, dst: &str) -> String {
    if !bare_name(dst) { return err("invalid destination name"); }
    let tmp = cfg.private.join("out.bin");
    let _ = fs::remove_file(&tmp);
    let r = voluma_ffi::export_file_json(&cfg.device, part, path, tmp.to_str().unwrap());
    if serde_json::from_str::<Value>(&r).ok().and_then(|v| v.get("ok").cloned()).is_none() { let _ = fs::remove_file(&tmp); return r; }
    let p = CString::new(cfg.stage.join(dst).to_str().unwrap_or("")).unwrap();
    let fd = unsafe { libc::open(p.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW, 0o600) };
    if fd < 0 { let _ = fs::remove_file(&tmp); return err("cannot create destination in the staging directory"); }
    unsafe { libc::fchown(fd, cfg.uid, u32::MAX); }
    let mut o = unsafe { File::from_raw_fd(fd) };
    let res = File::open(&tmp).and_then(|mut i| std::io::copy(&mut i, &mut o)).and_then(|_| o.sync_all());
    let _ = fs::remove_file(&tmp);
    match res { Ok(()) => r, Err(e) => err(e.to_string()) }
}

trait Mode0600 { fn mode_0600(&mut self) -> &mut Self; }
impl Mode0600 for fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self { use std::os::unix::fs::OpenOptionsExt; self.mode(0o600) }
}

#[allow(dead_code)]
fn _unused(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_macos_and_linux_block_devices() {
        for good in [
            "/dev/disk0", "/dev/disk2s1", "/dev/disk12s3",
            "/dev/sda", "/dev/sda1", "/dev/sdb12", "/dev/nvme0n1", "/dev/nvme0n1p1",
            "/dev/mmcblk0", "/dev/mmcblk0p2", "/dev/loop0", "/dev/loop0p1",
            "/dev/vda", "/dev/vda1", "/dev/dm-0", "/dev/sr0", "/dev/md0",
        ] {
            assert!(valid_device(good), "expected valid device: {good}");
        }
        for bad in [
            "", "/etc/passwd", "/dev/null", "/dev/zero", "/dev/disk", "/dev/diskXs1",
            "/dev/disk1s", "/dev/disk1/../../etc", "disk1", "/dev/sd", "/dev/sd1",
            "/dev/nvme0", "/dev/nvme0n", "/dev/nvme0n1p", "/dev/loop", "/dev/loop0p",
            "/dev/sda/../null", "/dev/sda.img",
        ] {
            assert!(!valid_device(bad), "expected rejected device: {bad}");
        }
    }
}
