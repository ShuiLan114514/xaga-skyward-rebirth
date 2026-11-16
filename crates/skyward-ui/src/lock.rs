//! The lock screen drawn over the blurred frame the screen went off on:
//! the time, the day, what the battery does, and the swipe up that lets
//! the pages show again.

use crate::canvas::Canvas;
use crate::fmt;
use crate::font::Fonts;
use crate::icons::{self, Icon};
use crate::lang::Lang;
use crate::theme::Theme;
use skyward_info::DeviceInfo;

/// The time now, for the day the lock screen shows.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Draw the lock screen over `bg`, which the caller has already lifted
/// onto the panel. `progress` is how far a finger has swiped up, from 0 at
/// rest to 1 where letting go unlocks.
#[allow(clippy::too_many_arguments)]
pub fn draw(c: &mut Canvas, t: &Theme, f: &Fonts, info: &DeviceInfo, lang: Lang, clock: &str, zone: usize, progress: f32) {
    let m = &t.m;
    let (w, h) = (m.w as f32, m.h as f32);
    let cx = w / 2.0;
    let ink = t.pal.text;
    let soft = t.pal.subtitle;
    // The panel shows the blurred page under a veil, light or dark as the
    // theme is, so the words keep the stock theme's contrast.
    let veil = if t.dark {
        crate::color::Rgba::rgb(0, 0, 0).with_alpha(130)
    } else {
        crate::color::Rgba::rgb(255, 255, 255).with_alpha(120)
    };
    c.shade_rows(0, m.h, |_, _| veil);

    // A small padlock over the clock, as the stock lock screen has one.
    let lock_ink = ink.with_alpha(150);
    icons::glyph(c, Icon::Lock, cx, m.dp(58.0), m.dp(20.0), lock_ink);

    let now = unix_now();
    let clock = if clock.is_empty() { fmt::clock(now, zone) } else { clock.to_string() };
    let cy = h * 0.30;
    c.text_center(&f.heading, &clock, cx, cy, m.dp(76.0), ink);
    let date = fmt::date(lang, now, zone);
    c.text_center(&f.body, &date, cx, cy + m.dp(74.0), m.dp(16.0), soft.with_alpha(220));

    if let Some(battery) = &info.battery {
        let level = match battery.charge {
            skyward_info::Charge::Charging | skyward_info::Charge::Full => fmt::level(lang, battery),
            _ => format!("{}%", battery.level),
        };
        c.text_center(&f.body, &level, cx, cy + m.dp(106.0), m.dp(14.0), soft);
    }

    // The hint follows the finger up, and says what letting go does.
    let lift = m.dp(36.0) * progress.clamp(0.0, 1.0);
    let hint_y = h - m.dp(82.0) - lift;
    let hint = if progress >= 0.55 {
        lang.pick(["松开解锁", "放開解鎖", "Release to unlock"])
    } else {
        lang.pick(["上滑解锁", "上滑解鎖", "Swipe up to unlock"])
    };
    let hint_ink = ink.with_alpha((190.0 - progress * 60.0).max(0.0) as u8);
    let (x0, x1) = (cx - m.dp(7.0), cx + m.dp(7.0));
    let top = hint_y - m.dp(24.0);
    c.line(x0, top + m.dp(7.0), cx, top, m.dp(2.0), hint_ink);
    c.line(cx, top, x1, top + m.dp(7.0), m.dp(2.0), hint_ink);
    c.text_center(&f.body, hint, cx, hint_y, m.dp(15.0), hint_ink);
}
