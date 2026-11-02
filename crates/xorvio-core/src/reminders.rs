//! Reminders for guide programmes on the watchlist: which are due, what their
//! notifications say, and which already went out. The app shows them; the
//! library remembers them, so a relaunch doesn't send one twice.

use xorvio_model::Timestamp;

use crate::watchlist;
use crate::{Core, Result};

/// A reminder ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    /// The programme's watchlist id.
    pub item_id: String,
    /// The channel to tune to when the notification is clicked.
    pub channel_id: Option<String>,
    /// "Starting in 5 minutes"
    pub title: String,
    /// "Match of the Day on BBC One"
    pub body: String,
}

impl Core {
    /// Reminders due at `now` that haven't gone out: reminded programmes
    /// starting within `lead_seconds`, or that started a moment ago (a Mac
    /// waking from sleep), soonest first.
    pub fn due_reminders(&self, now: Timestamp, lead_seconds: i64) -> Result<Vec<Reminder>> {
        let unsent = self.library.unsent_reminders()?;
        watchlist::reminders_due(&unsent, now, lead_seconds)
            .into_iter()
            .map(|entry| {
                let channel = match entry.channel_id.as_deref() {
                    Some(id) => self.channel(id)?,
                    None => None,
                };
                Ok(Reminder {
                    item_id: entry.id.clone(),
                    channel_id: entry.channel_id.clone(),
                    title: watchlist::reminder_title(entry.start.unwrap_or(now), now),
                    body: match channel {
                        Some(channel) => format!("{} on {}", entry.title, channel.name),
                        None => entry.title.clone(),
                    },
                })
            })
            .collect()
    }

    /// Notes reminders went out, so each is sent once, across relaunches too.
    pub fn mark_reminders_sent(&self, item_ids: &[String], now: Timestamp) -> Result<()> {
        self.library.mark_reminders_sent(item_ids, now)
    }
}
