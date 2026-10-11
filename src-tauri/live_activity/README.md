# Lock Screen Live Activity and widgets (iOS)

This directory is the iOS widget extension. While **auto-filter is on and an
Apple TV is connected**, Family Filter shows:

- **A Live Activity** on the Lock Screen and in the Dynamic Island. It shows
  what's playing, whether it's being filtered, and the next cue with a live
  countdown:

  ```
  ┌───────────────────────────────────┐
  │ The Mandalorian                   │
  │ Chapter 1                         │
  │ ● Filtering · Disney+             │
  │ ───────────────────────────────── │
  │ [SKIP] in 1:35  Violence          │
  └───────────────────────────────────┘
  ```

- **Widgets** with the same information: Home Screen (small, medium) and Lock
  Screen (rectangular, inline, circular). Add them like any other widget.

The app also **keeps running while the phone is locked** during that time,
so filtering keeps working.

## How it works

```
control::spawn_heartbeat (Rust, every 100 ms)
  ├─ apply_filter: mutes/skips on the Apple TV
  └─ once a second: live_activity_content → LiveActivitySync (src/live_activity.rs)
        only real changes (plus a 60 s refresh) go to the Swift plugin:
plugins/live-activity (Swift, in the app process)
  ├─ ActivityKit: start / update / end the Live Activity
  ├─ App Group: save state + reload widget timelines
  └─ BackgroundKeepAlive: silent, mix-with-others audio
this directory (widget extension process)
  ├─ FamilyFilterLiveActivity.swift: Lock Screen + Dynamic Island views
  └─ FamilyFilterWidget.swift + Provider.swift: widgets, reading the App Group
```

- **Driven from Rust, not the frontend.** iOS suspends the WebView's
  JavaScript in the background, which is exactly when the Lock Screen is
  showing. The existing Rust heartbeat already applied cues independently of
  the WebView, and it now drives the Live Activity too.
- **Keep-alive.** iOS normally suspends a backgrounded app within seconds,
  which would stop filtering and close the Apple TV connection. While a Live
  Activity is running, the app holds the `audio` background mode by playing
  silence, mixed with other audio so it never interrupts or ducks the user's
  own sound. It's released when auto-filter is turned off or the Apple TV
  disconnects (after a 5 s grace period, so reopening the app or a brief Wi-Fi
  drop doesn't restart everything).
- **The countdown is rendered by iOS** (`Text(timerInterval:)`) from the cue's
  wall-clock start time. It keeps ticking with no updates from the app, and
  updates are only sent at cue boundaries or on a seek or pause.
- **Staleness.** The app refreshes the activity at least once a minute, and
  each refresh pushes its stale date three minutes out. If the app is killed,
  the activity turns into "Family Filter stopped. Open the app to resume
  filtering." rather than showing a countdown to a cue that won't be filtered.
- **One shared model.** `FamilyFilterStatus` and
  `FamilyFilterActivityAttributes` live in
  `plugins/live-activity/ios/Sources/FamilyFilterShared.swift`, which is
  compiled into both the app and this extension. ActivityKit requires both
  sides to share the attributes type.
- **ActivityKit only lets an app *start* a Live Activity while it's in the
  foreground.** Turning on auto-filter happens in the app, so that's covered.
  If an activity can't start, it's retried on every update.

## Building: no manual setup

```bash
cargo tauri ios build      # or: cargo tauri ios dev
```

On a fresh clone, or after deleting `src-tauri/gen/`, first run
`cargo tauri ios init` once **from the repo root**.

- The Xcode project, including this extension target, is generated from the
  tracked **`src-tauri/ios/project.yml`** (via `bundle.iOS.template` in
  `tauri.conf.json`). Change project settings there, then re-run
  `cargo tauri ios init`. Never change them in `gen/` or in Xcode's UI.
- This directory is compiled in place as a synced folder: `.swift` files
  added here are picked up automatically. `*.md` files are excluded.
- `Info.ios.plist` declares `NSSupportsLiveActivities` and
  `UIBackgroundModes: audio`.
- **Building from Xcode** only works while `cargo tauri ios dev --open` (or
  `build --open`) is running. Xcode's "Build Rust Code" phase gets its options
  from that process. Otherwise it fails with "Command PhaseScriptExecution
  failed" (`failed to read CLI options … Connection refused`).

Gotchas, also noted in `project.yml`:

- **The template is rendered by Handlebars**, so it must never contain double
  curly braces.
- **The extension target's name must not contain `_iOS`.** tauri-cli finds
  the main app's settings by searching for `_iOS`, and would randomly stamp a
  second match with the app's bundle ID ("Mismatched bundle IDs" on install).
- `bundle.iOS.minimumSystemVersion` (tauri.conf.json) and
  `options.deploymentTarget` (project.yml) must stay in sync. Both are 27.0.

## Files

| File | Role |
|---|---|
| `FamilyFilterExtension.swift` | `@main` widget bundle: the widget + the Live Activity |
| `FamilyFilterLiveActivity.swift` | Lock Screen and Dynamic Island presentation |
| `FamilyFilterWidget.swift` | Home and Lock Screen widget, plus views shared with the Live Activity |
| `Provider.swift` | Widget timeline: reads the App Group |
| `Assets.xcassets` | Widget accent/background colors |

## App Store review

Apple only allows the `audio` background mode for apps that play audible
content for the user. This app uses it to keep filtering running while the
phone is locked, and App Review may reject that. If it does, the fallback is
to drop the keep-alive: the Live Activity would then show the "stopped" state
a few minutes after locking, and filtering would pause until the app is
reopened. Its staleness handling already covers that case.
