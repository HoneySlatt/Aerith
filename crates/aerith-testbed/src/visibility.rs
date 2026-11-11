// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Whether the controls show: from an input until [`HIDE_AFTER`] after the last one.

use std::time::{Duration, Instant};

/// How long the controls stay after the last input.
pub const HIDE_AFTER: Duration = Duration::from_secs(3);

/// The controls' visibility and its hide timer. They show exactly while the timer runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    hide_at: Option<Instant>,
}

impl Visibility {
    pub fn shown(&self) -> bool {
        self.hide_at.is_some()
    }

    /// When the controls hide, while they show: the window's loop waits until then.
    pub fn deadline(&self) -> Option<Instant> {
        self.hide_at
    }

    /// Shows the controls, or keeps them, until [`HIDE_AFTER`] from `now`.
    pub fn show(&mut self, now: Instant) {
        self.hide_at = Some(now + HIDE_AFTER);
    }

    pub fn hide(&mut self) {
        self.hide_at = None;
    }

    /// Shows hidden controls, hides shown ones.
    pub fn toggle(&mut self, now: Instant) {
        if self.shown() {
            self.hide();
        } else {
            self.show(now);
        }
    }

    /// Hides the controls if their time ran out by `now`.
    pub fn expire(&mut self, now: Instant) {
        if self.hide_at.is_some_and(|hide_at| now >= hide_at) {
            self.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_start_hidden_with_no_timer() {
        let visibility = Visibility::default();
        assert!(!visibility.shown());
        assert_eq!(visibility.deadline(), None);
    }

    #[test]
    fn an_input_shows_them_until_three_seconds_after_it() {
        let start = Instant::now();
        let mut visibility = Visibility::default();
        visibility.show(start);
        assert_eq!(visibility.deadline(), Some(start + HIDE_AFTER));
        visibility.expire(start + Duration::from_millis(2999));
        assert!(visibility.shown());
        visibility.expire(start + HIDE_AFTER);
        assert!(!visibility.shown());
        assert_eq!(visibility.deadline(), None);
    }

    #[test]
    fn each_input_restarts_the_timer() {
        let start = Instant::now();
        let mut visibility = Visibility::default();
        visibility.show(start);
        visibility.show(start + Duration::from_secs(2));
        visibility.expire(start + Duration::from_secs(4));
        assert!(visibility.shown());
        assert_eq!(
            visibility.deadline(),
            Some(start + Duration::from_secs(2) + HIDE_AFTER)
        );
    }

    #[test]
    fn a_toggle_hides_shown_controls_and_shows_hidden_ones() {
        let start = Instant::now();
        let mut visibility = Visibility::default();
        visibility.toggle(start);
        assert_eq!(visibility.deadline(), Some(start + HIDE_AFTER));
        visibility.toggle(start + Duration::from_secs(1));
        assert!(!visibility.shown());
        assert_eq!(visibility.deadline(), None);
    }

    #[test]
    fn an_expiry_while_hidden_changes_nothing() {
        let mut visibility = Visibility::default();
        visibility.expire(Instant::now() + Duration::from_secs(60));
        assert_eq!(visibility, Visibility::default());
    }
}
