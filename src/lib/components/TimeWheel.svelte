<script lang="ts">
  // iOS-style hours / minutes / seconds wheel for picking one time as a
  // whole number of seconds. Drag a column, scroll the mouse wheel over it,
  // or tap a neighbouring number. Hours run 0-9 (a feature film never needs
  // more), minutes and seconds 0-59, and every column wraps.
  //
  // Controlled: `value` goes in, `onchange` reports the new total. The
  // parent decides what a change means (the Add cue sheet bumps Stop when
  // Start passes it), so this never keeps a copy of the time itself.
  let { value, onchange }: { value: number; onchange: (seconds: number) => void } = $props();

  const ROW = 34; // px per number; the sheet's CSS uses the same height
  const OFFSETS = [-3, -2, -1, 0, 1, 2, 3];

  const COLS = [
    { key: "h", range: 10, label: "Hr", aria: "Hours" },
    { key: "m", range: 60, label: "Min", aria: "Minutes" },
    { key: "s", range: 60, label: "Sec", aria: "Seconds" },
  ] as const;
  type ColKey = (typeof COLS)[number]["key"];

  let parts = $derived({
    h: Math.floor(value / 3600),
    m: Math.floor((value % 3600) / 60),
    s: value % 60,
  });

  // Live drag: which column is moving and how far (px) it sits off its
  // resting row, so the numbers follow the finger between whole steps.
  let dragCol = $state<ColKey | null>(null);
  let dragOff = $state(0);
  // Plain (non-reactive) bookkeeping for the gesture in flight.
  let gesture: { col: ColKey; y0: number; v0: number; id: number; moved: boolean } | null = null;
  let suppressClick = false;
  let wheelAcc = 0;

  const wrap = (v: number, range: number) => ((v % range) + range) % range;

  function setPart(key: ColKey, v: number) {
    const next = { ...parts, [key]: v };
    onchange(next.h * 3600 + next.m * 60 + next.s);
  }

  function rowsFor(range: number, val: number) {
    return OFFSETS.map((d) => ({ d, v: wrap(val + d, range) }));
  }

  function rowOpacity(d: number) {
    return [1, 0.5, 0.2, 0.07][Math.abs(d)];
  }

  function down(e: PointerEvent, col: (typeof COLS)[number]) {
    gesture = { col: col.key, y0: e.clientY, v0: parts[col.key], id: e.pointerId, moved: false };
  }

  function move(e: PointerEvent, col: (typeof COLS)[number]) {
    const g = gesture;
    if (!g || g.col !== col.key) return;
    const dy = e.clientY - g.y0;
    if (!g.moved) {
      if (Math.abs(dy) < 4) return; // still a tap
      g.moved = true;
      try {
        (e.currentTarget as HTMLElement).setPointerCapture(g.id);
      } catch {
        // capture is a nicety (keeps the drag if the finger leaves the column)
      }
    }
    const steps = Math.round(-dy / ROW);
    setPart(col.key, wrap(g.v0 + steps, col.range));
    dragCol = col.key;
    dragOff = dy + steps * ROW;
  }

  function up() {
    if (gesture?.moved) {
      // A drag that ends over a number would otherwise also "click" it.
      suppressClick = true;
      setTimeout(() => (suppressClick = false), 0);
    }
    gesture = null;
    dragCol = null;
    dragOff = 0;
  }

  function pick(key: ColKey, v: number) {
    if (suppressClick) return;
    setPart(key, v);
  }

  // Svelte's onwheel is passive, which can't stop the sheet behind the
  // wheel from scrolling too -- hence a non-passive listener.
  function wheelStep(node: HTMLElement, col: (typeof COLS)[number]) {
    function onWheel(e: WheelEvent) {
      e.preventDefault();
      wheelAcc += e.deltaY;
      if (Math.abs(wheelAcc) < 30) return;
      const dir = wheelAcc > 0 ? 1 : -1;
      wheelAcc = 0;
      setPart(col.key, wrap(parts[col.key] + dir, col.range));
    }
    node.addEventListener("wheel", onWheel, { passive: false });
    return { destroy: () => node.removeEventListener("wheel", onWheel) };
  }
</script>

<div class="wheel">
  <div class="wheel-labels">
    {#each COLS as col, i (col.key)}
      <span class="wheel-label">{col.label}</span>
      {#if i < COLS.length - 1}<span class="wheel-gap"></span>{/if}
    {/each}
  </div>

  <div class="wheel-body">
    <div class="wheel-band"></div>
    {#each COLS as col, i (col.key)}
      <div
        class="wheel-col"
        role="group"
        aria-label="{col.aria}, drag or scroll"
        onpointerdown={(e) => down(e, col)}
        onpointermove={(e) => move(e, col)}
        onpointerup={up}
        onpointercancel={up}
        use:wheelStep={col}
      >
        <div
          class="wheel-list"
          class:dragging={dragCol === col.key}
          style="transform: translateY({dragCol === col.key ? dragOff : 0}px)"
        >
          {#each rowsFor(col.range, parts[col.key]) as row (row.d)}
            <button
              type="button"
              class="wheel-row"
              class:center={row.d === 0}
              style="opacity: {rowOpacity(row.d)}"
              aria-label="Set {col.aria.toLowerCase()} to {row.v}"
              onclick={() => pick(col.key, row.v)}
            >
              {row.v.toString().padStart(2, "0")}
            </button>
          {/each}
        </div>
      </div>
      {#if i < COLS.length - 1}<span class="wheel-colon">:</span>{/if}
    {/each}
  </div>
</div>

<style>
  .wheel {
    background: var(--card-bg);
    border: 1px solid var(--separator);
    border-radius: 20px;
    padding: 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .wheel-labels {
    display: flex;
    justify-content: center;
    align-items: center;
  }
  .wheel-label {
    width: 84px;
    text-align: center;
    font-family: var(--mono);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--secondary-label);
  }
  .wheel-gap {
    width: 16px;
  }
  .wheel-body {
    position: relative;
    display: flex;
    justify-content: center;
    align-items: center;
    height: 170px; /* five visible rows */
  }
  .wheel-band {
    position: absolute;
    left: 0;
    right: 0;
    top: 68px;
    height: 34px;
    background: var(--grouped-bg);
    border-radius: 10px;
  }
  .wheel-col {
    position: relative;
    width: 84px;
    height: 170px;
    overflow: hidden;
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
    cursor: ns-resize;
  }
  .wheel-list {
    position: absolute;
    left: 0;
    right: 0;
    top: -34px; /* one extra row above, so a drag never shows a gap */
    transition: transform 0.2s cubic-bezier(0.32, 0.72, 0, 1);
  }
  .wheel-list.dragging {
    transition: none;
  }
  .wheel-row {
    all: unset;
    box-sizing: border-box;
    display: block;
    width: 100%;
    height: 34px;
    line-height: 34px;
    text-align: center;
    font-family: var(--display);
    font-size: 24px;
    color: var(--label);
    font-variant-numeric: tabular-nums;
    cursor: pointer;
  }
  .wheel-row.center {
    font-size: 32px;
  }
  .wheel-row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    border-radius: 8px;
  }
  .wheel-colon {
    width: 16px;
    text-align: center;
    font-family: var(--display);
    font-size: 28px;
    line-height: 1;
    color: var(--secondary-label);
  }
</style>
