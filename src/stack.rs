// SPDX-License-Identifier: GPL-3.0-or-later
//! Which notifications are open, in what order, and when each one goes.
//!
//! No bus, no surface and no real clock: every method that cares about time is handed `now`,
//! so the whole of the expiry logic is testable by passing made-up instants.
//!
//! # A clock starts when a notification is on screen, not when it arrives
//!
//! Three cases make the difference, and in each one counting from arrival would throw a
//! notification away unseen:
//!
//! - **More arrived than fit.** The ones past `max_visible` wait below the fold, and their
//!   eight seconds should not run out while they wait.
//! - **The monitors are blanked.** Every layer surface is closed when an output goes, and the
//!   stack is not drawn again until one comes back -- possibly hours later.
//! - **The pointer is resting on it.** Someone is reading it, so its clock is held, and given
//!   back what was left when the pointer moves away.
//!
//! The loop calls [`Stack::start_clocks`] only once a frame has actually been drawn, so "on
//! screen" means drawn rather than merely queued.

use std::time::{Duration, Instant};

use crate::notification::Notification;

/// Where one notification's expiry stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clock {
    /// Not on screen yet, or never expires.
    Waiting,
    Running(Instant),
    /// The pointer is over it; this much was left when it arrived.
    Held(Duration),
}

/// One open notification.
#[derive(Debug, Clone)]
pub struct Entry {
    pub notification: Notification,
    /// How long it lasts once shown. `None` is until dismissed.
    lifetime: Option<Duration>,
    clock: Clock,
    /// Whether the pointer is over it. Kept apart from [`Clock::Held`] so a notification that is
    /// hovered before its clock ever started does not start running underneath the pointer.
    hovered: bool,
}

impl Entry {
    pub fn id(&self) -> u32 {
        self.notification.id
    }
}

/// Every open notification, oldest first.
#[derive(Debug, Default)]
pub struct Stack {
    entries: Vec<Entry>,
}

/// What [`Stack::add`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    /// A new notification.
    New,
    /// It replaced the open one with the same id, in place.
    Replaced,
    /// It took the place of an open one from the same application with the same stack tag, which
    /// had a different id. That one is gone, and its sender should be told.
    Displaced(u32),
}

impl Stack {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Open a notification, or update one already open.
    ///
    /// Replacement keeps the notification's place in the stack -- a download's progress moving
    /// to the top every time it ticks would shuffle everything under the pointer -- and restarts
    /// its clock, since what is on screen is new.
    pub fn add(&mut self, notification: Notification, lifetime: Option<Duration>) -> Added {
        let id = notification.id;
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id() == id) {
            entry.replace(notification, lifetime);
            return Added::Replaced;
        }
        if let Some(tag) = notification.hints.stack_tag.as_deref()
            && let Some(entry) = self.entries.iter_mut().find(|entry| {
                entry.notification.app_name == notification.app_name
                    && entry.notification.hints.stack_tag.as_deref() == Some(tag)
            })
        {
            let displaced = entry.id();
            entry.replace(notification, lifetime);
            return Added::Displaced(displaced);
        }
        self.entries.push(Entry {
            notification,
            lifetime,
            clock: Clock::Waiting,
            hovered: false,
        });
        Added::New
    }

    /// Close one. `None` when it was not open, which a `CloseNotification` for something that
    /// already expired routinely is.
    pub fn remove(&mut self, id: u32) -> Option<Notification> {
        let index = self.entries.iter().position(|entry| entry.id() == id)?;
        Some(self.entries.remove(index).notification)
    }

    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.entries
            .iter()
            .find(|entry| entry.id() == id)
            .map(|entry| &entry.notification)
    }

    /// The ones on screen, newest first: the newest sits nearest the corner the stack grows
    /// from, which is where the eye already is.
    pub fn visible(&self, max: usize) -> impl Iterator<Item = &Entry> {
        self.entries.iter().rev().take(max)
    }

    /// How many are waiting below the fold.
    pub fn waiting(&self, max: usize) -> usize {
        self.entries.len().saturating_sub(max)
    }

    /// Start the clock of every visible notification that has not started yet. Called once a
    /// frame showing them has been drawn.
    pub fn start_clocks(&mut self, now: Instant, max: usize) {
        for entry in self.entries.iter_mut().rev().take(max) {
            if entry.clock != Clock::Waiting || entry.hovered {
                continue;
            }
            if let Some(lifetime) = entry.lifetime {
                entry.clock = Clock::Running(now + lifetime);
            }
        }
    }

    /// Close everything whose time is up, and say which.
    pub fn expire(&mut self, now: Instant) -> Vec<u32> {
        let mut expired = Vec::new();
        self.entries.retain(|entry| match entry.clock {
            Clock::Running(deadline) if deadline <= now => {
                expired.push(entry.id());
                false
            }
            _ => true,
        });
        expired
    }

    /// The soonest anything expires, so the loop knows how long it may sleep.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter_map(|entry| match entry.clock {
                Clock::Running(deadline) => Some(deadline),
                _ => None,
            })
            .min()
    }

    /// The pointer is now over `id`, or over nothing when `None`. Holds that one's clock and gives
    /// every other one back what it had left.
    pub fn hover(&mut self, id: Option<u32>, now: Instant) {
        for entry in &mut self.entries {
            let over = Some(entry.id()) == id;
            if over == entry.hovered {
                continue;
            }
            entry.hovered = over;
            entry.clock = match (over, entry.clock) {
                (true, Clock::Running(deadline)) => {
                    Clock::Held(deadline.saturating_duration_since(now))
                }
                (false, Clock::Held(left)) => Clock::Running(now + left),
                (_, clock) => clock,
            };
        }
    }
}

impl Entry {
    fn replace(&mut self, notification: Notification, lifetime: Option<Duration>) {
        self.notification = notification;
        self.lifetime = lifetime;
        // Under the pointer, it stays held, with the whole of its new lifetime to come back to.
        self.clock = match (self.hovered, lifetime) {
            (true, Some(lifetime)) => Clock::Held(lifetime),
            _ => Clock::Waiting,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::Hints;

    fn notification(id: u32) -> Notification {
        Notification {
            id,
            app_name: "test".into(),
            app_icon: String::new(),
            summary: format!("number {id}"),
            body: Vec::new(),
            actions: Vec::new(),
            hints: Hints::default(),
            expire_timeout: -1,
        }
    }

    fn tagged(id: u32, app: &str, tag: &str) -> Notification {
        let mut n = notification(id);
        n.app_name = app.into();
        n.hints.stack_tag = Some(tag.into());
        n
    }

    const FIVE: Option<Duration> = Some(Duration::from_secs(5));

    fn ids(stack: &Stack, max: usize) -> Vec<u32> {
        stack.visible(max).map(Entry::id).collect()
    }

    #[test]
    fn newest_first_and_the_rest_wait() {
        let mut stack = Stack::default();
        for id in 1..=4 {
            assert_eq!(stack.add(notification(id), FIVE), Added::New);
        }
        assert_eq!(ids(&stack, 3), vec![4, 3, 2]);
        assert_eq!(stack.waiting(3), 1);
    }

    #[test]
    fn replacing_keeps_the_place() {
        let mut stack = Stack::default();
        stack.add(notification(1), FIVE);
        stack.add(notification(2), FIVE);
        let mut updated = notification(1);
        updated.summary = "50%".into();
        assert_eq!(stack.add(updated, FIVE), Added::Replaced);
        assert_eq!(ids(&stack, 5), vec![2, 1]);
        assert_eq!(stack.get(1).unwrap().summary, "50%");
    }

    #[test]
    fn a_stack_tag_displaces_only_its_own_application() {
        let mut stack = Stack::default();
        stack.add(tagged(1, "volume-script", "vol"), FIVE);
        stack.add(tagged(2, "other-app", "vol"), FIVE);
        assert_eq!(
            stack.add(tagged(3, "volume-script", "vol"), FIVE),
            Added::Displaced(1)
        );
        assert_eq!(stack.len(), 2);
        assert!(stack.get(1).is_none());
        assert!(
            stack.get(2).is_some(),
            "another application's tag is its own"
        );
    }

    #[test]
    fn a_clock_starts_when_shown_not_when_queued() {
        let mut stack = Stack::default();
        let t0 = Instant::now();
        stack.add(notification(1), FIVE);
        stack.add(notification(2), FIVE);
        // Only one fits; the older waits.
        stack.start_clocks(t0, 1);
        assert_eq!(stack.next_deadline(), Some(t0 + Duration::from_secs(5)));
        assert_eq!(stack.expire(t0 + Duration::from_secs(5)), vec![2]);
        // Now the waiting one is shown, and gets its whole five seconds from here.
        let t1 = t0 + Duration::from_secs(5);
        stack.start_clocks(t1, 1);
        assert!(stack.expire(t1 + Duration::from_secs(4)).is_empty());
        assert_eq!(stack.expire(t1 + Duration::from_secs(5)), vec![1]);
        assert!(stack.is_empty());
    }

    #[test]
    fn until_dismissed_never_expires() {
        let mut stack = Stack::default();
        let t0 = Instant::now();
        stack.add(notification(1), None);
        stack.start_clocks(t0, 5);
        assert_eq!(stack.next_deadline(), None);
        assert!(stack.expire(t0 + Duration::from_secs(86400)).is_empty());
    }

    #[test]
    fn hovering_holds_the_clock_and_gives_back_what_was_left() {
        let mut stack = Stack::default();
        let t0 = Instant::now();
        stack.add(notification(1), FIVE);
        stack.start_clocks(t0, 5);
        stack.hover(Some(1), t0 + Duration::from_secs(3));
        assert_eq!(stack.next_deadline(), None);
        // Read for a minute.
        let back = t0 + Duration::from_secs(63);
        assert!(stack.expire(back).is_empty());
        stack.hover(None, back);
        assert_eq!(stack.next_deadline(), Some(back + Duration::from_secs(2)));
    }

    #[test]
    fn a_hovered_notification_that_never_started_does_not_start_underneath_the_pointer() {
        let mut stack = Stack::default();
        let t0 = Instant::now();
        stack.add(notification(1), FIVE);
        stack.hover(Some(1), t0);
        stack.start_clocks(t0, 5);
        assert_eq!(stack.next_deadline(), None);
        // Leaving starts nothing either; the next drawn frame does.
        stack.hover(None, t0);
        stack.start_clocks(t0, 5);
        assert_eq!(stack.next_deadline(), Some(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn replacing_a_hovered_notification_keeps_it_held() {
        let mut stack = Stack::default();
        let t0 = Instant::now();
        stack.add(notification(1), FIVE);
        stack.start_clocks(t0, 5);
        stack.hover(Some(1), t0);
        stack.add(notification(1), Some(Duration::from_secs(9)));
        assert_eq!(stack.next_deadline(), None);
        stack.hover(None, t0);
        assert_eq!(stack.next_deadline(), Some(t0 + Duration::from_secs(9)));
    }

    #[test]
    fn removing_something_already_gone_is_not_an_error() {
        let mut stack = Stack::default();
        assert!(stack.remove(42).is_none());
        stack.add(notification(1), FIVE);
        assert!(stack.remove(1).is_some());
        assert!(stack.remove(1).is_none());
    }
}
