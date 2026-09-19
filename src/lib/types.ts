// Wire types shared by every screen -- one place instead of each component
// re-declaring its own copy, since several of these (Cue, PlaybackStatus,
// FilterSummary, CreationCue, Protocol/Step) are used by more than one.

export interface Device {
  host: string;
  port: number;
}

// One saved device, as returned by list_saved_devices -- the Devices
// screen's startup chooser lists these; `id` is what start_control_session/
// verify_saved_pairing/rename_saved_device/delete_saved_device key off of.
export interface SavedDeviceInfo {
  id: string;
  name: string;
  host: string;
  port: number;
  has_mrp: boolean;
  has_airplay: boolean;
}

export interface ControlInfo {
  has_live: boolean;
  // Which saved device this session connected to -- see control::ControlInfo.
  id: string;
  name: string;
  host: string;
}

export interface Cue {
  // Position within the matched entry's cue list -- what
  // set_filter_cue_enabled expects back to identify this cue.
  index: number;
  start: number;
  end: number;
  action: "mute" | "skip";
  category: string;
  // Whether this cue would actually fire right now: both its category
  // and this specific cue are enabled. False either way looks the same
  // to the frontend, so there's no need to track the two separately here.
  enabled: boolean;
  // The actual word/phrase this cue mutes, for a "language" (or
  // "language-*") cue -- e.g. "shit". null for any cue nothing has
  // recorded a word for yet. Shown censored (see censorWord in format.ts)
  // on the Filters tab in place of the plain MUTE pill when present.
  word: string | null;
  // Free-form note on a skip cue -- what the scene actually was ("bar
  // fight, brief"), for whoever reads the list later. Never shown in place
  // of the category, always under it. null for a cue with no note, which
  // is most of them.
  note: string | null;
}

export interface PlaybackStatus {
  title: string | null;
  // The show's name, for a TV episode -- title above is just the
  // episode's own title (e.g. "Chapter 1") in that case. Null for a
  // movie, or anything else the device doesn't report a series for.
  series_name: string | null;
  // Freeform secondary line some apps populate instead of series_name --
  // not guaranteed to be show-related, just whatever the app put there.
  // Only used as a fallback display when series_name is null.
  subtitle: string | null;
  position: number | null;
  duration: number | null;
  playback_state: string;
  // Whether `position` is actually advancing right now -- see
  // session.svelte.ts's `livePosition`, the only consumer: it uses this
  // (not `playback_state` alone, which some apps leave stale on pause) to
  // decide whether to keep interpolating `position` forward between polls.
  is_advancing: boolean;
  // Bundle id of whatever app is currently "now playing" (e.g.
  // "com.netflix.Netflix"), or null until the device has announced one.
  app_bundle_id: string | null;
  // Friendly name for app_bundle_id, or null when it isn't in the
  // backend's (necessarily incomplete) lookup table -- fall back to
  // app_bundle_id itself in that case rather than hiding the app.
  app_name: string | null;
  // Populated whenever a filter list is loaded and its title matches --
  // regardless of whether auto-filter mode is actually turned on -- so
  // the schedule is visible as a preview even while it's off.
  filter_match: string | null;
  filter_cues: Cue[];
  // Only ever set while auto-filter mode is on (nothing is actually
  // applied while it's off).
  filter_action: string | null;
  filter_category: string | null;
}

export interface FilterSummary {
  path: string;
  media_count: number;
  categories: string[];
  // Master auto-filter toggle's state as of this summary -- persisted
  // backend-side (see filter::load_saved_filter_enabled), so a session
  // restored via checkSavedFilter can come back armed instead of always
  // defaulting off. Loading a *different* file (load_filter_file/
  // select_filter_tile) always reports this false, matching the backend's
  // "never silently start muting/skipping" rule.
  enabled: boolean;
}

// Filter *creation* mode -- recording cue timestamps live from playback, as
// opposed to the auto-filter types above, which describe applying an
// already-authored file. Deliberately separate types even though the shapes
// overlap, since a draft cue has no `enabled` (that's an auto-filter-only
// concept).
export interface DraftSummary {
  path: string;
  media_count: number;
}

export interface CreationCue {
  index: number;
  start: number;
  end: number;
  action: "mute" | "skip";
  // Both are label state CueLabelSheet writes through
  // creation_set_cue_label: `category` doubles as a mute cue's *kind* of
  // language ("language-profanity" etc), `word` is the comma-separated
  // word list on such a cue ("shit, damn" -- one 8s window can genuinely
  // catch two), and `note` is the free-text note on a skip cue. null until
  // answered -- a mute cue with no words yet is what makes a Recorded row
  // read as unfinished.
  category: string;
  word: string | null;
  note: string | null;
}

// Returned by creation_mark_mute/creation_end_skip_mark -- the cue that was
// just recorded, plus which title it landed under. `index === 0` means this
// was the first cue ever recorded for that (title, service) pair, i.e. it
// just created a brand-new media entry -- see noteNewEntryIfFirstCue in
// creation.svelte.ts.
export interface CueMarkResult {
  title: string;
  index: number;
  start: number;
  end: number;
  action: "mute" | "skip";
  category: string;
}

export type CategoryKind = "mute" | "skip";
export interface CategoryDef {
  name: string;
  kind: CategoryKind;
}

// One kind of language a mute cue can be -- see LANGUAGE_KINDS in
// creation.svelte.ts. `censor` is false for childish language: grawlixing
// "stupid" makes the list unreadable without protecting anyone.
export interface LanguageKindDef {
  category: string;
  label: string;
  censor: boolean;
  words: string[];
}

// Siri Remote buttons control_button can send -- see control::RemoteButton.
// Deliberately excludes the touchpad's swipe/tap gestures.
export type RemoteButton = "up" | "down" | "left" | "right" | "select" | "menu" | "home" | "play_pause";

export type Protocol = "companion" | "mrp" | "airplay";
export type Step = Protocol | "save" | "done";

// The three bottom-tab-bar destinations once a control session is active.
export type Tab = "controls" | "select-filter" | "create-filter";

// One poster tile in the Select Filter grid -- see control::list_filter_tiles.
export interface FilterTile {
  title: string;
  path: string;
  // `data:` URI, or null when TMDB has no key configured, no match, or the
  // lookup otherwise failed -- render a placeholder tile in that case.
  poster: string | null;
  // How many cues the entry behind this tile carries -- the grid's badge.
  cue_count: number;
}

// What tapping a tile (after resolving which service, see ServiceOption)
// loads -- see control::select_filter_tile.
export interface FilterEntryDetail {
  title: string;
  service: string;
  categories: string[];
  // Which of `categories` are currently off -- the real backend state, not
  // an assumption; see control::FilterEntryDetail's doc comment.
  disabled_categories: string[];
  cues: Cue[];
  // Master auto-filter toggle's actual current state -- mirror this rather
  // than assuming it just got turned off by opening this detail view.
  enabled: boolean;
}

// One service variant of a title, found anywhere in the library -- see
// control::list_services_for_title. `path` is which file it lives in.
export interface ServiceOption {
  service: string;
  path: string;
}

// One tile in the Select Filter screen's "Online" grid -- see
// online::list_online_filters. Deduped by title exactly like FilterTile (a
// title recorded on more than one service gets one poster, not one per
// service) -- `media` carries every service variant's raw MediaEntry JSON
// for this title, opaque here, round-tripped straight back to
// downloadOnlineFilter so the backend doesn't need a second network round
// trip to fetch what it already just sent down. Unlike FilterTile there's
// no `path` -- nothing on disk yet for an entry nobody's downloaded.
export interface OnlineFilterTile {
  title: string;
  poster: string | null;
  cue_count: number;
  media: unknown[];
}
