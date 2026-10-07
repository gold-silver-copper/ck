//! The footer's message, changed only here: nothing replaces an error before it's been drawn
//! (another error joins it), so every failure is seen.

use std::time::{Duration, Instant};

/// A message in the footer, for a moment (errors a little longer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

impl Status {
    /// How long it replaces the key hints.
    fn ttl(&self) -> Duration {
        Duration::from_secs(if self.error { 5 } else { 2 })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Footer {
    shown: Option<Status>,
    /// When `shown` was first drawn (the tick before the draw that shows it).
    since: Option<Instant>,
}

impl Footer {
    pub fn get(&self) -> Option<&Status> {
        self.shown.as_ref()
    }

    fn undrawn_error(&self) -> bool {
        self.since.is_none() && self.shown.as_ref().is_some_and(|s| s.error)
    }

    /// Say something, if the rule lets it. The same message again keeps its time.
    pub(super) fn say(&mut self, s: Status) {
        let undrawn = self.undrawn_error();
        match &mut self.shown {
            Some(old) if *old == s || (undrawn && !s.error) => {}
            Some(old) if undrawn => {
                for part in s.text.split(" · ") {
                    if !old.text.split(" · ").any(|seen| seen == part) {
                        old.text = format!("{} · {part}", old.text);
                    }
                }
            }
            _ => *self = Footer { shown: Some(s), since: None },
        }
    }

    /// An info, only if nothing else is showing.
    pub(super) fn offer(&mut self, text: String) {
        self.shown.get_or_insert(Status { text, error: false });
    }

    /// A new load: what was said before it goes, except an error not yet seen.
    pub(super) fn clear_seen(&mut self) {
        if !self.undrawn_error() {
            *self = Footer::default();
        }
    }

    /// Before a draw: start the message's time if the draw shows it, or end it once it's up.
    pub(super) fn tick(&mut self, now: Instant, on_screen: bool) {
        let Some(s) = &self.shown else { return };
        match self.since {
            None if on_screen => self.since = Some(now),
            Some(t) if now.saturating_duration_since(t) >= s.ttl() => *self = Footer::default(),
            _ => {}
        }
    }

    /// When the message is up.
    pub(super) fn due(&self) -> Option<Instant> {
        self.since?.checked_add(self.shown.as_ref()?.ttl())
    }
}

/// Something that went wrong, in words for the footer: an `anyhow::Error` through `http::plain`.
pub struct Problem(pub(super) String);

macro_rules! problem_from {
    ($($t:ty => |$x:ident| $text:expr),* $(,)?) => {
        $(impl From<$t> for Problem {
            fn from($x: $t) -> Self {
                Problem($text)
            }
        })*
    };
}

problem_from! {
    anyhow::Error => |e| crate::http::plain(&e),
    String => |s| s,
    &str => |s| s.to_string(),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(t: &str) -> Status {
        Status { text: t.into(), error: false }
    }

    fn error(t: &str) -> Status {
        Status { text: t.into(), error: true }
    }

    fn text(f: &Footer) -> Option<&str> {
        f.get().map(|s| s.text.as_str())
    }

    #[test]
    fn an_info_doesnt_replace_an_error_until_its_drawn() {
        let t = Instant::now();
        let mut f = Footer::default();
        f.say(error("Couldn't save"));
        f.say(info("Sorted"));
        assert_eq!(f.get(), Some(&error("Couldn't save")), "undrawn");
        f.tick(t, true);
        f.say(info("Sorted"));
        assert_eq!(f.get(), Some(&info("Sorted")), "drawn");
        // An info gives way to an error at once.
        f.say(error("Oops"));
        assert_eq!(f.get(), Some(&error("Oops")));
    }

    #[test]
    fn errors_before_a_draw_are_joined_once() {
        let t = Instant::now();
        let mut f = Footer::default();
        for e in ["Couldn't save: disk full", "disk full", "Couldn't save: disk full", "disk full", "disk full · Couldn't save: disk full"] {
            f.say(error(e));
        }
        // A whole repeat is dropped, not one that's part of another.
        assert_eq!(f.get(), Some(&error("Couldn't save: disk full · disk full")));
        // Once drawn, a new error replaces it.
        f.tick(t, true);
        f.say(error("c"));
        assert_eq!(f.get(), Some(&error("c")));
    }

    #[test]
    fn a_message_behind_the_spinner_waits_to_be_seen() {
        let t = Instant::now();
        let mut f = Footer::default();
        f.say(error("Couldn't save"));
        f.tick(t, false);
        f.tick(t + Duration::from_secs(60), false);
        f.say(info("Sorted"));
        assert_eq!((f.get(), f.due()), (Some(&error("Couldn't save")), None), "not on screen yet");
        f.tick(t + Duration::from_secs(60), true);
        assert_eq!(f.due(), Some(t + Duration::from_secs(65)));
        // Once seen, its time runs whatever is drawn.
        f.tick(t + Duration::from_secs(65), false);
        assert_eq!(f.get(), None);
    }

    #[test]
    fn an_offer_fills_only_an_empty_footer_and_a_load_spares_only_an_unseen_error() {
        let t = Instant::now();
        let mut f = Footer::default();
        f.offer("Up to date".into());
        f.offer("New posts".into());
        assert_eq!(text(&f), Some("Up to date"));
        f.clear_seen();
        assert_eq!(f.get(), None);
        f.say(error("Couldn't save"));
        f.offer("Up to date".into());
        f.clear_seen();
        assert_eq!(text(&f), Some("Couldn't save"), "not seen yet");
        f.tick(t, true);
        f.clear_seen();
        assert_eq!(f.get(), None, "seen");
    }

    #[test]
    fn a_message_lasts_from_its_first_draw_and_the_same_one_again_keeps_its_time() {
        let t = Instant::now();
        let mut f = Footer::default();
        f.say(info("Sorted"));
        assert_eq!(f.due(), None, "not drawn yet");
        f.tick(t + Duration::from_secs(10), true);
        let start = t + Duration::from_secs(10);
        assert_eq!(f.due(), Some(start + Duration::from_secs(2)));
        f.say(info("Sorted"));
        f.tick(start + Duration::from_millis(1999), true);
        assert_eq!(text(&f), Some("Sorted"));
        f.tick(start + Duration::from_secs(2), true);
        assert_eq!((f.get(), f.due()), (None, None));
        f.say(error("Oops"));
        f.tick(start, true);
        assert_eq!(f.due(), Some(start + Duration::from_secs(5)));
    }
}
