<script lang="ts">
  // Cue editor sheet -- opens from a recorded cue in Create Filter's table,
  // or a cue row in Select Filter's detail view. Nudges the cue's edges a
  // second at a time against a ±30s window, so "how much am I trimming" is
  // visible rather than guessed from two numbers.
  //
  // Deliberately dumb: it owns only the draft values while open and hands
  // the result back through onSave/onDelete -- the caller decides what to
  // persist. Create Filter commits through creation_update_cue/
  // creation_delete_cue (a recording draft); Select Filter commits through
  // update_filter_cue/delete_filter_cue (the active, already-applied list,
  // persisted straight back to its file) -- see filter.svelte.ts's
  // updateDetailCueTime/deleteDetailCue.
  import { fmtTime, parseTime } from "$lib/format";
  import { session, doSeek } from "$lib/state/session.svelte";
  import type { CategoryDef } from "$lib/types";

  let {
    start,
    end,
    category,
    action,
    // Unused in this sheet today (nothing here lets you re-pick a cue's
    // category) -- kept as a prop for a future "change category" control
    // rather than plumbed through and dropped, so it's optional for callers
    // (like SelectFilterPage) that have no CategoryDef list of their own.
    categories = [],
    busy = false,
    onSave,
    onDelete,
    onClose,
  }: {
    start: number;
    end: number;
    category: string;
    action: "mute" | "skip";
    categories?: CategoryDef[];
    busy?: boolean;
    onSave: (cue: { start: number; end: number }) => void;
    onDelete: () => void;
    onClose: () => void;
  } = $props();

  // Fixed for the lifetime of the sheet, so nudging visibly moves the bar
  // inside a stable window rather than rescaling under your thumb.
  const w0 = Math.max(0, start - 30);
  const w1 = end + 30;
  const span = Math.max(1, w1 - w0);

  let draftStart = $state(start);
  let draftEnd = $state(end);

  let left = $derived(((draftStart - w0) / span) * 100);
  let width = $derived(((draftEnd - draftStart) / span) * 100);

  function nudge(field: "start" | "end", delta: number) {
    if (field === "start") draftStart = Math.max(w0, Math.min(draftEnd - 1, draftStart + delta));
    else draftEnd = Math.max(draftStart + 1, Math.min(w1, draftEnd + delta));
  }

  function typeTime(field: "start" | "end", text: string) {
    const seconds = parseTime(text);
    if (seconds == null) return;
    if (field === "start") draftStart = Math.min(draftEnd - 1, seconds);
    else draftEnd = Math.max(draftStart + 1, seconds);
  }

  // How far ahead of the cue's (draft) start to land -- enough runway to
  // hear/see normal content before the cue should kick in, without
  // rewinding so far the test drags on.
  const TEST_LEAD_IN = 5;

  // Needs the live (MRP/AirPlay) transport, not just Companion -- `doSeek`
  // dispatches MRP's absolute SeekToPlaybackPosition, same as a real skip
  // cue (see `control_seek`'s doc), which only a live session exposes.
  let canTest = $derived(session.page === "control" && session.hasLive);

  // Absolute jump via MRP's SeekToPlaybackPosition -- *not* Companion's
  // relative SkipBy (`doSkip`), which some apps (confirmed against
  // Disney+) only ever honor as a fixed, much-shorter-than-requested hop,
  // so a large skip landed well short of `draftStart - TEST_LEAD_IN`
  // instead of at it. Uses the *draft* value, not the saved cue, so
  // nudging start/end and testing again reflects whatever's on screen
  // right now.
  async function jumpToTest() {
    const target = Math.max(0, draftStart - TEST_LEAD_IN);
    await doSeek(target);
  }
</script>

<div
  class="sheet-backdrop"
  role="button"
  tabindex="-1"
  aria-label="Close cue editor"
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
>
  <div class="sheet" role="dialog" aria-label="Edit cue" onclick={(e) => e.stopPropagation()} onkeydown={() => {}} tabindex="-1">
    <div class="sheet-grabber"></div>

    <div class="list-row static" style="padding:0; min-height:auto">
      <span class="cue-pill" data-action={action}>{action === "mute" ? "MUTE" : "SKIP"}</span>
      <span style="flex:1; padding-left:11px; font-size:19px; font-weight:700; text-transform:capitalize">{category}</span>
      <span class="cue-time">{fmtTime(draftEnd - draftStart)} long</span>
    </div>

    <div class="cue-window">
      <div class="cue-window-track">
        <div class="cue-window-fill" data-action={action} style={`left:${left}%; width:${width}%`}></div>
      </div>
      <div class="cue-window-scale"><span>{fmtTime(w0)}</span><span>{fmtTime(w1)}</span></div>
    </div>

    {#each [{ label: "Start", field: "start" as const, value: draftStart }, { label: "End", field: "end" as const, value: draftEnd }] as row (row.field)}
      <div class="stepper-row">
        <div style="flex:1; min-width:0">
          <div class="stepper-label">{row.label}</div>
          <input
            class="stepper-value time-input"
            style="width:5.5em; background:transparent; padding:0; text-align:left; min-height:auto"
            value={fmtTime(row.value)}
            onchange={(e) => typeTime(row.field, (e.target as HTMLInputElement).value)}
          />
        </div>
        <button type="button" class="stepper-btn" onclick={() => nudge(row.field, -1)} aria-label={`${row.label} minus one second`}>−</button>
        <button type="button" class="stepper-btn" onclick={() => nudge(row.field, 1)} aria-label={`${row.label} plus one second`}>+</button>
      </div>
    {/each}

    <p class="footnote">One second at a time, or type the time directly as {fmtTime(draftStart)}.</p>

    <button type="button" class="btn-secondary" onclick={jumpToTest} disabled={!canTest || session.controlBusy}>
      ↻ Jump to {TEST_LEAD_IN}s before start
    </button>
    <p class="footnote">
      {canTest
        ? `Skips playback to ${fmtTime(Math.max(0, draftStart - TEST_LEAD_IN))} so you can watch this cue trigger.`
        : "Open Controls with a live device to test this cue against playback."}
    </p>

    <div style="display:flex; gap:10px">
      <button type="button" class="btn-destructive" style="width:auto" onclick={onDelete} disabled={busy}>Delete</button>
      <button type="button" class="btn-primary" onclick={() => onSave({ start: draftStart, end: draftEnd })} disabled={busy}>
        Save cue
      </button>
    </div>
  </div>
</div>
