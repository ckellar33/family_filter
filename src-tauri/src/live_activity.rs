//! Drives the iOS Lock Screen Live Activity and widgets (and, through them,
//! the background keep-alive) from the control heartbeat -- see
//! `control::spawn_heartbeat`. Runs entirely in Rust rather than from the
//! frontend because iOS suspends the webview's JavaScript once the app is
//! backgrounded, which is exactly when the Live Activity is on screen.
//!
//! The Swift side lives in the `tauri-plugin-live-activity` crate
//! (`plugins/live-activity`).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_live_activity::LiveActivityExt;

/// What the Live Activity shows. Serialized straight into the Swift
/// plugin's `UpdateArgs` -- keep the two in sync.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityContent {
    pub title: String,
    pub subtitle: Option<String>,
    pub app_name: Option<String>,
    pub is_filtering: bool,
    pub next_cue_action: Option<String>,
    pub next_cue_category: Option<String>,
    pub next_cue_time: Option<String>,
    /// Wall-clock Unix time (ms) the next cue starts, only while playback is
    /// advancing -- lets the Lock Screen run its own countdown.
    pub next_cue_epoch_ms: Option<f64>,
}

impl ActivityContent {
    /// `next_cue_epoch_ms` is re-derived from a moving position on every
    /// tick, so it jitters slightly; only a real jump (seek, pause, rate
    /// change) is worth an update.
    fn same_as(&self, other: &Self) -> bool {
        let epoch_close = match (self.next_cue_epoch_ms, other.next_cue_epoch_ms) {
            (Some(a), Some(b)) => (a - b).abs() < 1500.0,
            (None, None) => true,
            _ => false,
        };
        epoch_close
            && Self {
                next_cue_epoch_ms: None,
                ..self.clone()
            } == Self {
                next_cue_epoch_ms: None,
                ..other.clone()
            }
    }
}

/// Same format as the frontend's `fmtTime` (src/lib/format.ts) -- "M:SS",
/// or "H:MM:SS" from an hour on -- so the Lock Screen matches the app.
pub fn fmt_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Wall-clock Unix ms for something `seconds_from_now` in the future.
pub fn epoch_ms_in(seconds_from_now: f64) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    (now + seconds_from_now.max(0.0)) * 1000.0
}

/// Re-send unchanged content this often, so the Swift side can push the
/// activity's stale date forward -- if the app stops running, the activity
/// then marks itself stale instead of showing outdated cues indefinitely.
const RESEND_INTERVAL: Duration = Duration::from_secs(60);

/// How long "there should be no activity" has to persist before it's
/// actually ended (which also drops the background keep-alive). Reopening the
/// app reconnects (`app-resumed` -> `control::start_control_session`), which
/// briefly resets auto-filter to off until the frontend restores the saved
/// setting; without this, every app open would end the activity and start a
/// new one. Also rides out a momentary connection drop.
const END_GRACE: Duration = Duration::from_secs(5);

enum Sent {
    /// Nothing sent yet by this syncer -- always send the first state, since
    /// an activity from a previous session may still be on screen.
    Nothing,
    Ended,
    Content(ActivityContent),
}

/// Owned by one heartbeat loop; dedupes so ActivityKit/WidgetKit only hear
/// about real changes.
pub struct LiveActivitySync {
    app: AppHandle,
    sent: Sent,
    sent_at: Instant,
    /// When `sync` started seeing `None`, for `END_GRACE`.
    none_since: Option<Instant>,
}

impl LiveActivitySync {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            sent: Sent::Nothing,
            sent_at: Instant::now(),
            none_since: None,
        }
    }

    /// `None` means there should be no Live Activity (auto-filter off, or no
    /// live connection to the Apple TV).
    pub fn sync(&mut self, content: Option<ActivityContent>) {
        if content.is_some() {
            self.none_since = None;
        } else if self.none_since.get_or_insert_with(Instant::now).elapsed() < END_GRACE {
            return;
        }

        let unchanged = match (&self.sent, &content) {
            (Sent::Ended, None) => true,
            (Sent::Content(prev), Some(next)) => {
                prev.same_as(next) && self.sent_at.elapsed() < RESEND_INTERVAL
            }
            _ => false,
        };
        if unchanged {
            return;
        }

        self.sent = match &content {
            Some(c) => Sent::Content(c.clone()),
            None => Sent::Ended,
        };
        self.sent_at = Instant::now();

        // The plugin call blocks until the Swift side has run on the main
        // thread -- keep that off the async runtime the heartbeat shares.
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = match content {
                Some(c) => app.live_activity().update(&c),
                None => app.live_activity().end(),
            };
            if let Err(e) = result {
                eprintln!("[live-activity] {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(epoch: Option<f64>) -> ActivityContent {
        ActivityContent {
            title: "Chapter 1".into(),
            subtitle: None,
            app_name: None,
            is_filtering: true,
            next_cue_action: Some("SKIP".into()),
            next_cue_category: None,
            next_cue_time: Some("12:34".into()),
            next_cue_epoch_ms: epoch,
        }
    }

    #[test]
    fn fmt_time_matches_frontend_format() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(754.4), "12:34");
        assert_eq!(fmt_time(3600.0), "1:00:00");
        assert_eq!(fmt_time(-5.0), "0:00");
    }

    #[test]
    fn same_as_ignores_countdown_jitter_but_not_jumps() {
        assert!(content(Some(10_000.0)).same_as(&content(Some(11_000.0))));
        assert!(!content(Some(10_000.0)).same_as(&content(Some(20_000.0))));
        assert!(!content(Some(10_000.0)).same_as(&content(None)));
        let mut other = content(Some(10_000.0));
        other.next_cue_action = Some("MUTE".into());
        assert!(!content(Some(10_000.0)).same_as(&other));
    }
}
