// Filter *creation* mode: recording cue timestamps live from playback,
// as opposed to filter.svelte.ts's auto-filter state, which describes
// applying an already-authored file. Deliberately separate even though the
// shapes overlap, since a draft cue has no `enabled` (that's an
// auto-filter-only concept) -- the draft being authored here is never the
// list actively muting/skipping mid-movie unless it's separately loaded as
// the active filter from the Select Filter tab.
//
// The recording flow is mark-then-label (design 4a): only two actions
// exist while something is playing -- Skip (press at the start, press again
// at the end) and Mute (stamps a fixed window at the tap). Both land a cue
// on disk immediately with a placeholder label, then CueLabelSheet asks
// what it *was*. That ordering is the whole point: the timestamp is frozen
// by the tap, so no clock keeps running while the answer is being typed,
// and a mis-timed mark can't be caused by deliberation.
import { invoke } from "@tauri-apps/api/core";
import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { session } from "$lib/state/session.svelte";
import { filterState, deleteFilterFile } from "$lib/state/filter.svelte";
import { slugifyTitle } from "$lib/format";
import type { CreationCue, CueMarkResult, DraftSummary, LanguageKindDef } from "$lib/types";

// What a skip cue can be *about*. Fixed list rather than the old
// user-extensible one: the label sheet is a grid of chips, and a category
// that only exists on one machine can't be shared with the filter file.
// Stored lowercase, matching the category strings already written by
// earlier versions and by the sample filter files.
export const SKIP_CATEGORIES: string[] = [
  "violence",
  "gore",
  "nudity",
  "intimacy",
  "drugs & alcohol",
  "frightening",
  "crude humor",
  "peril",
];

// Mute *is* language -- the question a mute mark asks is which kind, since
// each kind carries its own word list and families differ in whether they
// even want censoring. The `category` strings stay in the "language-*"
// shape `filter::Cue::word` and `censorWord` already anticipate, so an
// older file's plain "language" cue keeps working (see languageKindFor,
// which falls back to profanity for it).
export const LANGUAGE_KINDS: LanguageKindDef[] = [
  {
    category: "language-profanity",
    label: "Profanity",
    censor: true,
    words: ["shit", "fuck", "ass", "bitch", "damn", "bastard", "hell"],
  },
  {
    category: "language-blasphemy",
    label: "Blasphemy",
    censor: true,
    words: ["god", "god damn", "jesus", "jesus christ", "oh my god"],
  },
  {
    // Not censored: the point of flagging "stupid" or "shut up" is that a
    // parent can read the list and decide, and grawlixing a word that
    // isn't itself offensive just makes the list unreadable.
    category: "language-childish",
    label: "Childish language",
    censor: false,
    words: ["stupid", "shut up", "dumb", "idiot", "jerk", "I hate you"],
  },
];

// Every category string a mute cue can carry, including the bare
// "language" written by versions predating the kinds.
export function isLanguageCategory(category: string): boolean {
  return category === "language" || category.startsWith("language-");
}

// Falls back to Profanity for a bare "language" cue (and for an unknown
// "language-*" value from a hand-edited file) rather than returning null
// and leaving the sheet with nothing to show.
export function languageKindFor(category: string): LanguageKindDef {
  return LANGUAGE_KINDS.find((k) => k.category === category) ?? LANGUAGE_KINDS[0];
}

// Placeholder a skip mark is recorded under between the two presses --
// replaced by the real category the moment the sheet is saved. It only
// ever reaches disk if the app dies between the second press and the save,
// which is exactly when a visibly-unlabeled cue beats a lost one.
export const UNLABELED_SKIP = "unlabeled";

// A mute mark's fixed window, in seconds -- mirrors the backend's
// MUTE_MARK_SECS default so the button can say how long it stamps.
export const MUTE_MARK_SECS = 8;

export const creationState = $state({
  // "idle" until a draft is started/opened, then "recording" while the two
  // mark buttons + the recorded list are shown.
  stage: "idle" as "idle" | "recording",
  draft: null as DraftSummary | null,
  cues: [] as CreationCue[],

  // Set between the two presses of Skip. `pendingSkipStart` is the frozen
  // position of the first press, kept frontend-side purely so the button
  // can show where the mark began (the authoritative copy lives in the
  // backend's PendingSkip).
  skipPending: false,
  pendingSkipStart: null as number | null,

  busy: false,
  error: "",

  // Text field for correcting a mis-detected service (see renameService) --
  // not the service itself, which is derived live from whatever's playing
  // (see currentService below), just this form's own input value.
  renameServiceInput: "",
});

// Which service's entry cue marks are currently landing in -- whatever app
// is "now playing" per the live poll, same source `creation_mark_mute`/
// `creation_end_skip_mark` derive it from server-side (see
// `creation::service_hint`). `""` (the generic/unspecified entry) when
// nothing's playing or the app isn't recognized, matching the backend's own
// fallback.
export function currentService(): string {
  return session.playback?.app_name ?? "";
}

// Backend's `start_control_session` rebuilds ControlState from scratch --
// including creation-mode's draft -- so the frontend's view of it needs
// resetting alongside filter.svelte.ts's checkSavedFilter() whenever that
// happens (see +page.svelte).
export function resetCreation() {
  creationState.stage = "idle";
  creationState.draft = null;
  creationState.cues = [];
  creationState.renameServiceInput = "";
  creationState.skipPending = false;
  creationState.pendingSkipStart = null;
  creationState.error = "";
}

export async function pickNewDraft() {
  creationState.error = "";
  try {
    // Defaults to whatever's on screen right now (e.g. "The Princess
    // Bride" -> "the-princess-bride.json") so the dialog doesn't always
    // open on the generic "filter.json" -- still just a suggestion, the
    // dialog lets it be edited/overridden before saving.
    const defaultPath = session.playback?.title ? `${slugifyTitle(session.playback.title)}.json` : "filter.json";

    // On iOS there's no dialog to show at all -- see
    // control::supports_save_location_picker's doc -- creation_new_draft
    // picks a path under the app's own storage instead when `path` is
    // omitted, using `defaultPath` as just a suggested filename.
    let path: string | null = null;
    if (filterState.saveLocationPickerSupported) {
      path = await saveDialog({ filters: [{ name: "Filter list", extensions: ["json"] }], defaultPath });
      if (!path) return; // user cancelled
    }
    creationState.draft = await invoke<DraftSummary>("creation_new_draft", { path, suggestedName: defaultPath });
    creationState.stage = "recording";
    creationState.cues = [];
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function pickExistingDraft() {
  creationState.error = "";
  try {
    const path = await open({ multiple: false, filters: [{ name: "Filter list", extensions: ["json"] }] });
    if (!path || Array.isArray(path)) return; // user cancelled
    creationState.draft = await invoke<DraftSummary>("creation_open_draft", { path });
    creationState.stage = "recording";
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  }
}

// Re-fetches the draft's cues for whatever's currently playing (on whatever
// service it's playing on) -- called after every mutation, and reactively
// (see CreateFilterPage.svelte) whenever the title or app changes while
// recording, so the list always reflects what's actually on screen.
export async function refreshCreationCues() {
  if (!session.playback?.title) {
    creationState.cues = [];
    return;
  }
  try {
    creationState.cues = await invoke<CreationCue[]>("creation_list_cues", { title: session.playback.title, service: currentService() });
  } catch (e) {
    creationState.error = String(e);
  }
}

// Corrects the current title's service -- e.g. the auto-tagging landed it
// as the generic entry because this build doesn't recognize the app, or it
// just guessed wrong. Renames *from* whatever's playing right now (the
// entry recording is actually landing in) *to* the typed-in name.
//
// Best used once you're done recording that title for this session: future
// marks still land wherever the *live* detection says (see
// currentService()), which no longer matches the renamed entry -- they'd
// start a fresh entry under the old (e.g. generic) name rather than
// continuing to append to the one just renamed.
export async function renameService() {
  const newService = creationState.renameServiceInput.trim();
  if (!newService || !session.playback?.title) return;
  creationState.error = "";
  try {
    await invoke("creation_set_service", { title: session.playback.title, oldService: currentService(), newService });
    creationState.renameServiceInput = "";
    // The entry just moved out from under currentService() -- refresh so
    // the list honestly reflects that this (still-generic, if the app
    // remains unrecognized) service now has no cues of its own.
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  }
}

// `creation_mark_mute`/`creation_end_skip_mark` create a media entry
// on-the-fly the first time a title+service is marked (see
// `filter::FilterList::entry_mut`), but `creationState.draft.media_count`
// is only ever set once, when the draft is opened/started -- it never
// reflects titles recorded since. `index === 0` on the returned cue means
// this mark just created that entry (a fresh `MediaEntry` always starts
// with an empty `cues` vec, so its first cue always lands at index 0), so
// bump the count then -- otherwise the draft card keeps showing "0 titles"
// even once marks are actually landing on disk, which reads as "nothing
// got saved".
function noteNewEntryIfFirstCue(result: CueMarkResult) {
  if (result.index === 0 && creationState.draft) {
    creationState.draft.media_count += 1;
  }
}

// Both mark paths return the cue that just landed, so the caller can open
// the label sheet on it. `null` means the mark failed (the error is already
// in creationState.error) and no sheet should open.

// Stamps the fixed mute window at the current position, under the bare
// "language" category -- the *kind* of language is what the sheet then
// asks for, and until it's answered the cue is honestly labeled as
// unspecified language rather than guessed at.
export async function markMute(): Promise<CueMarkResult | null> {
  creationState.busy = true;
  creationState.error = "";
  try {
    const result = await invoke<CueMarkResult>("creation_mark_mute", { category: "language" });
    noteNewEntryIfFirstCue(result);
    await refreshCreationCues();
    return result;
  } catch (e) {
    creationState.error = String(e);
    return null;
  } finally {
    creationState.busy = false;
  }
}

// First press starts the mark, second press ends it and returns the cue --
// same press-to-start/press-to-end rule as before, just on one button now
// that the category isn't chosen up front.
export async function startSkipMark() {
  creationState.busy = true;
  creationState.error = "";
  try {
    await invoke("creation_start_skip_mark", { category: UNLABELED_SKIP });
    creationState.skipPending = true;
    creationState.pendingSkipStart = session.playback?.position ?? null;
  } catch (e) {
    creationState.error = String(e);
  } finally {
    creationState.busy = false;
  }
}

export async function endSkipMark(): Promise<CueMarkResult | null> {
  creationState.busy = true;
  creationState.error = "";
  try {
    const result = await invoke<CueMarkResult>("creation_end_skip_mark");
    noteNewEntryIfFirstCue(result);
    creationState.skipPending = false;
    creationState.pendingSkipStart = null;
    await refreshCreationCues();
    return result;
  } catch (e) {
    creationState.error = String(e);
    return null;
  } finally {
    creationState.busy = false;
  }
}

export async function cancelSkipMark() {
  creationState.busy = true;
  creationState.error = "";
  try {
    await invoke("creation_cancel_skip_mark");
    creationState.skipPending = false;
    creationState.pendingSkipStart = null;
  } catch (e) {
    creationState.error = String(e);
  } finally {
    creationState.busy = false;
  }
}

// What the label sheet commits: the category (a skip category, or a
// "language-*" kind), the optional note, and -- for a mute cue whose words
// have been picked -- the words themselves. Every field is optional
// server-side; omitting one leaves it as it was, so the sheet can save a
// kind change without touching the words, and vice versa.
export async function setCueLabel(
  cue: CreationCue,
  label: { category?: string; note?: string; word?: string },
) {
  if (!session.playback?.title) return;
  creationState.busy = true;
  creationState.error = "";
  try {
    await invoke("creation_set_cue_label", {
      title: session.playback.title,
      service: currentService(),
      index: cue.index,
      category: label.category ?? null,
      note: label.note ?? null,
      word: label.word ?? null,
    });
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  } finally {
    creationState.busy = false;
  }
}

// Takes both `start` and `end` together (the sheet's full draft) in one
// call -- see filter.svelte.ts's updateDetailCueTime for why calling this
// twice, once per changed field, used to silently revert whichever field
// was set first whenever both changed in the same edit.
export async function updateCueTime(cue: CreationCue, start: number, end: number) {
  if (!session.playback?.title) return;
  creationState.error = "";
  try {
    await invoke("creation_update_cue", { title: session.playback.title, service: currentService(), index: cue.index, start, end });
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function deleteCue(cue: CreationCue) {
  if (!session.playback?.title) return;
  creationState.error = "";
  try {
    await invoke("creation_delete_cue", { title: session.playback.title, service: currentService(), index: cue.index });
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  }
}

// The "Delete" button's action: permanently removes the draft's own file
// from disk (same delete_filter_file command SelectFilterPage's "On this
// device" delete uses), as opposed to Save/resetCreation, which just ends
// the recording session and leaves the file -- and everything already
// written to it -- in place. Leaves the session open on failure (deleteFilterFile
// never throws; it reports into filterState.tilesError instead) so the error
// banner has something to point at and the draft isn't silently abandoned.
export async function deleteDraft() {
  if (!creationState.draft) return;
  creationState.busy = true;
  creationState.error = "";
  filterState.tilesError = "";
  try {
    await deleteFilterFile(creationState.draft.path);
    if (filterState.tilesError) {
      creationState.error = filterState.tilesError;
      return;
    }
    resetCreation();
  } finally {
    creationState.busy = false;
  }
}
