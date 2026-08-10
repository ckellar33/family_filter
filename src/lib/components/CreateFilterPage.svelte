<script lang="ts">
  // Record cue timestamps live from whatever's currently playing. Unchanged
  // from the original single-page version's Create Filter section, just
  // relocated to its own tab.
  import { session } from "$lib/state/session.svelte";
  import {
    creationState,
    pickNewDraft,
    pickExistingDraft,
    refreshCreationCues,
    refreshServices,
    resetCreation,
    useDraftAsActiveFilter,
    markMute,
    toggleSkipMark,
    cancelSkipMark,
    updateCueTime,
    deleteCue,
    addCustomCategory,
    addService,
    removeService,
  } from "$lib/state/creation.svelte";
  import { filterState } from "$lib/state/filter.svelte";
  import { fmtTime } from "$lib/format";

  // Re-fetches the draft's cues and service tags whenever the now-playing
  // title changes while recording, so both always reflect the title
  // actually on screen.
  $effect(() => {
    session.playback?.title;
    if (creationState.stage === "recording") {
      refreshCreationCues();
      refreshServices();
    }
  });
</script>

<section class="screen">
  {#if creationState.error}
    <p class="banner error">{creationState.error}</p>
  {/if}

  {#if creationState.stage === "idle"}
    <p class="hint">Record cue timestamps live from what's currently playing.</p>
    <div class="stack">
      <button class="btn-secondary" onclick={pickNewDraft}>Record New Filter File…</button>
      <button class="btn-secondary" onclick={pickExistingDraft}>Continue Existing Filter File…</button>
    </div>
  {:else if creationState.draft}
    {@const draft = creationState.draft}
    <ul class="list">
      <li class="list-row static">
        <span class="truncate"><code>{draft.path}</code></span>
      </li>
    </ul>
    <p class="footnote">
      {draft.media_count} title{draft.media_count === 1 ? "" : "s"}
      {#if !session.playback?.title}· nothing playing{/if}
    </p>
    <div class="stack">
      <button class="btn-secondary" onclick={useDraftAsActiveFilter} disabled={filterState.filterBusy}>Use This Draft as Active Filter</button>
      <button class="btn-secondary" onclick={resetCreation} disabled={creationState.busy}>Close Draft</button>
    </div>
    {#if filterState.filterSummary?.path === draft.path}
      <p class="hint">🛡️ this draft is the active auto filter -- cues you record show up there too.</p>
    {:else}
      <p class="hint">Not the active auto filter yet -- use the button above once you're ready to test it.</p>
    {/if}

    <p class="section-header">
      Services
      {#if session.playback?.title}— tagged automatically from whatever app is playing "{session.playback.title}"{/if}
    </p>
    {#if creationState.services.length > 0}
      <div class="category-buttons">
        {#each creationState.services as service (service)}
          <button type="button" class="category-btn" onclick={() => removeService(service)} disabled={!session.playback?.title}>
            {service} ✕
          </button>
        {/each}
      </div>
    {:else}
      <p class="hint">
        {#if session.playback?.title}No service tagged yet for "{session.playback.title}" -- mark a cue to auto-tag it, or add one below.{:else}Nothing playing -- a service gets tagged automatically from the first cue you mark.{/if}
      </p>
    {/if}
    <div class="field-row">
      <input class="field" placeholder="Add a service (e.g. Netflix)" bind:value={creationState.newServiceName} disabled={!session.playback?.title} />
      <button type="button" class="btn-secondary" onclick={addService} disabled={!creationState.newServiceName.trim() || !session.playback?.title}>
        Add
      </button>
    </div>

    <div class="category-buttons">
      {#each creationState.categories as c (c.name)}
        <button
          type="button"
          class="category-btn"
          class:recording={creationState.pendingSkipCategory === c.name}
          disabled={creationState.busy ||
            !session.playback?.title ||
            (c.kind === "skip" && creationState.pendingSkipCategory !== null && creationState.pendingSkipCategory !== c.name)}
          onclick={() => (c.kind === "mute" ? markMute(c.name) : toggleSkipMark(c.name))}
        >
          {c.kind === "mute" ? "🔇" : "⏭️"}
          {c.name}
          {#if creationState.pendingSkipCategory === c.name}(recording — tap to end){/if}
        </button>
      {/each}
    </div>
    {#if creationState.pendingSkipCategory}
      <button class="btn-destructive" onclick={cancelSkipMark} disabled={creationState.busy}>Cancel Mark</button>
    {/if}

    <div class="field-row">
      <input class="field" placeholder="Custom category" bind:value={creationState.newCategoryName} />
      <select class="field field-select" bind:value={creationState.newCategoryKind}>
        <option value="skip">skip</option>
        <option value="mute">mute</option>
      </select>
      <button type="button" class="btn-secondary" onclick={addCustomCategory} disabled={!creationState.newCategoryName.trim()}>Add</button>
    </div>

    {#if creationState.cues.length > 0}
      <p class="hint">Recorded cues for "{session.playback?.title}" -- edit a time and press Enter/Tab to correct it.</p>
      <ul class="list">
        {#each creationState.cues as cue (cue.index)}
          <li class="list-row cue-table-row">
            <span class="cue-action">{cue.action === "mute" ? "🔇" : "⏭️"} {cue.category}</span>
            <input class="time-input" value={fmtTime(cue.start)} onchange={(e) => updateCueTime(cue, "start", (e.target as HTMLInputElement).value)} />
            <span class="hint">–</span>
            <input class="time-input" value={fmtTime(cue.end)} onchange={(e) => updateCueTime(cue, "end", (e.target as HTMLInputElement).value)} />
            <button type="button" class="delete-btn" onclick={() => deleteCue(cue)} aria-label="Delete cue">✕</button>
          </li>
        {/each}
      </ul>
    {:else if session.playback?.title}
      <p class="hint">No cues recorded yet for "{session.playback.title}".</p>
    {/if}
  {/if}
</section>
