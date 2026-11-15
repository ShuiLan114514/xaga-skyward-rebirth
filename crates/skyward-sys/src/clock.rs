use anyhow::{anyhow, bail, Result};

/// The vendor persist partition, where Android's time daemon keeps how far
/// the real time runs ahead of the phone's real-time clock.
pub const PERSIST: &str = "/dev/block/by-name/persist";

/// Where the recovery mounts persist while it reads, read-only, when it is
/// not mounted already.
pub const MOUNT: &str = "/skyward/persist";

/// The time daemon's offset for the time of day, relative to persist's root:
/// milliseconds as a little-endian u64.
pub const OFFSET: &str = "time/ats_2";

/// The real-time clock's count of seconds. On Qualcomm phones it runs from
/// whenever the PMIC last lost power, and Android never sets it.
pub const RTC: &str = "/sys/class/rtc/rtc0/since_epoch";

/// The offset an ats_ file holds, in milliseconds. A daemon that never had
/// the time stores none, and nothing can be told from the RTC alone.
pub fn offset(bytes: &[u8]) -> Result<u64> {
    let raw: [u8; 8] = bytes.try_into().map_err(|_| anyhow!("an offset is 8 bytes, this one is {}", bytes.len()))?;
    match u64::from_le_bytes(raw) {
        0 => bail!("the time daemon has stored no offset yet"),
        ms => Ok(ms),
    }
}

/// The real time, in Unix milliseconds, from the RTC's count of seconds and
/// the time daemon's offset.
pub fn wall(rtc_s: u64, offset_ms: u64) -> u64 {
    rtc_s * 1000 + offset_ms
}

/// The span a phone that keeps its own time can be at, in Unix milliseconds:
/// from before this recovery was written to after any clock still set by
/// hand. A time outside it is one nothing good can be done with.
pub const EARLIEST: u64 = 1_577_836_800_000; // 2020-01-01
pub const LATEST: u64 = 2_208_988_800_000; // 2040-01-01

/// The time to set, given what the RTC counted and the offset the time daemon
/// left: the sum, unless it lands outside [`EARLIEST`]..[`LATEST`] while the
/// RTC's own count lands inside, which is a stored offset gone wrong — a
/// phone that runs a system not its own can have one ahead by decades, and a
/// clock ahead of the certificates websites show breaks every one of them.
pub fn choose(rtc_s: u64, offset_ms: u64) -> u64 {
    let summed = wall(rtc_s, offset_ms);
    let counted = rtc_s * 1000;
    if !(EARLIEST..LATEST).contains(&summed) && (EARLIEST..LATEST).contains(&counted) {
        return counted;
    }
    summed
}

/// Set the system clock to the real time and return it, in Unix
/// milliseconds: the RTC's count plus the offset Android left on persist.
#[cfg(target_os = "android")]
pub fn set_from_rtc() -> Result<u64> {
    use anyhow::Context;
    use std::fs;
    use std::path::Path;
    // An earlier run of the recovery service may have left persist mounted
    // where the phone's own programs use it, writable, and then it cannot
    // be mounted read-only besides.
    let read = match skyward_block::mount::points_of(skyward_block::devnum(PERSIST)?)?.first() {
        Some(at) => fs::read(at.join(OFFSET)).with_context(|| format!("read persist's {OFFSET}")),
        None => {
            let mount = Path::new(MOUNT);
            fs::create_dir_all(mount).with_context(|| format!("make {}", mount.display()))?;
            skyward_block::mount::mount_ro(Path::new(PERSIST), mount)?;
            let read = fs::read(mount.join(OFFSET)).with_context(|| format!("read persist's {OFFSET}"));
            skyward_block::mount::umount(mount)?;
            read
        }
    };
    let offset_ms = offset(&read?)?;
    let rtc = fs::read_to_string(RTC).with_context(|| format!("read {RTC}"))?;
    let rtc_s: u64 = rtc.trim().parse().with_context(|| format!("{RTC} holds {rtc:?}"))?;
    let now = choose(rtc_s, offset_ms);
    let ts = libc::timespec { tv_sec: (now / 1000) as libc::time_t, tv_nsec: ((now % 1000) * 1_000_000) as libc::c_long };
    if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } != 0 {
        return Err(std::io::Error::last_os_error()).context("set the system clock");
    }
    Ok(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offset_and_the_rtc_make_the_time() {
        // A 13 Ultra on 2026-10-19: its RTC had counted 411912 seconds and
        // ats_2 held 1791968239417 ms; the computer's clock said
        // 1792380150 at that moment.
        let held = 1_791_968_239_417u64.to_le_bytes();
        let now = wall(411_912, offset(&held).unwrap());
        assert!(now / 1000 >= 1_792_380_150 && now / 1000 <= 1_792_380_152, "{now}");
    }

    #[test]
    fn a_stored_offset_ahead_by_decades_gives_way_to_the_rtc() {
        // The Redmi Note 11T Pro this port was written for: its RTC counted
        // the real time, and a system not its own had left an offset that
        // put the wall clock in 2080.
        let rtc = 1_794_644_740;
        assert_eq!(choose(rtc, 1_688_115_833_265), rtc * 1000, "the RTC's own count, which was right");
        // A phone whose RTC counts from when its power last went: the offset
        // is the only thing that makes the time.
        let counted = 411_912;
        assert_eq!(choose(counted, 1_791_968_239_417), wall(counted, 1_791_968_239_417));
    }

    #[test]
    fn an_offset_never_stored_or_cut_short_is_refused() {
        assert!(offset(&[0; 8]).is_err());
        assert!(offset(&[1, 2, 3]).is_err());
    }
}
