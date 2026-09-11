<script lang="ts">
  // Record cue timestamps live from whatever's currently playing --
  // mark-then-label (design 4a). Two buttons, always the same two: Skip
  // (press at the start of the scene, again at the end) and Mute (stamps a
  // fixed window at the tap). The press freezes the timestamp and the cue
  // lands on disk immediately; CueLabelSheet then asks what it was, so no
  // clock is running while that gets answered.
  //
  // Recorded rows stay tappable afterwards: a skip row to change its
  // category or add a note, a mute row to pick the specific words for its
  // kind of language -- the one part of labeling deliberately deferred past
  // record time. Timing still lives in CueEditorSheet, reachable from the
  // label sheet's footer.
  import { session } from "$lib/state/session.svelte";
  import {
    creationState,
    currentService,
    pickNewDraft,
    pickExistingDraft,
    refreshCreationCues,
    renameService,
    resetCreation,
    useDraftAsActiveFilter,
    markMute,
    startSkipMark,
    endSkipMark,
    cancelSkipMark,
    setCueLabel,
    updateCueTime,
    deleteCue,
    isLanguageCategory,
    languageKindFor,
    MUTE_MARK_SECS,
    UNLABELED_SKIP,
  } from "$lib/state/creation.svelte";
  import { filterState } from "$lib/state/filter.svelte";
  import { fmtTime, censorWord } from "$lib/format";
  import type { CreationCue } from "$lib/types";
  import CueLabelSheet from "$lib/components/CueLabelSheet.svelte";
  import CueEditorSheet from "$lib/components/CueEditorSheet.svelte";

  // Which recorded cue each sheet is open on, if any. `labeling` carries
  // its own mode: "new" right after a mark (Discard deletes the cue),
  // "edit" when opened from a row.
  let labeling = $state<{ cue: CreationCue; mode: "new" | "edit" } | null>(null);
  let editingCue = $state<CreationCue | null>(null);

  // Re-fetches the draft's cues whenever the now-playing title or the app
  // it's playing in changes while recording, so the list always reflects
  // what's actually landing marks right now (see currentService).
  $effect(() => {
    session.playback?.title;
    session.playback?.app_name;
    if (creationState.stage === "recording") {
      refreshCreationCues();
    }
  });

  // Both mark paths land the cue first and hand back its index; the sheet
  // opens on the refreshed copy of that cue rather than the mark result, so
  // it's editing the same object the list is showing.
  function openLabelSheet(index: number, mode: "new" | "edit") {
    const cue = creationState.cues.find((c) => c.index === index);
    if (cue) labeling = { cue, mode };
  }

  async function onSkip() {
    if (creationState.skipPending) {
      const result = await endSkipMark();
      if (result) openLabelSheet(result.index, "new");
    } else {
      await startSkipMark();
    }
  }

  async function onMute() {
    const result = await markMute();
    if (result) openLabelSheet(result.index, "new");
  }

  async function saveLabel(cue: CreationCue, label: { category: string; note: string; word: string }) {
    await setCueLabel(cue, label);
    labeling = null;
  }

  // Discarding a *new* cue deletes it outright: it was recorded before
  // anyone said what it was, so an unlabeled leftover is worse than none.
  async function discardLabel(cue: CreationCue) {
    await deleteCue(cue);
    labeling = null;
  }

  // Both paths must clear `editingCue` -- a sheet left open on a cue that
  // no longer exists (or whose index just shifted) is the one failure mode
  // worth guarding here.
  async function saveEditedCue(cue: CreationCue, next: { start: number; end: number }) {
    if (next.start !== cue.start || next.end !== cue.end) await updateCueTime(cue, next.start, next.end);
    editingCue = null;
  }

  async function deleteEditedCue(cue: CreationCue) {
    await deleteCue(cue);
    editingCue = null;
  }

  // One line under the time range saying what this cue is -- and, for a
  // mute cue with no words picked yet, saying so in clay so the row reads
  // as unfinished rather than done.
  function cueLabel(cue: CreationCue): { text: string; pending: boolean } {
    if (isLanguageCategory(cue.category)) {
      const kind = languageKindFor(cue.category);
      const words = (cue.word ?? "")
        .split(",")
        .map((w) => w.trim())
        .filter(Boolean);
      if (!words.length) return { text: `${kind.label} · tap to pick the words`, pending: true };
      return { text: `${kind.label} · ${words.map((w) => (kind.censor ? censorWord(w) : w)).join(" · ")}`, pending: false };
    }
    if (cue.category === UNLABELED_SKIP) return { text: "tap to say what it was", pending: true };
    return { text: cue.category, pending: false };
  }

  let armed = $derived(creationState.draft != null && filterState.filterSummary?.path === creationState.draft.path);
  let canMark = $derived(!!session.playback?.title && !creationState.busy);
</script>

<section class="screen">
  {#if creationState.error}
    <p class="banner error">{creationState.error}</p>
  {/if}

  {#if creationState.stage === "idle"}
    <p class="hint">Record cue timestamps live from what's currently playing. Nothing is armed until you say so.</p>
    <div class="stack">
      <button class="btn-primary" onclick={pickNewDraft}>Record a new filter file…</button>
      <button class="btn-secondary" onclick={pickExistingDraft}>Continue an existing one…</button>
    </div>
  {:else if creationState.draft}
    {@const draft = creationState.draft}
    <div class="draft-card">
      <div style="flex:1; min-width:0">
        <p class="path truncate">{draft.path}</p>
        <p class="title">{session.playback?.title ?? "Nothing playing"}</p>
        <p class="path">
          {draft.media_count} title{draft.media_count === 1 ? "" : "s"}
          {#if session.playback?.title}· marks land under {currentService() || "Generic"}{/if}
        </p>
      </div>
      <span class="shield" data-state={armed ? "on" : "off"}>{armed ? "ARMED" : "DRAFT"}</span>
    </div>

    {#if session.playback?.title && !currentService()}
      <p class="hint">
        The app on screen isn't recognized, so marks are landing in the generic entry — each service gets its own independent timing.
      </p>
      <div class="field-row">
        <input class="field" placeholder="Correct the service (e.g. Netflix)" bind:value={creationState.renameServiceInput} />
        <button type="button" class="btn-secondary" style="width:auto; min-height:48px" onclick={renameService} disabled={!creationState.renameServiceInput.trim()}>
          Rename
        </button>
      </div>
    {/if}

    <div class="mark-buttons">
      <button type="button" class="mark-btn" class:recording={creationState.skipPending} onclick={onSkip} disabled={!canMark}>
        <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M5 6.5 13 12l-8 5.5V6.5Z" />
          <path d="M17.5 6v12" />
        </svg>
        <span class="mark-btn-text">
          <span class="mark-btn-name">{creationState.skipPending ? "End skip" : "Start skip"}</span>
          <span class="mark-btn-hint">
            {creationState.skipPending
              ? `started at ${fmtTime(creationState.pendingSkipStart)}`
              : "press at the start, again at the end"}
          </span>
        </span>
      </button>

      <button type="button" class="mark-btn" onclick={onMute} disabled={!canMark || creationState.skipPending}>
        <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M11 5 6.5 9H3.5v6h3L11 19V5Z" />
          <path d="m15.5 9.5 5 5M20.5 9.5l-5 5" />
        </svg>
        <span class="mark-btn-text">
          <span class="mark-btn-name">Mute</span>
          <span class="mark-btn-hint">always language · stamps {MUTE_MARK_SECS}s</span>
        </span>
      </button>
    </div>

    {#if creationState.skipPending}
      <div class="pending-mark">
        <span class="dot"></span>
        <span style="flex:1">Recording a skip — press it again where the scene ends</span>
        <button type="button" onclick={cancelSkipMark} disabled={creationState.busy}>Cancel</button>
      </div>
    {/if}

    <div class="section-head-row">
      <span class="section-header" style="margin:0">Recorded</span>
      <span class="path">{creationState.cues.length} cue{creationState.cues.length === 1 ? "" : "s"}</span>
    </div>

    {#if creationState.cues.length > 0}
      <ul class="list">
        {#each creationState.cues as cue (cue.index)}
          {@const label = cueLabel(cue)}
          <li>
            <button type="button" class="list-row cue-table-row" onclick={() => (labeling = { cue, mode: "edit" })}>
              <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
              <span class="device-row-text">
                <span class="cue-time">{fmtTime(cue.start)} – {fmtTime(cue.end)}</span>
                <span class="cue-label" class:pending={label.pending}>{label.text}</span>
                {#if cue.note}
                  <span class="cue-note">“{cue.note}”</span>
                {/if}
              </span>
              <span class="chevron">›</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <ul class="list">
        <li class="list-row static" style="justify-content:center; padding:24px 16px">
          <span class="hint centered" style="margin:0">
            {session.playback?.title
              ? "Nothing recorded yet. Start a skip, or mute a line."
              : "Nothing playing — start something on the Apple TV to record against it."}
          </span>
        </li>
      </ul>
    {/if}

    <div class="stack">
      <button
        class="btn-primary"
        style={armed ? "background: var(--success-bg); color: oklch(0.36 0.07 152); border:1px solid var(--accent-line)" : ""}
        onclick={useDraftAsActiveFilter}
        disabled={filterState.filterBusy || armed}
      >
        {armed ? "Active filter — cues apply live" : "Use this draft as the active filter"}
      </button>
      <button class="btn-secondary" onclick={resetCreation} disabled={creationState.busy}>Close draft</button>
    </div>
  {/if}
</section>

{#if labeling}
  {@const cue = labeling.cue}
  <CueLabelSheet
    mode={labeling.mode}
    action={cue.action}
    start={cue.start}
    end={cue.end}
    category={cue.category}
    note={cue.note}
    word={cue.word}
    busy={creationState.busy}
    onSave={(label) => saveLabel(cue, label)}
    onDiscard={() => discardLabel(cue)}
    onClose={() => (labeling = null)}
    onRetime={() => {
      labeling = null;
      editingCue = cue;
    }}
  />
{/if}

{#if editingCue}
  {@const cue = editingCue}
  <CueEditorSheet
    start={cue.start}
    end={cue.end}
    category={cue.category}
    action={cue.action}
    busy={creationState.busy}
    onSave={(next) => saveEditedCue(cue, next)}
    onDelete={() => deleteEditedCue(cue)}
    onClose={() => (editingCue = null)}
  />
{/if}
