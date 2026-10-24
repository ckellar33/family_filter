<script lang="ts">
  // "Type in a cue by hand" sheet for the Filters tab's own "+ Add cue"
  // button -- deliberately separate from CueEditorSheet (timing only, for a
  // cue that already exists) and CueLabelSheet (category/word/note, for a
  // cue a live mark just created): this is the one place a cue's category,
  // action, *and* timing all get picked from scratch, so it asks for
  // everything at once.
  //
  // Two screens in one sheet: the form, and a category picker the category
  // row opens. Times are set on a wheel (TimeWheel) rather than typed.
  //
  // Built for back-filling many cues in a row, so a successful save does NOT
  // close the sheet: it confirms, moves Start to where the last cue ended,
  // and waits for the next one. "Done" closes it (see filter.svelte.ts's
  // addDetailCue for the other half).
  import { portal, swipeDownToClose } from "$lib/actions";
  import { fmtClock } from "$lib/format";
  import { livePosition } from "$lib/state/session.svelte";
  import { CATEGORY_GROUPS, CUSTOM_GROUP, findCategory, type CueAction } from "$lib/categories";
  import TimeWheel from "$lib/components/TimeWheel.svelte";

  let {
    busy = false,
    onSave,
    onClose,
  }: {
    busy?: boolean;
    // Resolves to null on success, or a message to show if the save failed.
    onSave: (cue: { start: number; end: number; action: CueAction; category: string; word: string | null; note: string | null }) => Promise<string | null>;
    onClose: () => void;
  } = $props();

  const MAX_SECONDS = 9 * 3600 + 59 * 60 + 59; // the wheel's 9:59:59 ceiling
  const DEFAULT_LENGTH = 3; // seconds, for a fresh Stop

  // Starts at wherever playback is, when something is playing: the usual
  // reason to add a cue is that you just watched it.
  const startAt = Math.min(Math.floor(livePosition() ?? 0), MAX_SECONDS - DEFAULT_LENGTH);

  let view = $state<"form" | "categories">("form");
  let action = $state<CueAction>("mute");
  let catKey = $state("language-profanity");
  let group = $state("language");
  let active = $state<"start" | "end">("start");
  let start = $state(startAt);
  let end = $state(startAt + DEFAULT_LENGTH);
  let word = $state("");
  let note = $state("");
  let custom = $state("");
  let added = $state("");
  let error = $state("");

  let found = $derived(findCategory(catKey));
  let catName = $derived(found?.item.name ?? catKey);
  let catGroup = $derived(found?.group ?? CUSTOM_GROUP);
  let valid = $derived(end > start);
  let length = $derived(end - start);
  let lengthText = $derived(length < 60 ? `${length}s` : `${Math.floor(length / 60)}m ${length % 60}s`);
  let shownGroup = $derived(CATEGORY_GROUPS.find((g) => g.id === group));

  let status = $derived.by(() => {
    if (error) return { text: error, tone: "bad" };
    if (!valid) return { text: "Stop must come after start", tone: "bad" };
    if (added) return { text: added, tone: "good" };
    return { text: `Length ${lengthText}`, tone: "" };
  });

  function setActiveTime(seconds: number) {
    const v = Math.max(0, Math.min(MAX_SECONDS, seconds));
    added = "";
    error = "";
    if (active === "start") {
      start = v;
      // Dragging Start past Stop carries Stop along rather than flagging an
      // error on every step of the drag.
      if (end <= start) end = Math.min(MAX_SECONDS, start + DEFAULT_LENGTH);
    } else {
      end = v;
    }
  }

  function openPicker() {
    group = found?.group.id ?? CUSTOM_GROUP.id;
    view = "categories";
  }

  function pickCategory(key: string, act: CueAction) {
    catKey = key;
    action = act;
    added = "";
    view = "form";
  }

  function useCustom() {
    const key = custom.trim().toLowerCase();
    if (key === "") return;
    catKey = key;
    added = "";
    view = "form";
  }

  async function save() {
    if (!valid || busy) return;
    const cue = {
      start,
      end,
      action,
      category: catKey,
      word: action === "mute" && word.trim() !== "" ? word.trim() : null,
      note: action === "skip" && note.trim() !== "" ? note.trim() : null,
    };
    const err = await onSave(cue);
    if (err) {
      error = err;
      return;
    }
    error = "";
    added = `Added · ${catName} · ${fmtClock(cue.start)} to ${fmtClock(cue.end)}`;
    // Next cue most likely starts where this one ended.
    start = Math.min(cue.end, MAX_SECONDS - DEFAULT_LENGTH);
    end = start + DEFAULT_LENGTH;
    active = "start";
    word = "";
    note = "";
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
  <div class="sheet add-cue" role="dialog" aria-label="Add cue" onclick={(e) => e.stopPropagation()} onkeydown={() => {}} tabindex="-1">
    {#if view === "form"}
      <div class="sheet-drag-region" use:swipeDownToClose={onClose}>
        <div class="sheet-grabber"></div>
        <h2 class="sheet-heading">Add a cue</h2>
      </div>
      <button type="button" class="top-btn right" onclick={onClose}>{added ? "Done" : "Cancel"}</button>

      <button type="button" class="cat-row" onclick={openPicker} aria-label="Category: {catGroup.label}, {catName}. Change category">
        <span class="icon-chip">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={catGroup.icon}></path></svg>
        </span>
        <span class="cat-text">
          <span class="mono-label">Category · {catGroup.label}</span>
          <span class="cat-name">{catName}</span>
        </span>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 6l6 6-6 6"></path></svg>
      </button>

      <div class="category-buttons">
        <button type="button" class="category-btn" class:selected={action === "mute"} aria-pressed={action === "mute"} onclick={() => (action = "mute")}>Mute</button>
        <button type="button" class="category-btn" class:selected={action === "skip"} aria-pressed={action === "skip"} onclick={() => (action = "skip")}>Skip</button>
      </div>

      <div class="time-cards">
        <button type="button" class="time-card" class:on={active === "start"} aria-pressed={active === "start"} onclick={() => (active = "start")}>
          <span class="mono-label">Start</span>
          <span class="time-text">{fmtClock(start)}</span>
        </button>
        <button type="button" class="time-card" class:on={active === "end"} aria-pressed={active === "end"} onclick={() => (active = "end")}>
          <span class="mono-label">Stop</span>
          <span class="time-text">{fmtClock(end)}</span>
        </button>
      </div>

      <TimeWheel value={active === "start" ? start : end} onchange={setActiveTime} />

      <p class="status {status.tone}" role="status">{status.text}</p>

      <div class="stepper-row">
        <div style="flex:1; min-width:0">
          <div class="stepper-label">{action === "mute" ? "Word (optional)" : "Note (optional)"}</div>
          {#if action === "mute"}
            <input class="stepper-value plain-input" bind:value={word} placeholder="what gets muted" autocapitalize="none" autocorrect="off" spellcheck="false" />
          {:else}
            <input class="stepper-value plain-input" bind:value={note} placeholder="what the scene is" />
          {/if}
        </div>
      </div>

      <button type="button" class="btn-primary" onclick={save} disabled={busy || !valid}>{busy ? "Adding…" : "Add cue"}</button>
    {:else}
      <div class="sheet-drag-region" use:swipeDownToClose={onClose}>
        <div class="sheet-grabber"></div>
        <h2 class="sheet-heading indented">Category</h2>
      </div>
      <button type="button" class="top-btn left" onclick={() => (view = "form")} aria-label="Back to add a cue">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 6l-6 6 6 6"></path></svg>
      </button>

      <div class="tabs" role="tablist" aria-label="Category groups">
        {#each [...CATEGORY_GROUPS, CUSTOM_GROUP] as g (g.id)}
          <button type="button" role="tab" class="tab" class:on={group === g.id} aria-selected={group === g.id} onclick={() => (group = g.id)}>{g.label}</button>
        {/each}
      </div>

      {#if shownGroup}
        <div class="cat-list">
          {#each shownGroup.items as it (it.key)}
            <button type="button" class="cat-item" class:on={catKey === it.key} aria-pressed={catKey === it.key} onclick={() => pickCategory(it.key, it.action)}>
              <span class="item-name">{it.name}</span>
              <span class="kind-pill {it.action}">{it.action}</span>
              <span class="check" class:on={catKey === it.key}>
                {#if catKey === it.key}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"></path></svg>
                {/if}
              </span>
            </button>
          {/each}
        </div>
      {:else}
        <div class="cat-list">
          <div class="stepper-row">
            <div style="flex:1; min-width:0">
              <div class="stepper-label">Category name</div>
              <input class="stepper-value plain-input" bind:value={custom} placeholder="e.g. political themes" autocapitalize="none" autocorrect="off" spellcheck="false" />
            </div>
          </div>
          <p class="footnote">Saved lowercase, exactly as typed. Other people's copies of the filter file won't have a label for it, so prefer a built-in category when one fits.</p>
          <button type="button" class="btn-primary" onclick={useCustom} disabled={custom.trim() === ""}>Use this category</button>
        </div>
      {/if}

      <p class="footnote">The category sets a default action: language mutes, everything else skips. You can flip it on the previous screen.</p>
    {/if}
  </div>
</div>

<style>
  .add-cue {
    position: relative; /* anchors the Cancel / Back buttons */
    gap: 12px;
  }
  .sheet-heading {
    margin: 0;
    font-family: var(--display);
    font-size: 30px;
    font-weight: 400;
    line-height: 1.1;
    color: var(--label);
    padding-top: 8px;
  }
  .sheet-heading.indented {
    padding-left: 36px;
  }
  /* Sits over the (inert) drag region's right/left edge; a sibling rather
     than a child so the swipe-to-close gesture never sees it. */
  .top-btn {
    all: unset;
    box-sizing: border-box;
    position: absolute;
    top: 22px;
    min-height: 44px;
    min-width: 44px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 14.5px;
    font-weight: 600;
    color: var(--secondary-label);
    cursor: pointer;
  }
  .top-btn.right {
    right: 18px;
  }
  .top-btn.left {
    left: 12px;
    color: var(--label);
  }
  .top-btn:focus-visible,
  .cat-row:focus-visible,
  .time-card:focus-visible,
  .tab:focus-visible,
  .cat-item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .mono-label {
    font-family: var(--mono);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--secondary-label);
  }

  .cat-row {
    all: unset;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 62px;
    padding: 10px 14px;
    background: var(--card-bg);
    border: 1px solid rgba(0, 0, 0, 0.09);
    border-radius: var(--r-lg);
    color: var(--secondary-label);
    cursor: pointer;
  }
  .icon-chip {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 12px;
    background: var(--grouped-bg);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--label);
  }
  .cat-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .cat-name {
    font-size: 17px;
    font-weight: 700;
    line-height: 1.15;
    color: var(--label);
  }

  .time-cards {
    display: flex;
    gap: 10px;
  }
  .time-card {
    all: unset;
    box-sizing: border-box;
    flex: 1;
    min-height: 68px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 14px;
    border-radius: var(--r-lg);
    border: 2px solid var(--separator);
    background: var(--card-bg);
    cursor: pointer;
  }
  .time-card.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .time-text {
    font-family: var(--display);
    font-size: 30px;
    line-height: 1.05;
    color: var(--label);
    font-variant-numeric: tabular-nums;
  }

  .status {
    margin: 0 2px;
    min-height: 18px;
    font-size: 13px;
    font-weight: 600;
    line-height: 18px;
    color: var(--secondary-label);
  }
  .status.bad {
    color: oklch(0.45 0.12 30);
  }
  .status.good {
    color: oklch(0.42 0.09 152);
  }

  .plain-input {
    width: 100%;
    font-family: var(--ui);
    font-size: 17px;
    font-weight: 500;
    background: transparent;
    border: 0;
    outline: 0;
    padding: 0;
    text-align: left;
    color: var(--label);
  }

  .tabs {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    margin: 0 -22px;
    padding: 2px 22px;
    scrollbar-width: none;
  }
  .tab {
    all: unset;
    box-sizing: border-box;
    flex: none;
    min-height: 44px;
    padding: 0 16px;
    border-radius: var(--r-pill);
    display: flex;
    align-items: center;
    font-size: 14px;
    font-weight: 700;
    background: var(--card-bg);
    border: 1px solid rgba(0, 0, 0, 0.09);
    color: var(--label);
    cursor: pointer;
  }
  .tab.on {
    background: var(--label);
    border-color: var(--label);
    color: var(--bg);
  }

  .cat-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 330px; /* keeps the sheet from jumping between groups */
  }
  .cat-item {
    all: unset;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 56px;
    padding: 10px 14px;
    border-radius: var(--r-lg);
    border: 1px solid rgba(0, 0, 0, 0.09);
    background: var(--card-bg);
    cursor: pointer;
  }
  .cat-item.on {
    border: 2px solid var(--accent);
    background: var(--accent-soft);
  }
  .item-name {
    flex: 1;
    min-width: 0;
    font-size: 16px;
    font-weight: 700;
    color: var(--label);
  }
  .kind-pill {
    flex: none;
    font-family: var(--mono);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 5px 8px;
    border-radius: var(--r-pill);
  }
  .kind-pill.mute {
    background: #f8ebd0;
    color: #7a4d0b;
  }
  .kind-pill.skip {
    background: #f7e1db;
    color: #8a3523;
  }
  .check {
    flex: none;
    width: 22px;
    height: 22px;
    box-sizing: border-box;
    border-radius: 50%;
    border: 1.5px solid rgba(0, 0, 0, 0.2);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .check.on {
    border: 0;
    background: var(--accent);
  }
</style>
