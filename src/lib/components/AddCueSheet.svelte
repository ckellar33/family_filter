<script lang="ts">
  // "Type in a cue by hand" sheet for the Filters tab's own "+ Add cue"
  // button -- deliberately separate from CueEditorSheet (timing only, for a
  // cue that already exists) and CueLabelSheet (category/word/note, for a
  // cue a live mark just created): this is the one place a cue's action,
  // category, *and* timing all need to be picked from scratch with nothing
  // to start from, so it asks for everything at once instead. Kept lean
  // (plain inputs, no nudge stepper, no live-playback "jump to test") --
  // added for manually back-filling cues in bulk, not meant to carry the
  // same polish as those two long-term (see filter.svelte.ts's
  // addDetailCue for the other half of that).
  import { parseTime } from "$lib/format";
  import { portal, swipeDownToClose } from "$lib/actions";

  let {
    busy = false,
    onSave,
    onClose,
  }: {
    busy?: boolean;
    onSave: (cue: { start: number; end: number; action: "mute" | "skip"; category: string; word: string | null; note: string | null }) => void;
    onClose: () => void;
  } = $props();

  let action = $state<"mute" | "skip">("mute");
  let category = $state("");
  let startText = $state("");
  let endText = $state("");
  let word = $state("");
  let note = $state("");
  let error = $state("");

  function save() {
    const start = parseTime(startText);
    const end = parseTime(endText);
    if (start == null || end == null) {
      error = "Enter both times as HH:MM:SS (or a plain number of seconds).";
      return;
    }
    if (end <= start) {
      error = "End must be after start.";
      return;
    }
    if (category.trim() === "") {
      error = "Category can't be empty.";
      return;
    }
    error = "";
    onSave({
      start,
      end,
      action,
      category: category.trim(),
      word: word.trim() === "" ? null : word.trim(),
      note: note.trim() === "" ? null : note.trim(),
    });
  }
</script>

<div
  class="sheet-backdrop"
  use:portal
  role="button"
  tabindex="-1"
  aria-label="Close add cue"
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
>
  <div class="sheet" role="dialog" aria-label="Add cue" onclick={(e) => e.stopPropagation()} onkeydown={() => {}} tabindex="-1">
    <div class="sheet-drag-region" use:swipeDownToClose={onClose}>
      <div class="sheet-grabber"></div>

      <p class="section-header" style="margin:0">Add a cue</p>
    </div>

    {#if error}
      <p class="banner error">{error}</p>
    {/if}

    <div class="category-buttons">
      <button type="button" class="category-btn" class:selected={action === "mute"} onclick={() => (action = "mute")}>Mute</button>
      <button type="button" class="category-btn" class:selected={action === "skip"} onclick={() => (action = "skip")}>Skip</button>
    </div>

    <div class="stepper-row">
      <div style="flex:1; min-width:0">
        <div class="stepper-label">Category</div>
        <input
          class="stepper-value"
          style="width:100%; font-size:19px; background:transparent; padding:0; text-align:left"
          bind:value={category}
          placeholder="e.g. language-profanity"
          autocapitalize="none"
          autocorrect="off"
          spellcheck="false"
        />
      </div>
    </div>

    <div style="display:flex; gap:12px">
      <div class="stepper-row" style="flex:1">
        <div style="flex:1; min-width:0">
          <div class="stepper-label">Start</div>
          <input
            class="stepper-value time-input"
            style="width:100%; background:transparent; padding:0; text-align:left"
            bind:value={startText}
            placeholder="0:00"
          />
        </div>
      </div>
      <div class="stepper-row" style="flex:1">
        <div style="flex:1; min-width:0">
          <div class="stepper-label">End</div>
          <input
            class="stepper-value time-input"
            style="width:100%; background:transparent; padding:0; text-align:left"
            bind:value={endText}
            placeholder="0:00"
          />
        </div>
      </div>
    </div>

    {#if action === "mute"}
      <div class="stepper-row">
        <div style="flex:1; min-width:0">
          <div class="stepper-label">Word (optional)</div>
          <input
            class="stepper-value"
            style="width:100%; font-size:19px; background:transparent; padding:0; text-align:left"
            bind:value={word}
            placeholder="what gets muted"
          />
        </div>
      </div>
    {:else}
      <div class="stepper-row">
        <div style="flex:1; min-width:0">
          <div class="stepper-label">Note (optional)</div>
          <input
            class="stepper-value"
            style="width:100%; font-size:19px; background:transparent; padding:0; text-align:left"
            bind:value={note}
            placeholder="what the scene is"
          />
        </div>
      </div>
    {/if}

    <p class="footnote">Times as {"HH:MM:SS"} (or a plain number of seconds) -- must fall outside every other cue for this title/service.</p>

    <button type="button" class="btn-primary" onclick={save} disabled={busy}>{busy ? "Adding…" : "Add cue"}</button>
  </div>
</div>
