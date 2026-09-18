<script lang="ts">
  // Label sheet -- what a mark becomes a *cue* in. Opens twice in a cue's
  // life, and asks a different question each time:
  //
  //   mode="new"   straight after a mark landed. Skip asks which category
  //                and an optional note; Mute asks only what kind of
  //                language it was. Discard deletes the just-recorded cue.
  //   mode="edit"  from a row in Recorded. Same controls, plus -- for a
  //                mute cue -- the word list for its kind, which is
  //                deliberately *not* asked for at record time: picking
  //                the exact word is a "later, on the couch" job, and
  //                demanding it mid-scene is how marks get skipped.
  //
  // The range shown is always the frozen one the mark captured. This sheet
  // never edits timing (that's CueEditorSheet, reachable from the footer in
  // edit mode) -- it only ever writes category / note / word, through
  // creation_set_cue_label.
  import { fmtTime, censorWord } from "$lib/format";
  import { SKIP_CATEGORIES, LANGUAGE_KINDS, languageKindFor, MUTE_MARK_SECS } from "$lib/state/creation.svelte";
  import { portal } from "$lib/actions";

  let {
    mode,
    action,
    start,
    end,
    category,
    note = null,
    word = null,
    busy = false,
    onSave,
    onDiscard,
    onClose,
    onRetime,
  }: {
    mode: "new" | "edit";
    action: "mute" | "skip";
    start: number;
    end: number;
    category: string;
    note?: string | null;
    word?: string | null;
    busy?: boolean;
    onSave: (label: { category: string; note: string; word: string }) => void;
    onDiscard: () => void;
    onClose: () => void;
    onRetime?: () => void;
  } = $props();

  const isMute = action === "mute";

  // Local drafts -- nothing is committed until Save/Done, so backing out of
  // a half-made edit leaves the cue exactly as it was.
  let draftCategory = $state(isMute ? languageKindFor(category).category : category);
  let draftNote = $state(note ?? "");

  // Words are stored on the cue as one comma-separated string (see
  // filter::Cue::word) -- a single 8-second mute window can genuinely
  // contain two words, and one field that reads "shit, damn" beats two
  // cues stacked on the same range. Split into a set while the sheet is
  // open, re-joined on save.
  let picked = $state(new Set((word ?? "").split(",").map((w) => w.trim().toLowerCase()).filter(Boolean)));

  let kind = $derived(languageKindFor(draftCategory));

  // Placeholder category from the mark itself -- treat it as "nothing
  // chosen yet" rather than showing an "unlabeled" chip as if it were a
  // real answer.
  let chosenSkip = $derived(SKIP_CATEGORIES.includes(draftCategory) ? draftCategory : "");

  function toggleWord(w: string) {
    const next = new Set(picked);
    if (next.has(w)) next.delete(w);
    else next.add(w);
    picked = next;
  }

  // Changing the kind clears the words: the lists don't overlap, so a word
  // picked under Profanity is meaningless once the cue says Blasphemy.
  function pickKind(next: string) {
    if (next === draftCategory) return;
    draftCategory = next;
    picked = new Set();
  }

  function save() {
    onSave({
      category: isMute ? draftCategory : chosenSkip,
      note: draftNote.trim(),
      // Only meaningful for a mute cue; a skip cue has no word field to
      // write, so send "" and let the backend leave it alone.
      word: isMute ? [...picked].join(", ") : "",
    });
  }

  let canSave = $derived(isMute ? !!draftCategory : !!chosenSkip);

  let title = $derived(
    mode === "edit" ? (isMute ? "Which words?" : "Skip details") : isMute ? "What kind of language?" : "What did we skip?",
  );
</script>

<div
  class="sheet-backdrop"
  use:portal
  role="button"
  tabindex="-1"
  aria-label="Close label sheet"
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
>
  <div class="sheet" role="dialog" aria-label="Label cue" onclick={(e) => e.stopPropagation()} onkeydown={() => {}} tabindex="-1">
    <div class="sheet-grabber"></div>

    <div class="sheet-head">
      <h2 class="sheet-title">{title}</h2>
      <span class="cue-time">{fmtTime(start)} – {fmtTime(end)}</span>
    </div>

    {#if isMute}
      <div class="chip-group">
        <div class="chip-group-head">
          <span class="section-header" style="margin:0">Kind of language</span>
          <span class="chip-group-note">
            {mode === "edit" ? "change the kind if it was tagged wrong" : "pick the words later, from Recorded"}
          </span>
        </div>
        <div class="chips">
          {#each LANGUAGE_KINDS as k (k.category)}
            <button type="button" class="chip" class:on={draftCategory === k.category} onclick={() => pickKind(k.category)}>
              {k.label}
            </button>
          {/each}
        </div>
      </div>

      {#if mode === "edit"}
        <div class="chip-group">
          <div class="chip-group-head">
            <span class="section-header" style="margin:0">Words</span>
            <span class="chip-group-note">{kind.censor ? "muted everywhere in this cue" : "shown as recorded"}</span>
          </div>
          <div class="chips">
            {#each kind.words as w (w)}
              <button type="button" class="chip chip-mono" class:on={picked.has(w)} onclick={() => toggleWord(w)}>
                {kind.censor ? censorWord(w) : w}
              </button>
            {/each}
          </div>
        </div>
      {/if}
    {:else}
      <div class="chip-group">
        <span class="section-header" style="margin:0">Category</span>
        <div class="chips">
          {#each SKIP_CATEGORIES as c (c)}
            <button type="button" class="chip" class:on={chosenSkip === c} onclick={() => (draftCategory = c)}>{c}</button>
          {/each}
        </div>
      </div>

      <div class="chip-group">
        <span class="section-header" style="margin:0">Note — optional</span>
        <input class="field" placeholder="e.g. bar fight, brief" bind:value={draftNote} />
      </div>
    {/if}

    {#if mode === "new" && isMute}
      <p class="footnote">Stamped {MUTE_MARK_SECS}s from the tap. Nudge the edges later from Recorded if it caught too much.</p>
    {/if}

    {#if mode === "edit" && onRetime}
      <button type="button" class="btn-secondary" onclick={onRetime} disabled={busy}>Nudge the timing…</button>
    {/if}

    <div style="display:flex; gap:10px">
      <button type="button" class={mode === "new" ? "btn-destructive" : "btn-secondary"} style="width:auto" onclick={mode === "new" ? onDiscard : onClose} disabled={busy}>
        {mode === "new" ? "Discard" : "Close"}
      </button>
      <button type="button" class="btn-primary" onclick={save} disabled={busy || !canSave}>
        {mode === "new" ? "Save cue" : "Done"}
      </button>
    </div>
  </div>
</div>
