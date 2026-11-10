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
  import { session, livePosition } from "$lib/state/session.svelte";
  import {
    creationState,
    currentService,
    pickNewDraft,
    refreshCreationCues,
    renameService,
    resetCreation,
    deleteDraft,
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
    buildAndAddFilter,
    resetGenerate,
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

  // Auto-generate flow: whether the "Build from subtitles" form is open.
  let showGenerate = $state(false);

  // Split into two rails, VidAngel-style: an unlabeled mark is "waiting to
  // be tagged" no matter its action, and once labeled it moves into the
  // category tree below (mirrors SelectFilterPage's grouped list) --
  // grouped by the cue's own `category` string, so each language kind
  // (Profanity, Blasphemy, …) gets its own group rather than one big
  // "language" bucket.
  let pendingCues = $derived(creationState.cues.filter((c) => cueLabel(c).pending));
  let taggedCues = $derived(creationState.cues.filter((c) => !cueLabel(c).pending));

  let taggedGroups = $derived.by(() => {
    const order: string[] = [];
    const byCategory: Record<string, CreationCue[]> = {};
    for (const cue of taggedCues) {
      if (!byCategory[cue.category]) {
        byCategory[cue.category] = [];
        order.push(cue.category);
      }
      byCategory[cue.category].push(cue);
    }
    return order.map((category) => ({
      category,
      label: isLanguageCategory(category) ? languageKindFor(category).label : category,
      cues: byCategory[category],
    }));
  });

  // One line under a tagged cue's time range for the detail that isn't
  // already said by its group heading -- the specific word(s) for a
  // language cue, or a note left on a skip cue. Omitted entirely when
  // there's nothing more to say (a plain skip cue in Gore/Peril/etc.).
  function cueDetailText(cue: CreationCue): string | null {
    if (isLanguageCategory(cue.category)) {
      const kind = languageKindFor(cue.category);
      const words = (cue.word ?? "")
        .split(",")
        .map((w) => w.trim())
        .filter(Boolean);
      if (words.length) return words.map((w) => (kind.censor ? censorWord(w) : w)).join(" · ");
    }
    return cue.note ? `“${cue.note}”` : null;
  }

  // Which tagged category groups are expanded -- same "absent means
  // expanded" default as SelectFilterPage's own tree, so it opens up
  // ready-to-read.
  let expandedTagged = $state<Record<string, boolean>>({});
  function toggleTaggedExpanded(category: string) {
    expandedTagged = { ...expandedTagged, [category]: !(expandedTagged[category] ?? true) };
  }

  // Delete's own native confirm, same no-custom-sheet reasoning
  // SelectFilterPage's "On this device" delete already uses for its first
  // whole-file destructive action -- this permanently removes the file, not
  // just the on-screen session (see deleteDraft).
  function confirmDeleteDraft() {
    if (!creationState.draft) return;
    if (confirm(`Delete "${creationState.draft.path}"? This permanently removes the file and every cue recorded in it.`)) {
      deleteDraft();
    }
  }

  // Open the auto-generate form.
  function openGenerate() {
    showGenerate = true;
  }

  // Close the auto-generate form without saving.
  function closeGenerate() {
    showGenerate = false;
    resetGenerate();
  }

  // Handle the generate action button — calls buildAndAddFilter which
  // downloads subtitles, builds the filter, saves it, and opens it.
  async function handleGenerate() {
    const result = await buildAndAddFilter();
    if (result) showGenerate = false;
  }
</script>

<section class="screen">
  {#if creationState.error}
    <p class="banner error">{creationState.error}</p>
  {/if}

  {#if creationState.stage === "idle" && !showGenerate}
    <p class="hint">Record cue timestamps live from what's currently playing. Nothing is armed until you say so.</p>
    <div class="stack">
      <button class="btn-primary" onclick={pickNewDraft}>Record a new filter file…</button>
      <button class="btn-secondary" onclick={openGenerate}>Build from subtitles…</button>
    </div>
  {:else if showGenerate}
    <div class="screen">
      <div class="screen-edge">
        <button type="button" class="back-link" onclick={closeGenerate}>‹ Back</button>
      </div>
      <h2 class="screen-title">Build from subtitles</h2>
      <p class="hint">Download captions for this title and automatically generate a filter file.</p>

      {#if creationState.generatePath}
        <div class="success-card">
          <p class="success-title">Filter generated</p>
          <p class="success-detail">"{creationState.generateTitle}" — added to your local filters.</p>
        </div>
      {/if}

      {#if creationState.generateError}
        <p class="banner error">{creationState.generateError}</p>
      {/if}

      <div class="generate-form">
        <div class="field-row">
          <input class="field" placeholder="Movie or show title" bind:value={creationState.generateTitle} />
        </div>
        <div class="field-row">
          <input class="field" placeholder="Year (optional)" bind:value={creationState.generateYear} />
        </div>
        <div class="field-row">
          <input class="field" placeholder="SubDL API key" bind:value={creationState.generateApikey} />
        </div>
      </div>

      <div class="stack">
        <button class="btn-primary" onclick={handleGenerate} disabled={creationState.generateBusy || !creationState.generateTitle.trim() || !creationState.generateApikey.trim()}>
          {creationState.generateBusy ? "Generating…" : "Generate filter"}
        </button>
      </div>
    </div>
  {:else if creationState.draft}
    {@const draft = creationState.draft}
    <div class="screen">
        <div class="screen-edge">
            <button type="button" class="back-link" onclick={resetCreation}>‹ Back</button>
        </div>
        <div class="draft-card">
          <div style="flex:1; min-width:0">
            <p class="caption">Recording into a new filter</p>
            <p class="title truncate">{session.playback?.title ?? "Unknown"}</p>
            <p class="path">
              {#if session.playback?.title}
                {currentService() || "Generic"} · {fmtTime(livePosition() ?? 0)}
              {:else}
                {draft.media_count} title{draft.media_count === 1 ? "" : "s"} · nothing playing
              {/if}
            </p>
          </div>
          <span class="shield" data-state={armed ? "on" : "off"}>{armed ? "ARMED" : "DRAFT"}</span>
        </div>
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
      <button type="button" class="mark-btn mark-btn-skip" class:recording={creationState.skipPending} onclick={onSkip} disabled={!canMark}>
        <span class="mark-btn-text">
          <span class="mark-btn-name">{creationState.skipPending ? "End skip" : "Start skip"}</span>
          <span class="mark-btn-hint">
            {creationState.skipPending ? `started at ${fmtTime(creationState.pendingSkipStart)}` : "tag it later"}
          </span>
        </span>
      </button>

      <button type="button" class="mark-btn" onclick={onMute} disabled={!canMark || creationState.skipPending}>
        <span class="mark-btn-text">
          <span class="mark-btn-name">Mute</span>
          <span class="mark-btn-hint">{MUTE_MARK_SECS}s stamp</span>
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
      <span class="section-header attn" style="margin:0">Waiting to be tagged</span>
      <span class="path">{pendingCues.length}</span>
    </div>

    {#if pendingCues.length > 0}
      <ul class="list">
        {#each pendingCues as cue (cue.index)}
          {@const label = cueLabel(cue)}
          <li>
            <button type="button" class="list-row cue-table-row" onclick={() => (labeling = { cue, mode: "edit" })}>
              <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
              <span class="device-row-text">
                <span class="cue-time">{fmtTime(cue.start)} – {fmtTime(cue.end)}</span>
                <span class="cue-label pending">{label.text}</span>
              </span>
              <span class="chevron">›</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <div class="waiting-box">
        <p class="hint centered" style="margin:0">
          {session.playback?.title
            ? "Nothing waiting. Marks land here the moment you press a button."
            : "Nothing playing — start something on the Apple TV to record against it."}
        </p>
      </div>
    {/if}

    <div class="section-head-row">
      <span class="section-header" style="margin:0">Tagged — by category</span>
      <span class="path">{taggedCues.length} cue{taggedCues.length === 1 ? "" : "s"}</span>
    </div>

    {#if taggedGroups.length > 0}
      <ul class="list">
        {#each taggedGroups as group (group.category)}
          {@const isExpanded = expandedTagged[group.category] ?? true}
          <li>
            <div class="list-row category-row">
              <button
                type="button"
                class="category-label"
                onclick={() => toggleTaggedExpanded(group.category)}
                aria-expanded={isExpanded}
              >
                <span class="cat-dot" data-cat={isLanguageCategory(group.category) ? "language" : group.category}></span>
                {group.label}
                <span class="hint">{group.cues.length} {group.cues.length === 1 ? "Cue" : "Cues"}</span>
              </button>
              <button
                type="button"
                class="disclosure"
                class:expanded={isExpanded}
                onclick={() => toggleTaggedExpanded(group.category)}
                aria-label={`${isExpanded ? "Collapse" : "Expand"} ${group.label}`}
              >
                ›
              </button>
            </div>

            {#if isExpanded}
              <ul class="list nested-list">
                {#each group.cues as cue (cue.index)}
                  {@const detail = cueDetailText(cue)}
                  <li>
                    <button type="button" class="list-row cue-row" onclick={() => (labeling = { cue, mode: "edit" })}>
                      <span class="device-row-text">
                        <span class="cue-time">{fmtTime(cue.start)}–{fmtTime(cue.end)}</span>
                        {#if detail}
                          <span class="cue-label">{detail}</span>
                        {/if}
                      </span>
                      <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <div class="waiting-box">
        <p class="hint centered" style="margin:0">Nothing tagged yet — labeled marks land here, grouped by category.</p>
      </div>
    {/if}

    <div class="stack">
      <button class="btn-primary" onclick={resetCreation} disabled={creationState.busy}>Save</button>
      <button class="btn-destructive" onclick={confirmDeleteDraft} disabled={creationState.busy}>Delete</button>
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
