//! The lock screen: the screen going off keeps what it showed, blurred,
//! behind a clock and a swipe up that lets the pages show again.

use super::*;

impl App {
    /// Whether the pages wait behind the lock screen.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Lock the pages with `screen` as the blurred background, unless the
    /// setting is off or they are locked already.
    pub fn lock_with_snapshot(&mut self, screen: &Canvas) {
        if !self.state.lock_screen || self.locked {
            return;
        }
        let radius = self.w.min(self.h) as f32 * 0.04 * self.state.lock_blur.clamp(0.0, 1.0);
        // The panel's frame is the screen's size; a page drawn for a test
        // may be taller, and only what the panel showed is kept.
        let whole = if (screen.w, screen.h) == (self.w, self.h) {
            screen.clone()
        } else if screen.w == self.w && screen.h > self.h && screen.held().start == 0 {
            screen.crop(0, 0, self.w, self.h)
        } else {
            let mut whole = Canvas::new(self.w, self.h);
            whole.replace(screen);
            whole
        };
        // The blur is made on the screen shrunk to a quarter, where the same
        // radius reaches over the same share of the picture and costs a
        // sixteenth of the work, then mixed back up to the panel's size.
        self.lock_bg = Some(match radius >= 2.0 {
            true => whole.downscaled(4).blurred(radius / 4.0).scaled(self.w, self.h),
            // Barely blurred: the sharp frame is near enough, and saves a
            // full pass over the screen at the smallest setting.
            false => whole,
        });
        self.locked = true;
        self.lock_drag = None;
        self.charging = None;
        self.charge_art = None;
        self.charge_canvas = None;
        self.chord = None;
        self.button = None;
        self.keys = false;
        self.adjusting = false;
        self.end_turn();
        self.dirty = true;
    }

    /// Let the pages show again, their background and the swipe forgotten.
    pub fn unlock_screen(&mut self) {
        if !self.locked {
            return;
        }
        self.locked = false;
        self.lock_bg = None;
        self.lock_drag = None;
        self.end_charging();
        self.refresh();
    }

    /// How far up the lock screen a finger has swiped, from 0 at rest to 1
    /// where letting go unlocks.
    pub(super) fn lock_progress(&self) -> f32 {
        let Some((start, now)) = self.lock_drag else { return 0.0 };
        ((start - now) / self.lock_threshold()).clamp(0.0, 1.0)
    }

    /// How far a finger must travel up the lock screen to unlock it.
    fn lock_threshold(&self) -> f32 {
        self.theme().m.dp(72.0).max(self.h as f32 * 0.12)
    }

    /// A finger on the lock screen: it moves the hint up, and letting go
    /// past [`App::lock_threshold`] unlocks.
    pub(super) fn touch_locked(&mut self, ev: Touch, now: u64) -> Vec<Effect> {
        self.stir(now);
        match ev {
            Touch::Down(_, y) => {
                self.lock_drag = Some((y, y));
                self.dirty = true;
            }
            Touch::Move(_, y) => {
                if let Some((start, _)) = self.lock_drag {
                    self.lock_drag = Some((start, y));
                    self.dirty = true;
                }
            }
            Touch::Up => {
                let unlocked = self.lock_drag.is_some_and(|(start, y)| start - y >= self.lock_threshold());
                self.lock_drag = None;
                self.dirty = true;
                if unlocked {
                    self.unlock_screen();
                    return if self.state.vibrate { vec![Effect::Haptic] } else { Vec::new() };
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::*;

    #[test]
    fn the_lock_screen_needs_a_swipe_up_and_follows_the_finger() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        let background = app.frame(&f, &i).canvas.clone();
        app.lock_with_snapshot(&background);
        assert!(app.is_locked());
        assert!(app.lock_bg.is_some());
        assert_eq!(app.lock_progress(), 0.0);
        let y = H as f32 * 0.8;
        assert!(app.touch(Touch::Down(W as f32 / 2.0, y), &f, 1000).is_empty());
        assert!(app.touch(Touch::Move(W as f32 / 2.0, y - 100.0), &f, 1100).is_empty());
        assert!(app.lock_progress() > 0.0);
        assert!(app.touch(Touch::Up, &f, 1200).is_empty(), "too short");
        assert!(app.is_locked());
        assert!(app.touch(Touch::Down(W as f32 / 2.0, y), &f, 2000).is_empty());
        assert_eq!(app.touch(Touch::Move(W as f32 / 2.0, 0.0), &f, 2100), Vec::<Effect>::new());
        assert_eq!(app.touch(Touch::Up, &f, 2200), vec![Effect::Haptic]);
        assert!(!app.is_locked());
        assert!(app.lock_bg.is_none());
    }

    #[test]
    fn the_lock_screen_is_only_made_where_the_setting_turns_it_on() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        app.frame(&f, &i);
        let background = app.frame(&f, &i).canvas.clone();
        app.state.lock_screen = false;
        app.lock_with_snapshot(&background);
        assert!(!app.is_locked());
    }
}
