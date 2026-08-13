<script lang="ts">
  // Record cue timestamps live from whatever's currently playing. Unchanged
  // from the original single-page version's Create Filter section, just
  // relocated to its own tab -- plus the cue editor sheet, which is where
  // retiming a recorded cue now happens (the inline time inputs are still
  // there underneath it).
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
    toggleSkipMark,
    cancelSkipMark,
    updateCueTime,
    deleteCue,
    addCustomCategory,
  } from "$lib/state/creation.svelte";
  import { filterState } from "$lib/state/filter.svelte";
  import { fmtTime } from "$lib/format";
  import type { CreationCue } from "$lib/types";
  import CueEditorSheet from "$lib/components/CueEditorSheet.svelte";

  // Which recorded cue the editor sheet is open on, if any.
  let editingCue = $state<CreationCue | null>(null);

  // Re-fetches the draft's cues whenever the now-playing title or the app
  // it's playing in changes while recording, so the table always reflects
  // what's actually landing marks right now (see currentService).
  $effect(() => {
    session.playback?.title;
    session.playback?.app_name;
    if (creationState.stage === "recording") {
      refreshCreationCues();
    }
  });

  // Both paths must clear `editingCue` -- a sheet left open on a cue that
  // no longer exists (or whose index just shifted) is the one failure mode
  // worth guarding here.
  async function saveEditedCue(cue: CreationCue, next: { start: number; end: number }) {
    if (next.start !== cue.start) await updateCueTime(cue, "start", fmtTime(next.start));
    if (next.end !== cue.end) await updateCueTime(cue, "end", fmtTime(next.end));
    editingCue = null;
  }

  async function deleteEditedCue(cue: CreationCue) {
    await deleteCue(cue);
    editingCue = null;
  }

  let armed = $derived(creationState.draft != null && filterState.filterSummary?.path === creationState.draft.path);
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

    <p class="section-header">Mark a cue</p>
    <div class="category-buttons">
      {#each creationState.categories as c (c.name)}
        {@const recording = creationState.pendingSkipCategory === c.name}
        <button
          type="button"
          class="category-btn"
          class:recording
          disabled={creationState.busy ||
            !session.playback?.title ||
            (c.kind === "skip" && creationState.pendingSkipCategory !== null && !recording)}
          onclick={() => (c.kind === "mute" ? markMute(c.name) : toggleSkipMark(c.name))}
        >
          <span>{c.kind === "mute" ? "🔇" : "⏭"}</span>
          <span class="category-btn-text">
            <span>{c.name}</span>
            <span class="category-btn-kind">{recording ? "tap to end" : c.kind}</span>
          </span>
        </button>
      {/each}
    </div>

    {#if creationState.pendingSkipCategory}
      <div class="pending-mark">
        <span class="dot"></span>
        <span style="flex:1">Recording {creationState.pendingSkipCategory} — tap it again to end</span>
        <button type="button" onclick={cancelSkipMark} disabled={creationState.busy}>Cancel</button>
      </div>
    {/if}

    <div class="field-row">
      <input class="field" placeholder="Custom category…" bind:value={creationState.newCategoryName} />
      <select class="field field-select" bind:value={creationState.newCategoryKind}>
        <option value="skip">skip</option>
        <option value="mute">mute</option>
      </select>
      <button type="button" class="btn-secondary" style="width:auto; min-height:48px" onclick={addCustomCategory} disabled={!creationState.newCategoryName.trim()}>
        Add
      </button>
    </div>

    <p class="section-header">Recorded — tap a cue to nudge its edges</p>
    {#if creationState.cues.length > 0}
      <ul class="list">
        {#each creationState.cues as cue (cue.index)}
          <li>
            <button type="button" class="list-row cue-table-row" onclick={() => (editingCue = cue)}>
              <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
              <span class="device-row-text">
                <span class="cue-time">{fmtTime(cue.start)} – {fmtTime(cue.end)}</span>
                <span class="addr" style="font-family: inherit">{cue.category}</span>
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
              ? `No cues yet for "${session.playback.title}". Tap a category the moment something lands.`
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

{#if editingCue}
  {@const cue = editingCue}
  <CueEditorSheet
    start={cue.start}
    end={cue.end}
    category={cue.category}
    action={cue.action}
    categories={creationState.categories}
    busy={creationState.busy}
    onSave={(next) => saveEditedCue(cue, next)}
    onDelete={() => deleteEditedCue(cue)}
    onClose={() => (editingCue = null)}
  />
{/if}
