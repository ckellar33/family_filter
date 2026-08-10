// Wire types shared by every screen -- one place instead of each component
// re-declaring its own copy, since several of these (Cue, PlaybackStatus,
// FilterSummary, CreationCue, Protocol/Step) are used by more than one.

export interface Device {
  host: string;
  port: number;
}

export interface SavedPairingInfo {
  host: string;
  port: number;
  has_mrp: boolean;
  has_airplay: boolean;
}

export interface ControlInfo {
  has_live: boolean;
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
  category: string;
}

export type CategoryKind = "mute" | "skip";
export interface CategoryDef {
  name: string;
  kind: CategoryKind;
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
}

// What tapping a tile loads -- see control::select_filter_tile. `services`
// comes straight from the filter file's own MediaEntry.services -- not a
// live lookup against anything.
export interface FilterEntryDetail {
  title: string;
  categories: string[];
  cues: Cue[];
  services: string[];
}
