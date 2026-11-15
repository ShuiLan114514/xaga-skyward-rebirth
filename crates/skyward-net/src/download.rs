//! Downloading a package whole: from where an earlier download of it
//! stopped, over connections that may drop, and checked against the digest
//! its server gave before it takes its own name.

use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use sha2::Sha256;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How much is read and written at a time.
const CHUNK: usize = 1 << 20;

/// The digest a package's server gives of it, in hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sum {
    Md5(String),
    Sha256(String),
}

/// What the phone has for files to grow into, in bytes, as the kernel says:
/// what a program may ask for without the phone starting to swap out what it
/// runs.
fn memory() -> Option<u64> {
    let info = fs::read_to_string("/proc/meminfo").ok()?;
    available(&info)
}

/// The `MemAvailable` a `/proc/meminfo` holds, in bytes.
fn available(info: &str) -> Option<u64> {
    let line = info.lines().find(|line| line.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Where a package is downloaded to until it is whole and checked.
pub fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().expect("a package has a name").to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// How far a download has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// What an earlier download left is read for its digest, before the
    /// rest is added to it.
    Checking { done: u64, total: u64 },
    Downloading { done: u64, total: u64 },
}

/// Why a download did not end with the package whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The folder has too little room for what is left of it.
    NoRoom { needed: u64, free: u64 },
    /// What was downloaded was not the package its server described, and
    /// is gone.
    Damaged,
    /// The server now offers another system than the one downloaded.
    Changed,
    /// The connection kept failing, or anything else: why.
    Other(String),
}

/// The server turned the download down: what a signed address that has
/// run out gets, or one it no longer serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused(pub u16);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the server answered {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// The whole package does not have the digest its server gave; what was
/// downloaded of it is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damaged;

impl std::fmt::Display for Damaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the package downloaded does not have the digest its server gave")
    }
}

impl std::error::Error for Damaged {}

/// The folder has too little room for what is left to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoRoom {
    pub needed: u64,
    pub free: u64,
}

impl std::fmt::Display for NoRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} bytes more are needed, and {} are free", self.needed, self.free)
    }
}

impl std::error::Error for NoRoom {}

/// The digest, by `D`, of what `file` holds, read from its start.
fn hash_of<D: Digest>(file: &mut File, total: u64, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<D> {
    let mut hash = D::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            bail!(crate::Stopped);
        }
        let n = file.read(&mut buf).context("read what was downloaded before")?;
        if n == 0 {
            return Ok(hash);
        }
        hash.update(&buf[..n]);
        done += n as u64;
        progress(Progress::Checking { done, total });
    }
}

/// Download `url` to `path`, carrying on from what an earlier download
/// left, and give it its name once it has the digest `sum`. A part as long
/// as the package is only checked; a server that will not send from the
/// middle sends it all again.
pub fn fetch(agent: &ureq::Agent, url: &str, path: &Path, sum: &Sum, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    match sum {
        Sum::Md5(hex) => fetch_as::<Md5>(agent, url, path, hex, stop, progress),
        Sum::Sha256(hex) => fetch_as::<Sha256>(agent, url, path, hex, stop, progress),
    }
}

/// [`fetch`], with the digest by `D` in hex.
fn fetch_as<D: Digest>(agent: &ureq::Agent, url: &str, path: &Path, sum: &str, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let part = partial(path);
    let dir = path.parent().context("a package's path is in a folder")?;
    fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    let head = agent.head(url).call().context("ask the server about the package")?;
    match head.status().as_u16() {
        200 => {}
        status => bail!(Refused(status)),
    }
    let total = content_length(head.headers()).context("the server does not say how long the package is")?;
    let mut file = OpenOptions::new().create(true).read(true).append(true).open(&part).with_context(|| format!("open {}", part.display()))?;
    let mut have = file.metadata().with_context(|| format!("read {}", part.display()))?.len();
    if have > total {
        file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
        have = 0;
    }
    let space = skyward_block::mount::space(dir).with_context(|| format!("read the free space of {}", dir.display()))?;
    // A ramfs, which the recovery's own root is, has no size to say: what it
    // may hold is the phone's memory, which the kernel hands out as files
    // ask for it.
    let room = if space.total == 0 { memory().unwrap_or(space.available) } else { space.available };
    if total - have > room {
        bail!(NoRoom { needed: total - have, free: room });
    }
    let mut hash = hash_of::<D>(&mut file, have, stop, progress)?;
    if have < total {
        let mut asked = agent.get(url);
        if have > 0 {
            asked = asked.header("Range", format!("bytes={have}-"));
        }
        let mut response = asked.call().context("start the download")?;
        match response.status().as_u16() {
            206 if have > 0 => {}
            200 => {
                if have > 0 {
                    file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
                    have = 0;
                    hash = D::new();
                }
            }
            status => bail!(Refused(status)),
        }
        let mut body = response.body_mut().with_config().limit(u64::MAX).reader();
        let mut buf = vec![0u8; CHUNK];
        let mut done = have;
        progress(Progress::Downloading { done, total });
        while done < total {
            if stop.load(Ordering::Relaxed) {
                bail!(crate::Stopped);
            }
            let n = body.read(&mut buf).context("download the package")?;
            if n == 0 {
                bail!("the server stopped sending at {done} of {total} bytes");
            }
            let n = n.min((total - done) as usize);
            file.write_all(&buf[..n]).with_context(|| format!("write {}", part.display()))?;
            hash.update(&buf[..n]);
            done += n as u64;
            progress(Progress::Downloading { done, total });
        }
        file.sync_all().with_context(|| format!("write {} through", part.display()))?;
    }
    if hex::encode(hash.finalize()) != sum.to_lowercase() {
        drop(file);
        fs::remove_file(&part).with_context(|| format!("delete {}", part.display()))?;
        bail!(Damaged);
    }
    fs::rename(&part, path).with_context(|| format!("name {} {}", part.display(), path.display()))
}

/// The length a response says its body has.
pub(crate) fn content_length(headers: &ureq::http::HeaderMap) -> Option<u64> {
    headers.get(ureq::http::header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_left_is_read_from_the_kernels_own_note() {
        let info = "MemTotal:        7409536 kB\nMemFree:          200000 kB\nMemAvailable:    6824672 kB\n";
        assert_eq!(available(info), Some(6824672 * 1024), "the line for what may be asked for");
        assert_eq!(available("MemTotal: 1 kB\n"), None, "a kernel that does not say");
        assert_eq!(available("MemAvailable: plenty kB\n"), None, "what cannot be read as a number");
    }

    #[test]
    fn a_part_is_named_after_its_package() {
        assert_eq!(partial(Path::new("/sdcard/downloaded_rom/a.zip")), Path::new("/sdcard/downloaded_rom/a.zip.part"));
    }

    #[test]
    fn what_is_there_is_read_for_its_digest() {
        let dir = std::env::temp_dir().join(format!("skyward-net-hash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.part");
        fs::write(&path, vec![7u8; CHUNK + 5]).unwrap();
        let mut seen = Vec::new();
        let hash = hash_of::<Md5>(&mut File::open(&path).unwrap(), (CHUNK + 5) as u64, &AtomicBool::new(false), &mut |p| seen.push(p)).unwrap();
        assert_eq!(hash.finalize(), Md5::digest(vec![7u8; CHUNK + 5]));
        assert_eq!(seen.last(), Some(&Progress::Checking { done: (CHUNK + 5) as u64, total: (CHUNK + 5) as u64 }));
        let hash = hash_of::<Sha256>(&mut File::open(&path).unwrap(), (CHUNK + 5) as u64, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(hash.finalize(), Sha256::digest(vec![7u8; CHUNK + 5]));
        let stopped = hash_of::<Md5>(&mut File::open(&path).unwrap(), 1, &AtomicBool::new(true), &mut |_| {});
        assert!(stopped.unwrap_err().is::<crate::Stopped>());
        fs::remove_dir_all(&dir).unwrap();
    }
}
