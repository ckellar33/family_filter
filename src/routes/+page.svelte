<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";

  interface Device {
    host: string;
    port: number;
  }

  interface SavedPairingInfo {
    host: string;
    port: number;
    has_mrp: boolean;
    has_airplay: boolean;
  }

  interface ControlInfo {
    has_live: boolean;
  }

  interface Cue {
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

  interface PlaybackStatus {
    title: string | null;
    position: number | null;
    duration: number | null;
    playback_state: string;
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

  interface FilterSummary {
    path: string;
    media_count: number;
    categories: string[];
  }

  // Filter *creation* mode -- recording cue timestamps live from playback,
  // as opposed to the auto-filter types above, which describe applying an
  // already-authored file. Deliberately separate types even though the
  // shapes overlap, since a draft cue has no `enabled` (that's an
  // auto-filter-only concept).
  interface DraftSummary {
    path: string;
    media_count: number;
  }

  interface CreationCue {
    index: number;
    start: number;
    end: number;
    action: "mute" | "skip";
    category: string;
  }

  type CategoryKind = "mute" | "skip";
  interface CategoryDef {
    name: string;
    kind: CategoryKind;
  }

  type Protocol = "companion" | "mrp" | "airplay";
  type Step = Protocol | "save" | "done";

  // Companion is required (it's what unlocks mute/skip control); MRP and
  // AirPlay are each their own optional pairing ceremony against their own
  // discovered device, needed only for live playback position. Mirrors
  // libs/appletv-cli's pair_flow().
  const STEPS: Step[] = ["companion", "mrp", "airplay", "save", "done"];
  const PROTOCOL_LABEL: Record<Protocol, string> = {
    companion: "Companion",
    mrp: "MRP",
    airplay: "AirPlay",
  };

  // Top-level page: "checking" while we ask the backend for a saved
  // pairing.store on mount, "saved" if one was found (offer to verify it
  // instead of re-pairing from scratch), "wizard" for the discover-and-pair
  // flow -- either because there was nothing saved, or the user chose to
  // pair a different device anyway -- and "control" once a control session
  // is open (mute/unmute, skip, now playing).
  type Page = "checking" | "saved" | "wizard" | "control";
  let page = $state<Page>("checking");
  let savedPairing = $state<SavedPairingInfo | null>(null);
  let verifying = $state(false);
  let verifyResult = $state<"ok" | "failed" | null>(null);
  let verifyError = $state("");

  let hasLive = $state(false);
  let playback = $state<PlaybackStatus | null>(null);
  let controlBusy = $state(false);
  let controlError = $state("");

  // Auto-filter mode: a loaded cue file (filterSummary), the master on/off
  // toggle, and per-category on/off state keyed by category name -- absent
  // from the map (or true) means enabled, matching the backend's
  // disabled_categories-is-the-exception-list convention.
  let filterSummary = $state<FilterSummary | null>(null);
  let filterEnabled = $state(false);
  let categoryEnabled = $state<Record<string, boolean>>({});
  let filterBusy = $state(false);
  let filterError = $state("");

  // Filter creation mode: "idle" until a draft is started/opened, then
  // "recording" while category buttons + the cue table are shown. Separate
  // from filterSummary/filterEnabled above on purpose -- the draft being
  // authored here is never the list actively muting/skipping mid-movie
  // unless the user explicitly arms it via useDraftAsActiveFilter().
  const DEFAULT_CATEGORIES: CategoryDef[] = [
    { name: "language", kind: "mute" },
    { name: "violence", kind: "skip" },
    { name: "gore", kind: "skip" },
    { name: "nudity", kind: "skip" },
    { name: "peril", kind: "skip" },
  ];
  let creationStage = $state<"idle" | "recording">("idle");
  let draft = $state<DraftSummary | null>(null);
  let categories = $state<CategoryDef[]>(DEFAULT_CATEGORIES);
  let creationCues = $state<CreationCue[]>([]);
  let pendingSkipCategory = $state<string | null>(null);
  let creationBusy = $state(false);
  let creationError = $state("");
  let newCategoryName = $state("");
  let newCategoryKind = $state<CategoryKind>("skip");

  // The single soonest cue that hasn't fully passed yet and would actually
  // fire (category + individual toggle both on) -- what shows up next to
  // the playback position, rather than the whole schedule. filter_cues
  // already arrives sorted by start, so the first match is the soonest one.
  let nextCue = $derived.by(() => {
    const p = playback;
    if (!p) return null;
    return p.filter_cues.find((c) => c.enabled && (p.position == null || c.end > p.position)) ?? null;
  });

  // The currently-matched title's cues, grouped by category, for the
  // categories-as-a-tree view -- each category is expandable to show (and
  // individually toggle) just its own cues.
  let cuesByCategory = $derived.by(() => {
    const grouped: Record<string, Cue[]> = {};
    for (const cue of playback?.filter_cues ?? []) {
      (grouped[cue.category] ??= []).push(cue);
    }
    return grouped;
  });

  // Which categories are expanded in the tree. Absent means "default" --
  // expanded whenever there's something to show, so the tree opens up
  // ready-to-read rather than making you click through every category
  // after loading a file or matching a new title.
  let expandedCategories = $state<Record<string, boolean>>({});

  function toggleExpanded(category: string) {
    expandedCategories = { ...expandedCategories, [category]: !(expandedCategories[category] ?? true) };
  }

  let step = $state<Step>("companion");
  let devices = $state<Device[]>([]);
  let scanning = $state(false);
  let pairing = $state(false);
  let error = $state("");

  // Set once the backend's `pin-requested` event fires for the protocol
  // currently being paired -- i.e. the on-screen code is now showing on the
  // Apple TV and the backend is awaiting `submit_pin`.
  let awaitingPinFor = $state<Protocol | null>(null);
  let pin = $state("");

  // Purely cosmetic label for the sticky nav bar -- doesn't drive any
  // behavior, just names whichever screen `page`/`step` currently show.
  let navTitle = $derived.by(() => {
    if (page === "checking") return "Family Filter";
    if (page === "saved") return "Saved Pairing";
    if (page === "control") return "Control";
    if (step === "save") return "Save Pairing";
    if (step === "done") return "Paired";
    return "Pair an Apple TV";
  });

  function isProtocol(s: Step): s is Protocol {
    return s === "companion" || s === "mrp" || s === "airplay";
  }

  // Runs once on mount (no reactive dependencies) to decide the starting
  // page: straight into the wizard if there's nothing saved yet, or the
  // saved-pairing screen if libs/appletv-cli (or an earlier run of this
  // app) already produced a pairing.store.
  $effect(() => {
    checkSaved();
  });

  async function checkSaved() {
    try {
      const found = await invoke<SavedPairingInfo | null>("check_saved_pairing");
      if (found) {
        savedPairing = found;
        page = "saved";
      } else {
        page = "wizard";
      }
    } catch (e) {
      // Backend couldn't even check -- fall through to the normal pairing
      // wizard rather than get stuck on "checking...".
      error = String(e);
      page = "wizard";
    }
  }

  async function verifySaved() {
    verifying = true;
    verifyResult = null;
    verifyError = "";
    try {
      await invoke("verify_saved_pairing");
      verifyResult = "ok";
    } catch (e) {
      verifyResult = "failed";
      verifyError = String(e);
    } finally {
      verifying = false;
    }
  }

  // Runs Pair-Verify + bootstraps a Companion control session (plus a live
  // MRP/AirPlay session if one was paired), then switches to the control
  // page. Available from both the saved-pairing screen and right after a
  // fresh pairing finishes.
  async function openControls() {
    error = "";
    try {
      const info = await invoke<ControlInfo>("start_control_session");
      hasLive = info.has_live;
      playback = null;
      controlError = "";
      page = "control";
      // start_control_session replaces the backend's whole ControlState, so
      // any previously loaded filter list -- and creation-mode draft -- is
      // gone too. Re-check for a saved filter now that the fresh state
      // exists, same as checkSaved() does for the pairing itself on mount,
      // and drop the frontend's now-stale view of the draft.
      await checkSavedFilter();
      resetCreation();
    } catch (e) {
      error = String(e);
    }
  }

  // Tries to reload whatever filter file was last picked (persisted
  // backend-side). Leaves the mode off either way -- loading a list must
  // never silently start auto-muting/skipping.
  async function checkSavedFilter() {
    filterSummary = null;
    filterEnabled = false;
    categoryEnabled = {};
    filterError = "";
    try {
      const found = await invoke<FilterSummary | null>("check_saved_filter_file");
      if (found) {
        filterSummary = found;
        categoryEnabled = Object.fromEntries(found.categories.map((c) => [c, true]));
      }
    } catch (e) {
      filterError = String(e);
    }
  }

  async function pickFilterFile() {
    filterBusy = true;
    filterError = "";
    try {
      const path = await open({ multiple: false, filters: [{ name: "Filter list", extensions: ["json"] }] });
      if (!path || Array.isArray(path)) return; // user cancelled
      const summary = await invoke<FilterSummary>("load_filter_file", { path });
      filterSummary = summary;
      filterEnabled = false;
      categoryEnabled = Object.fromEntries(summary.categories.map((c) => [c, true]));
    } catch (e) {
      filterError = String(e);
    } finally {
      filterBusy = false;
    }
  }

  async function toggleFilterEnabled() {
    const next = !filterEnabled;
    filterBusy = true;
    filterError = "";
    try {
      await invoke("set_filter_enabled", { enabled: next });
      filterEnabled = next;
    } catch (e) {
      filterError = String(e);
    } finally {
      filterBusy = false;
    }
  }

  async function toggleCategory(category: string) {
    const next = !categoryEnabled[category];
    filterError = "";
    try {
      await invoke("set_filter_category_enabled", { category, enabled: next });
      categoryEnabled = { ...categoryEnabled, [category]: next };
      // A category flip can also change which cue is "next" -- refresh now
      // rather than waiting up to a second for the poll to catch up.
      await refreshPlayback();
    } catch (e) {
      filterError = String(e);
    }
  }

  // Unlike categoryEnabled, per-cue enabled state isn't tracked separately
  // client-side -- `playback.filter_cues[i].enabled` already comes back
  // fresh from the backend every poll, so that's the single source of
  // truth; toggling just tells the backend and re-polls immediately.
  async function toggleCue(cue: Cue) {
    if (!playback?.filter_match) return;
    filterError = "";
    try {
      await invoke("set_filter_cue_enabled", { title: playback.filter_match, index: cue.index, enabled: !cue.enabled });
      await refreshPlayback();
    } catch (e) {
      filterError = String(e);
    }
  }

  // Backend's `start_control_session` rebuilds ControlState from scratch --
  // including creation-mode's draft -- so the frontend's view of it needs
  // resetting alongside checkSavedFilter() whenever that happens.
  function resetCreation() {
    creationStage = "idle";
    draft = null;
    creationCues = [];
    pendingSkipCategory = null;
    creationError = "";
  }

  async function pickNewDraft() {
    creationError = "";
    try {
      const path = await saveDialog({ filters: [{ name: "Filter list", extensions: ["json"] }], defaultPath: "filter.json" });
      if (!path) return; // user cancelled
      draft = await invoke<DraftSummary>("creation_new_draft", { path });
      creationStage = "recording";
      creationCues = [];
    } catch (e) {
      creationError = String(e);
    }
  }

  async function pickExistingDraft() {
    creationError = "";
    try {
      const path = await open({ multiple: false, filters: [{ name: "Filter list", extensions: ["json"] }] });
      if (!path || Array.isArray(path)) return; // user cancelled
      draft = await invoke<DraftSummary>("creation_open_draft", { path });
      creationStage = "recording";
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    }
  }

  // Re-fetches the draft's cues for whatever's currently playing -- called
  // after every mutation, and reactively (see the $effect below) whenever
  // the title changes while recording, so the table always reflects the
  // title actually on screen.
  async function refreshCreationCues() {
    if (!playback?.title) {
      creationCues = [];
      return;
    }
    try {
      creationCues = await invoke<CreationCue[]>("creation_list_cues", { title: playback.title });
    } catch (e) {
      creationError = String(e);
    }
  }

  $effect(() => {
    if (creationStage === "recording") {
      refreshCreationCues();
    }
  });

  async function markMute(category: string) {
    creationBusy = true;
    creationError = "";
    try {
      await invoke("creation_mark_mute", { category });
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    } finally {
      creationBusy = false;
    }
  }

  // First press on a skip-category button starts its mark; the second
  // press on that *same* button ends it. Other skip buttons are disabled
  // in the markup while pendingSkipCategory is set, so the "already
  // recording a different category" branch below is a safety net, not the
  // normal path.
  async function toggleSkipMark(category: string) {
    creationBusy = true;
    creationError = "";
    try {
      if (pendingSkipCategory === category) {
        await invoke("creation_end_skip_mark");
        pendingSkipCategory = null;
        await refreshCreationCues();
      } else if (pendingSkipCategory === null) {
        await invoke("creation_start_skip_mark", { category });
        pendingSkipCategory = category;
      }
    } catch (e) {
      creationError = String(e);
    } finally {
      creationBusy = false;
    }
  }

  async function cancelSkipMark() {
    creationBusy = true;
    creationError = "";
    try {
      await invoke("creation_cancel_skip_mark");
      pendingSkipCategory = null;
    } catch (e) {
      creationError = String(e);
    } finally {
      creationBusy = false;
    }
  }

  // Pairs with fmtTime -- parses what the cue table's inputs display back
  // into seconds, or null for anything unrecognized so the caller can
  // reject the edit without touching the backend. Accepts both the m:ss
  // fmtTime shows under an hour and the h:mm:ss it shows at/past an hour,
  // so typing over a displayed value round-trips either way.
  function parseTime(text: string): number | null {
    const m = /^(\d+):([0-5]?\d)(?::([0-5]?\d))?$/.exec(text.trim());
    if (!m) return null;
    if (m[3] != null) {
      return Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]);
    }
    return Number(m[1]) * 60 + Number(m[2]);
  }

  async function updateCueTime(cue: CreationCue, field: "start" | "end", text: string) {
    const seconds = parseTime(text);
    if (seconds == null) {
      creationError = `"${text}" isn't a valid m:ss time`;
      return;
    }
    if (!playback?.title) return;
    creationError = "";
    try {
      const start = field === "start" ? seconds : cue.start;
      const end = field === "end" ? seconds : cue.end;
      await invoke("creation_update_cue", { title: playback.title, index: cue.index, start, end });
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    }
  }

  async function deleteCue(cue: CreationCue) {
    if (!playback?.title) return;
    creationError = "";
    try {
      await invoke("creation_delete_cue", { title: playback.title, index: cue.index });
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    }
  }

  function addCustomCategory() {
    const name = newCategoryName.trim();
    if (!name || categories.some((c) => c.name === name)) return;
    categories = [...categories, { name, kind: newCategoryKind }];
    newCategoryName = "";
  }

  // Reloads the draft's own file into the (separate) auto-filter list, so
  // cues just recorded can be tried out live without manually re-picking
  // the file via "Load a different filter file…".
  async function useDraftAsActiveFilter() {
    if (!draft) return;
    filterBusy = true;
    filterError = "";
    try {
      const summary = await invoke<FilterSummary>("load_filter_file", { path: draft.path });
      filterSummary = summary;
      filterEnabled = false;
      categoryEnabled = Object.fromEntries(summary.categories.map((c) => [c, true]));
    } catch (e) {
      filterError = String(e);
    } finally {
      filterBusy = false;
    }
  }

  async function doSkip(seconds: number) {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_skip", { seconds });
      // Backend actively re-fetches on every control_playback_status call
      // now, but that's on the next 1s poll tick -- fetch immediately too
      // so the display catches up to the skip right away.
      await refreshPlayback();
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  async function doMute() {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_mute");
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  async function doUnmute() {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_unmute");
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  // Fetches one playback snapshot -- errors propagate to the caller rather
  // than being handled here, since callers (doSkip, the periodic poll)
  // each have their own busy/error state to manage around it.
  async function refreshPlayback() {
    playback = await invoke<PlaybackStatus | null>("control_playback_status");
  }

  // Polls now-playing status once a second while the control page is open
  // and a live (MRP/AirPlay) transport is available -- there's nothing to
  // poll otherwise, since title/position only ever come from that
  // transport, not Companion. The backend actively re-fetches on every
  // call (see control_playback_status), so this alone keeps the display
  // in sync with skips and remote-triggered pauses/seeks automatically.
  $effect(() => {
    if (page !== "control" || !hasLive) return;
    const id = setInterval(async () => {
      try {
        await refreshPlayback();
      } catch (e) {
        controlError = String(e);
      }
    }, 1000);
    return () => clearInterval(id);
  });

  // m:ss under an hour (movies' cue timestamps are usually well under
  // that); h:mm:ss once the position/duration/cue rolls past 60 minutes,
  // so a 2-hour movie reads as "2:05:33" rather than a confusing "125:33".
  function fmtTime(seconds: number | null | undefined): string {
    if (seconds == null) return "--:--";
    const total = Math.max(0, Math.round(seconds));
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = total % 60;
    if (h > 0) {
      return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
    }
    return `${m}:${s.toString().padStart(2, "0")}`;
  }

  async function scan(protocol: Protocol) {
    scanning = true;
    error = "";
    devices = [];
    try {
      devices = await invoke<Device[]>("discover_devices", { protocol });
      if (devices.length === 0) {
        error = `No ${PROTOCOL_LABEL[protocol]} devices found on the network.`;
      }
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  // Re-scan automatically whenever the wizard lands on a new discovery
  // step -- but only once we're actually on the wizard page (not while
  // still checking for, or looking at, a saved pairing).
  // This is called when step or page is changed.
  $effect(() => {
    if (page === "wizard" && isProtocol(step)) {
      scan(step);
    }
  });

  async function pair(protocol: Protocol, device: Device) {
    pairing = true;
    error = "";
    let unlisten: UnlistenFn | undefined;
    try {
      // Must be listening *before* invoking -- the backend can fire
      // pin-requested partway through the still-pending invoke() call.
      unlisten = await listen<Protocol>("pin-requested", (event) => {
        if (event.payload === protocol) {
          awaitingPinFor = protocol;
        }
      });
      await invoke(`pair_${protocol}`, { host: device.host, port: device.port });
      advance();
    } catch (e) {
      error = String(e);
    } finally {
      unlisten?.();
      awaitingPinFor = null;
      pin = "";
      pairing = false;
    }
  }

  async function submitPin() {
    if (!awaitingPinFor) return;
    try {
      await invoke("submit_pin", { protocol: awaitingPinFor, pin });
    } catch (e) {
      error = String(e);
    }
  }

  function advance() {
    const i = STEPS.indexOf(step);
    step = STEPS[i + 1];
  }

  function skip() {
    error = "";
    advance();
  }

  async function save() {
    pairing = true;
    error = "";
    try {
      await invoke("finish_pairing");
      step = "done";
    } catch (e) {
      error = String(e);
    } finally {
      pairing = false;
    }
  }
</script>

<div class="phone-shell">
  <main class="canvas">
    <header class="navbar">
      <h1>{navTitle}</h1>
    </header>

    <div class="content">
      {#if page === "checking"}
        <p class="hint centered">Checking for a saved pairing…</p>
      {:else if page === "saved" && savedPairing}
        <section class="screen">
          <p class="section-header">Device</p>
          <ul class="list">
            <li class="list-row static">
              <code>{savedPairing.host}:{savedPairing.port}</code>
            </li>
            <li class="list-row static">
              <span>MRP</span>
              <span class="value">{savedPairing.has_mrp ? "Paired" : "Not paired"}</span>
            </li>
            <li class="list-row static">
              <span>AirPlay</span>
              <span class="value">{savedPairing.has_airplay ? "Paired" : "Not paired"}</span>
            </li>
          </ul>

          {#if verifyResult === "ok"}
            <p class="banner success">✅ Verified — this pairing is still valid.</p>
          {:else if verifyResult === "failed"}
            <p class="banner error">{verifyError}</p>
          {/if}

          <div class="stack">
            <button class="btn-primary" onclick={openControls} disabled={verifying}>Open Controls</button>
            <button class="btn-secondary" onclick={verifySaved} disabled={verifying}>{verifying ? "Verifying…" : "Verify Pairing"}</button>
            <button class="btn-secondary" onclick={() => { page = "wizard"; }} disabled={verifying}>Pair a Different Device</button>
          </div>
        </section>
      {:else if page === "control"}
        <section class="screen">
          {#if controlError}
            <p class="banner error">{controlError}</p>
          {/if}

          {#if hasLive}
            <div class="now-playing">
              <p class="title">{playback?.title ?? "Nothing Playing"}</p>
              {#if playback}
                <p class="subtitle">{playback.playback_state}</p>
              {/if}
              {#if playback?.duration}
                {@const pct = playback.position != null ? Math.min(100, (playback.position / playback.duration) * 100) : 0}
                <div class="progress-track"><div class="progress-fill" style={`width: ${pct}%`}></div></div>
              {/if}
              <p class="position">{fmtTime(playback?.position)} / {fmtTime(playback?.duration)}</p>
              {#if filterSummary && playback}
                <p class="hint centered">
                  {#if playback.filter_action}
                    🛡️ {playback.filter_action} — {playback.filter_category}
                  {:else if !playback.filter_match}
                    no filter list for this title
                  {:else if nextCue}
                    🛡️ next: {nextCue.action === "mute" ? "🔇 mute" : "⏭️ skip"} at {fmtTime(nextCue.start)}–{fmtTime(nextCue.end)} — {nextCue.category}
                    {#if !filterEnabled}(mode off){/if}
                  {:else if playback.filter_cues.length > 0}
                    no more cues
                  {:else}
                    🛡️ filter list found for "{playback.filter_match}", no cues
                  {/if}
                </p>
              {/if}
            </div>
          {:else}
            <p class="hint centered">Pair MRP or AirPlay (from the pairing wizard) to unlock mute/unmute and playback info.</p>
          {/if}

          <div class="transport-row">
            <button class="icon-btn" onclick={() => doSkip(-15)} disabled={controlBusy} aria-label="Back 15 seconds">⏪</button>
            {#if hasLive}
              <button class="icon-btn icon-btn-lg" onclick={doMute} disabled={controlBusy} aria-label="Mute">🔇</button>
              <button class="icon-btn icon-btn-lg" onclick={doUnmute} disabled={controlBusy} aria-label="Unmute">🔊</button>
            {/if}
            <button class="icon-btn" onclick={() => doSkip(15)} disabled={controlBusy} aria-label="Forward 15 seconds">⏩</button>
          </div>

          {#if hasLive}
            <section class="group">
              <p class="section-header">Auto Filter</p>
              {#if filterError}
                <p class="banner error">{filterError}</p>
              {/if}

              {#if filterSummary}
                <ul class="list">
                  <li class="list-row static">
                    <span class="truncate"><code>{filterSummary.path}</code></span>
                  </li>
                  <li class="list-row">
                    <span>Enabled</span>
                    <label class="switch">
                      <input type="checkbox" checked={filterEnabled} onchange={toggleFilterEnabled} disabled={filterBusy} />
                      <span class="switch-track"><span class="switch-thumb"></span></span>
                    </label>
                  </li>
                </ul>
                <p class="footnote">{filterSummary.media_count} title{filterSummary.media_count === 1 ? "" : "s"} in this list.</p>

                {#if filterSummary.categories.length > 0}
                  <p class="section-header">
                    Categories
                    {#if playback?.filter_match}— tap one to see (and individually toggle) its cues for "{playback.filter_match}"{/if}
                  </p>
                  <ul class="list">
                    {#each filterSummary.categories as category (category)}
                      {@const cues = cuesByCategory[category] ?? []}
                      {@const isExpanded = expandedCategories[category] ?? true}
                      <li>
                        <div class="list-row category-row">
                          <button
                            type="button"
                            class="disclosure"
                            class:expanded={isExpanded && cues.length > 0}
                            onclick={() => toggleExpanded(category)}
                            disabled={cues.length === 0}
                            aria-expanded={isExpanded}
                            aria-label={`${isExpanded ? "Collapse" : "Expand"} ${category}`}
                          >
                            {cues.length > 0 ? "›" : "·"}
                          </button>
                          <span class="category-label">
                            {category}
                            {#if cues.length > 0}<span class="hint">({cues.length})</span>{/if}
                          </span>
                          <label class="switch">
                            <input
                              type="checkbox"
                              checked={categoryEnabled[category] ?? true}
                              onchange={() => toggleCategory(category)}
                            />
                            <span class="switch-track"><span class="switch-thumb"></span></span>
                          </label>
                        </div>

                        {#if isExpanded && cues.length > 0}
                          <ul class="list nested-list">
                            {#each cues as cue (cue.index)}
                              <li
                                class="list-row cue-row"
                                class:cue-past={playback?.position != null && playback.position >= cue.end}
                                class:cue-active={playback?.position != null &&
                                  playback.position >= cue.start &&
                                  playback.position < cue.end}
                              >
                                <span class="cue-time">{fmtTime(cue.start)}–{fmtTime(cue.end)}</span>
                                <span class="cue-action">{cue.action === "mute" ? "🔇 mute" : "⏭️ skip"}</span>
                                <label class="switch switch-sm">
                                  <input type="checkbox" checked={cue.enabled} onchange={() => toggleCue(cue)} />
                                  <span class="switch-track"><span class="switch-thumb"></span></span>
                                </label>
                              </li>
                            {/each}
                          </ul>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                {/if}
              {/if}

              <button class="btn-secondary" onclick={pickFilterFile} disabled={filterBusy}>
                {filterSummary ? "Load a Different Filter File…" : "Load Filter File…"}
              </button>
            </section>

            <section class="group">
              <p class="section-header">Create Filter</p>
              {#if creationError}
                <p class="banner error">{creationError}</p>
              {/if}

              {#if creationStage === "idle"}
                <p class="hint">Record cue timestamps live from what's currently playing.</p>
                <div class="stack">
                  <button class="btn-secondary" onclick={pickNewDraft}>Record New Filter File…</button>
                  <button class="btn-secondary" onclick={pickExistingDraft}>Continue Existing Filter File…</button>
                </div>
              {:else if draft}
                <ul class="list">
                  <li class="list-row static">
                    <span class="truncate"><code>{draft.path}</code></span>
                  </li>
                </ul>
                <p class="footnote">
                  {draft.media_count} title{draft.media_count === 1 ? "" : "s"}
                  {#if !playback?.title}· nothing playing{/if}
                </p>
                <div class="stack">
                  <button class="btn-secondary" onclick={useDraftAsActiveFilter} disabled={filterBusy}>Use This Draft as Active Filter</button>
                  <button class="btn-secondary" onclick={() => { resetCreation(); }} disabled={creationBusy}>Close Draft</button>
                </div>
                {#if filterSummary?.path === draft.path}
                  <p class="hint">🛡️ this draft is the active auto filter -- cues you record show up there too.</p>
                {:else}
                  <p class="hint">Not the active auto filter yet -- use the button above once you're ready to test it.</p>
                {/if}

                <div class="category-buttons">
                  {#each categories as c (c.name)}
                    <button
                      type="button"
                      class="category-btn"
                      class:recording={pendingSkipCategory === c.name}
                      disabled={creationBusy || !playback?.title || (c.kind === "skip" && pendingSkipCategory !== null && pendingSkipCategory !== c.name)}
                      onclick={() => (c.kind === "mute" ? markMute(c.name) : toggleSkipMark(c.name))}
                    >
                      {c.kind === "mute" ? "🔇" : "⏭️"}
                      {c.name}
                      {#if pendingSkipCategory === c.name}(recording — tap to end){/if}
                    </button>
                  {/each}
                </div>
                {#if pendingSkipCategory}
                  <button class="btn-destructive" onclick={cancelSkipMark} disabled={creationBusy}>Cancel Mark</button>
                {/if}

                <div class="field-row">
                  <input class="field" placeholder="Custom category" bind:value={newCategoryName} />
                  <select class="field field-select" bind:value={newCategoryKind}>
                    <option value="skip">skip</option>
                    <option value="mute">mute</option>
                  </select>
                  <button type="button" class="btn-secondary" onclick={addCustomCategory} disabled={!newCategoryName.trim()}>Add</button>
                </div>

                {#if creationCues.length > 0}
                  <p class="hint">Recorded cues for "{playback?.title}" -- edit a time and press Enter/Tab to correct it.</p>
                  <ul class="list">
                    {#each creationCues as cue (cue.index)}
                      <li class="list-row cue-table-row">
                        <span class="cue-action">{cue.action === "mute" ? "🔇" : "⏭️"} {cue.category}</span>
                        <input
                          class="time-input"
                          value={fmtTime(cue.start)}
                          onchange={(e) => updateCueTime(cue, "start", (e.target as HTMLInputElement).value)}
                        />
                        <span class="hint">–</span>
                        <input
                          class="time-input"
                          value={fmtTime(cue.end)}
                          onchange={(e) => updateCueTime(cue, "end", (e.target as HTMLInputElement).value)}
                        />
                        <button type="button" class="delete-btn" onclick={() => deleteCue(cue)} aria-label="Delete cue">✕</button>
                      </li>
                    {/each}
                  </ul>
                {:else if playback?.title}
                  <p class="hint">No cues recorded yet for "{playback.title}".</p>
                {/if}
              {/if}
            </section>
          {/if}
        </section>
      {:else}
        <section class="screen">
          <div class="segmented">
            {#each ["companion", "mrp", "airplay", "save"] as s (s)}
              <div class="segment" class:active={step === s} class:done={STEPS.indexOf(step) > STEPS.indexOf(s as Step)}>
                {isProtocol(s as Step) ? PROTOCOL_LABEL[s as Protocol] : "Save"}
              </div>
            {/each}
          </div>

          {#if error}
            <p class="banner error">{error}</p>
          {/if}

          {#if isProtocol(step)}
            <p class="section-header">{PROTOCOL_LABEL[step]} pairing{step !== "companion" ? " (optional)" : ""}</p>
            {#if step !== "companion"}
              <p class="hint">Needed only for live playback position. Skip if this Apple TV isn't reachable over {PROTOCOL_LABEL[step]}.</p>
            {/if}

            {#if awaitingPinFor === step}
              <form onsubmit={(e) => { e.preventDefault(); submitPin(); }}>
                <p class="hint centered">Enter the PIN shown on your Apple TV:</p>
                <input class="pin-input" inputmode="numeric" autocomplete="one-time-code" bind:value={pin} placeholder="0000" />
                <button type="submit" class="btn-primary" disabled={!pin}>Submit</button>
              </form>
            {:else}
              <div class="stack">
                <button class="btn-secondary" onclick={() => scan(step as Protocol)} disabled={scanning || pairing}>
                  {scanning ? "Scanning…" : "Rescan"}
                </button>
                {#if step !== "companion"}
                  <button class="btn-secondary" onclick={skip} disabled={pairing}>Skip</button>
                {/if}
              </div>

              {#if devices.length > 0}
                <ul class="list">
                  {#each devices as device (device.host + device.port)}
                    <li>
                      <button class="list-row" onclick={() => pair(step as Protocol, device)} disabled={pairing}>
                        <span>{device.host}:{device.port}</span>
                        <span class="chevron">›</span>
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            {/if}
          {:else if step === "save"}
            <p class="section-header">Save pairing</p>
            <p class="hint">Ready to save credentials to <code>pairing.store</code>.</p>
            <button class="btn-primary" onclick={save} disabled={pairing}>{pairing ? "Saving…" : "Save"}</button>
          {:else if step === "done"}
            <p class="section-header">✅ Paired</p>
            <p class="hint">Credentials saved. This Apple TV is ready to control.</p>
            <button class="btn-primary" onclick={openControls}>Open Controls</button>
          {/if}
        </section>
      {/if}
    </div>
  </main>
</div>

<style>
:root {
  color-scheme: light dark;
  --bg: #f2f2f7;
  --grouped-bg: #f2f2f7;
  --card-bg: #ffffff;
  --label: #000000;
  --secondary-label: #6c6c70;
  --tertiary-label: #8e8e93;
  --separator: rgba(60, 60, 67, 0.29);
  --accent: #007aff;
  --destructive: #ff3b30;
  --success: #34c759;
  --error-bg: #fdecea;
  --success-bg: #e6f6ea;
  --shell-bg: #e5e5ea;
}

@media (prefers-color-scheme: dark) {
  :root {
    --bg: #000000;
    --grouped-bg: #000000;
    --card-bg: #1c1c1e;
    --label: #ffffff;
    --secondary-label: #98989f;
    --tertiary-label: #6c6c70;
    --separator: rgba(84, 84, 88, 0.6);
    --accent: #0a84ff;
    --destructive: #ff453a;
    --success: #30d158;
    --error-bg: #3a1613;
    --success-bg: #122a17;
    --shell-bg: #1c1c1e;
  }
}

:global(html),
:global(body) {
  height: 100%;
}

:global(body) {
  margin: 0;
  background: var(--shell-bg);
}

* {
  box-sizing: border-box;
}

/* Letterboxes the app to a phone-width column regardless of how wide the
   desktop window is resized -- the rest of the window just shows a plain
   fill so the app reads as a phone screen, not a stretched desktop pane. */
.phone-shell {
  display: flex;
  justify-content: center;
  min-height: 100vh;
  background: var(--shell-bg);
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", Arial, sans-serif;
  -webkit-font-smoothing: antialiased;
}

.canvas {
  width: 100%;
  max-width: 430px;
  min-height: 100vh;
  background: var(--bg);
  color: var(--label);
  display: flex;
  flex-direction: column;
}

.navbar {
  position: sticky;
  top: 0;
  z-index: 10;
  padding: calc(env(safe-area-inset-top) + 10px) 16px 10px;
  background: color-mix(in srgb, var(--bg) 82%, transparent);
  backdrop-filter: saturate(180%) blur(20px);
  -webkit-backdrop-filter: saturate(180%) blur(20px);
  border-bottom: 0.5px solid var(--separator);
  text-align: center;
}

.navbar h1 {
  margin: 0;
  font-size: 1.05em;
  font-weight: 600;
  letter-spacing: -0.01em;
}

.content {
  flex: 1;
  padding: 16px 16px calc(env(safe-area-inset-bottom) + 32px);
}

.screen {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.group {
  margin-top: 20px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.section-header {
  margin: 18px 4px 6px;
  font-size: 0.8em;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.03em;
  color: var(--secondary-label);
}

.section-header:first-child {
  margin-top: 4px;
}

.footnote {
  margin: 6px 4px 0;
  font-size: 0.8em;
  color: var(--secondary-label);
}

.hint {
  color: var(--secondary-label);
  font-size: 0.9em;
  margin: 8px 4px;
}

.hint.centered {
  text-align: center;
}

.banner {
  border-radius: 12px;
  padding: 0.75em 1em;
  margin: 8px 0;
  font-size: 0.92em;
}

.banner.error {
  color: var(--destructive);
  background: var(--error-bg);
}

.banner.success {
  color: var(--success);
  background: var(--success-bg);
}

/* Grouped-table-view list: rounded card, hairline separators between rows. */
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  background: var(--card-bg);
  border-radius: 14px;
  overflow: hidden;
}

.list li:not(:last-child) {
  border-bottom: 0.5px solid var(--separator);
}

.list-row {
  all: unset;
  box-sizing: border-box;
  display: flex;
  width: 100%;
  min-height: 44px;
  align-items: center;
  justify-content: space-between;
  gap: 0.6em;
  padding: 11px 16px;
  cursor: pointer;
}

.list-row.static {
  cursor: default;
}

button.list-row:active:not(:disabled) {
  background: color-mix(in srgb, var(--label) 6%, transparent);
}

.list-row:disabled {
  opacity: 0.5;
  cursor: default;
}

.list-row .value {
  color: var(--secondary-label);
}

.list-row .truncate {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.chevron {
  color: var(--tertiary-label);
  font-size: 1.1em;
}

.category-row {
  gap: 0.5em;
}

.category-label {
  flex: 1;
  text-align: left;
}

.nested-list {
  margin: 6px 12px 12px 40px;
  width: auto;
  border-radius: 10px;
}

.cue-row {
  font-size: 0.92em;
  opacity: 0.6;
  transition: opacity 0.15s ease;
}

.cue-time {
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

.cue-action {
  flex: 1;
  text-align: left;
  padding-left: 0.6em;
}

.cue-active {
  opacity: 1;
  background: color-mix(in srgb, var(--accent) 12%, transparent);
}

.cue-past {
  opacity: 0.35;
}

.cue-table-row .cue-action {
  padding-left: 0;
}

/* Disclosure chevron -- rotates open instead of swapping glyphs. */
.disclosure {
  all: unset;
  width: 1.4em;
  flex-shrink: 0;
  text-align: center;
  cursor: pointer;
  color: var(--tertiary-label);
  font-size: 1.1em;
  transition: transform 0.2s ease;
}

.disclosure.expanded {
  transform: rotate(90deg);
}

.disclosure:disabled {
  cursor: default;
  opacity: 0.4;
}

/* iOS-style toggle switch. */
.switch {
  position: relative;
  display: inline-flex;
  flex-shrink: 0;
  width: 51px;
  height: 31px;
}

.switch input {
  position: absolute;
  inset: 0;
  margin: 0;
  opacity: 0;
  cursor: pointer;
}

.switch-track {
  position: absolute;
  inset: 0;
  background: var(--separator);
  border-radius: 999px;
  transition: background-color 0.2s ease;
}

.switch-thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 27px;
  height: 27px;
  border-radius: 50%;
  background: #ffffff;
  box-shadow: 0 3px 1px rgba(0, 0, 0, 0.06), 0 3px 8px rgba(0, 0, 0, 0.15);
  transition: transform 0.2s ease;
}

.switch input:checked + .switch-track {
  background: var(--success);
}

.switch input:checked + .switch-track .switch-thumb {
  transform: translateX(20px);
}

.switch input:disabled + .switch-track {
  opacity: 0.5;
}

.switch-sm {
  width: 40px;
  height: 24px;
}

.switch-sm .switch-thumb {
  width: 20px;
  height: 20px;
}

.switch-sm input:checked + .switch-track .switch-thumb {
  transform: translateX(16px);
}

/* Now-playing card. */
.now-playing {
  text-align: center;
  margin: 12px 0 20px;
}

.now-playing .title {
  font-size: 1.25em;
  font-weight: 700;
  margin: 0 0 0.2em;
}

.now-playing .subtitle {
  color: var(--secondary-label);
  margin: 0 0 0.8em;
  text-transform: capitalize;
}

.progress-track {
  height: 4px;
  border-radius: 999px;
  background: var(--separator);
  overflow: hidden;
  margin: 0 4px;
}

.progress-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 999px;
}

.now-playing .position {
  color: var(--secondary-label);
  font-variant-numeric: tabular-nums;
  margin: 0.6em 0 0;
  font-size: 0.9em;
}

/* Transport controls. */
.transport-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 1.4em;
  margin: 0.5em 0 1.2em;
}

.icon-btn {
  all: unset;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 52px;
  height: 52px;
  border-radius: 50%;
  background: var(--card-bg);
  font-size: 1.3em;
  cursor: pointer;
}

.icon-btn-lg {
  width: 64px;
  height: 64px;
  font-size: 1.6em;
}

.icon-btn:active:not(:disabled) {
  background: color-mix(in srgb, var(--label) 8%, var(--card-bg));
}

.icon-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

/* Buttons. */
.btn-primary,
.btn-secondary,
.btn-destructive {
  all: unset;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  min-height: 46px;
  border-radius: 12px;
  font-size: 1em;
  font-weight: 600;
  text-align: center;
  cursor: pointer;
}

.btn-primary {
  background: var(--accent);
  color: #ffffff;
  padding: 0.7em 1em;
}

.btn-primary:active:not(:disabled) {
  opacity: 0.85;
}

.btn-secondary {
  color: var(--accent);
  background: var(--card-bg);
  padding: 0.7em 1em;
}

.btn-secondary:active:not(:disabled) {
  background: color-mix(in srgb, var(--label) 6%, var(--card-bg));
}

.btn-destructive {
  color: var(--destructive);
  background: var(--card-bg);
  padding: 0.7em 1em;
}

.btn-destructive:active:not(:disabled) {
  background: color-mix(in srgb, var(--destructive) 10%, var(--card-bg));
}

.btn-primary:disabled,
.btn-secondary:disabled,
.btn-destructive:disabled {
  opacity: 0.4;
  cursor: default;
}

.stack {
  display: flex;
  flex-direction: column;
  gap: 0.6em;
  margin: 0.8em 0;
}

/* Segmented control for the pairing wizard's step indicator. */
.segmented {
  display: flex;
  gap: 2px;
  padding: 2px;
  margin: 4px 0 16px;
  background: var(--card-bg);
  border-radius: 10px;
  font-size: 0.82em;
}

.segment {
  flex: 1;
  text-align: center;
  padding: 0.5em 0.2em;
  border-radius: 8px;
  color: var(--tertiary-label);
  font-weight: 500;
}

.segment.done {
  color: var(--accent);
}

.segment.active {
  background: var(--accent);
  color: #ffffff;
  font-weight: 600;
}

.field-row {
  display: flex;
  gap: 0.5em;
  align-items: center;
  margin: 1em 0;
}

.field {
  all: unset;
  box-sizing: border-box;
  flex: 1;
  min-height: 40px;
  padding: 0.5em 0.8em;
  border-radius: 10px;
  background: var(--card-bg);
  font-size: 0.95em;
}

.field-select {
  flex: 0 0 auto;
}

.category-buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5em;
  margin: 1em 0;
}

.category-btn {
  all: unset;
  box-sizing: border-box;
  padding: 0.6em 1em;
  border-radius: 999px;
  background: var(--card-bg);
  color: var(--label);
  font-weight: 500;
  cursor: pointer;
}

.category-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.category-btn.recording {
  background: var(--destructive);
  color: #ffffff;
}

.time-input {
  all: unset;
  box-sizing: border-box;
  width: 4.2em;
  padding: 0.3em 0.5em;
  border-radius: 8px;
  background: var(--bg);
  font-variant-numeric: tabular-nums;
  text-align: center;
}

.delete-btn {
  all: unset;
  box-sizing: border-box;
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  background: var(--destructive);
  color: #ffffff;
  font-size: 0.7em;
  cursor: pointer;
  flex-shrink: 0;
}

form {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.8em;
  margin: 1.5em 0;
}

.pin-input {
  all: unset;
  box-sizing: border-box;
  width: 60%;
  text-align: center;
  font-size: 1.6em;
  font-weight: 600;
  letter-spacing: 0.15em;
  padding: 0.4em;
  border-radius: 12px;
  background: var(--card-bg);
  font-variant-numeric: tabular-nums;
}

.pin-input::placeholder {
  color: var(--tertiary-label);
}

code {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.95em;
}
</style>
