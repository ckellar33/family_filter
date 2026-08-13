<script lang="ts">
  // Cue editor sheet -- opens from a recorded cue in Create Filter's table.
  // Nudges the cue's edges a second at a time against a ±30s window, so
  // "how much am I trimming" is visible rather than guessed from two
  // numbers. Commits through the same creation_update_cue / creation_delete_cue
  // path the inline table inputs already use.
  //
  // Deliberately dumb: it owns only the draft values while open and hands
  // the result back through onSave -- the caller decides what to persist.
  // (There is currently no backend command to retime a cue in an *applied*
  // filter file, only in a draft, which is why Select Filter's cue rows
  // still only toggle enabled/disabled.)
  import { fmtTime, parseTime } from "$lib/format";
  import type { CategoryDef } from "$lib/types";

  let {
    start,
    end,
    category,
    action,
    categories,
    busy = false,
    onSave,
    onDelete,
    onClose,
  }: {
    start: number;
    end: number;
    category: string;
    action: "mute" | "skip";
    categories: CategoryDef[];
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

    <div style="display:flex; gap:10px">
      <button type="button" class="btn-destructive" style="width:auto" onclick={onDelete} disabled={busy}>Delete</button>
      <button type="button" class="btn-primary" onclick={() => onSave({ start: draftStart, end: draftEnd })} disabled={busy}>
        Save cue
      </button>
    </div>
  </div>
</div>
