//! Writing the boot image the boot menu picked into the partition the
//! bootloader starts, as the xaga's own switch app does: `boot_a` only,
//! with `boot_b` left as it is.

use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The mark every Android boot image opens with, the xaga's iOS image
/// among them: its kernel is XNU, but the boot image is the one the
/// bootloader reads.
const BOOT_MAGIC: &[u8; 8] = b"ANDROID!";

/// The most a boot image may be: the partition it goes into.
const MOST: u64 = 64 * 1024 * 1024;

/// Write `image` into `partition`, after keeping the image it holds now
/// beside the switch app's own images where it can, and flush it to the
/// phone. The caller restarts into it.
pub fn write(partition: &str, image: &str) -> Result<()> {
    if skyward_vab::current_slot()? != Some(skyward_vab::Slot::A) {
        bail!("{partition} is written only while slot A runs");
    }
    let bytes = fs::read(image).with_context(|| format!("read {image}"))?;
    if !bytes.starts_with(BOOT_MAGIC) {
        bail!("{image} is not an Android boot image");
    }
    if bytes.len() as u64 > MOST {
        bail!("{image} is {} bytes, more than the {MOST}-byte boot partition", bytes.len());
    }
    let device = Path::new("/dev/block/by-name").join(partition);
    let mut file = OpenOptions::new().read(true).write(true).open(&device).with_context(|| format!("open {}", device.display()))?;
    let size = file.seek(SeekFrom::End(0)).with_context(|| format!("ask the size of {}", device.display()))?;
    if bytes.len() as u64 > size {
        bail!("{image} is {} bytes, more than {} holds ({size} bytes)", bytes.len(), device.display());
    }
    // Keep the image the phone starts from now, once, where the switch app
    // keeps its own; a recovery that cannot write the backup still goes on,
    // as the user's own backup app does not make one either.
    let backup = Path::new("/data/local/tmp/boot_a-before-skyward.img");
    if !backup.exists() && size <= MOST {
        let mut current = vec![0u8; size as usize];
        if file.seek(SeekFrom::Start(0)).and_then(|_| file.read_exact(&mut current)).is_ok()
            && let Err(e) = fs::write(backup, &current)
        {
            let _ = skyward_sys::kmsg(&format!("the boot image before the switch was not kept: {e}"));
        }
    }
    file.seek(SeekFrom::Start(0)).with_context(|| format!("seek in {}", device.display()))?;
    file.write_all(&bytes).with_context(|| format!("write {image} into {}", device.display()))?;
    file.sync_all().with_context(|| format!("flush {}", device.display()))?;
    unsafe { libc::sync() };
    let _ = skyward_sys::kmsg(&format!("wrote {image} ({} bytes) into {}", bytes.len(), device.display()));
    Ok(())
}
