<!--
  LaunchScreen.svelte — Family Filter launch screen (design 10b).

  Drop-in, self-contained: no deps, no build config, scoped styles, plain CSS
  animations. Reads the app's own --display / --ui font tokens from app.css
  when present, with literal fallbacks so it also works standalone. Colors
  are flat literals rather than --accent/oklch tokens on purpose -- see the
  .launch comment below re: a WKWebView paint bug this design works around.

  Usage in src/routes/+page.svelte — replace the launching splash:

      import LaunchScreen from "$lib/components/LaunchScreen.svelte";
      ...
      {#if launching}
        <LaunchScreen />
      {:else if devicesOpen}

  Or hold it a beat past the device check so the animation always completes:

      let splashDone = $state(false);
      ...
      {#if launching || !splashDone}
        <LaunchScreen onDone={() => { splashDone = true; }} />
      {:else if devicesOpen}
-->
<script lang="ts">
  let { onDone, minDuration = 7800 }: { onDone?: () => void; minDuration?: number } = $props();

  const GLYPHS = ["!", "@", "#", "$", "%", "&", "*"];

  // Deterministic RNG so the field is identical every launch — it reads as a
  // designed pattern rather than noise that changes shape each cold start.
  let seed = 20260912;
  const rnd = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;

  type Mote = {
    ch: string; x: number; y: number; size: number; box: number;
    rot: number; driftX: number; driftY: number; opacity: number; delay: number;
  };

  // Jittered grid: random size, rotation and offset, but each glyph's
  // rotation-safe bounding circle stays inside its own cell, so no two
  // symbols can ever overlap.
  function buildField(w: number, h: number): Mote[] {
    seed = 20260912;
    const cols = 4;
    const cell = w / (cols - 0.7);
    const rows = Math.ceil(h / cell) + 1;
    const bandTop = h / 2 - 132;
    const bandBot = h / 2 + 132;
    const out: Mote[] = [];
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols + 1; c++) {
        const size = cell * (0.52 + rnd() * 0.18);
        const box = size * 1.42;
        const slack = Math.max(0, cell - box);
        const x = (c - 0.6) * cell + rnd() * slack;
        const y = (r - 0.7) * cell + rnd() * slack;
        const behind = y + box > bandTop && y < bandBot;
        out.push({
          ch: GLYPHS[Math.floor(rnd() * GLYPHS.length)],
          x, y, size, box,
          rot: rnd() * 360,
          driftX: (rnd() * 2 - 1) * 46,
          driftY: 28 + rnd() * 46,
          // glyphs sitting behind the wordmark stay quiet so the name holds
          opacity: behind ? 0.22 : 0.2 + rnd() * 0.3,
          delay: rnd() * 1500
        });
      }
    }
    return out;
  }

  let w = $state(typeof window === "undefined" ? 430 : window.innerWidth);
  let h = $state(typeof window === "undefined" ? 932 : window.innerHeight);
  const field = $derived(buildField(w, h));

  $effect(() => {
    const onResize = () => { w = window.innerWidth; h = window.innerHeight; };
    window.addEventListener("resize", onResize);
    const t = setTimeout(() => onDone?.(), minDuration);
    return () => { window.removeEventListener("resize", onResize); clearTimeout(t); };
  });
</script>

<div class="launch" role="img" aria-label="Family Filter">
  {#each field as m, i (i)}
    <span
      class="mote"
      aria-hidden="true"
      style="left:{m.x}px; top:{m.y}px; width:{m.box}px; height:{m.box}px;
             font-size:{m.size}px; --rot:{m.rot}deg; --dx:{m.driftX}px; --dy:{m.driftY}px;
             --op:{m.opacity}; animation-delay:{m.delay}ms"
    >{m.ch}</span>
  {/each}

  <div class="wordmark">
    <div class="line one">Family</div>
    <div class="line two">Filter</div>
  </div>

  <div class="verse">
    <p class="text">Ears to hear and eyes to see, both are gifts from the Lord</p>
    <p class="ref">Proverbs 20:12</p>
  </div>
</div>

<style>
  .launch {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    left: 0;
    z-index: 100;
    overflow: hidden;
    background: #3f8158;
    /* keeps the wordmark clear of notches and home indicators */
    padding: env(safe-area-inset-top) env(safe-area-inset-right)
             env(safe-area-inset-bottom) env(safe-area-inset-left);
  }

  /* The ~30 .mote spans below are direct children of .launch, NOT wrapped
     in their own full-size positioned container (there used to be a
     `.field { position:absolute; inset:0 }` wrapper here) -- confirmed
     on-device (iOS 17.2 Simulator WKWebView, via a bisected series of
     screenshots) that wrapper made .launch's OWN background silently fail
     to paint, while the wrapper's motes still rendered fine: an occlusion
     false-positive, where the compositor treats a full-size sibling/child
     layer as fully covering .launch and skips painting .launch's
     background, even though that layer is mostly transparent (each mote is
     20-50% opacity). Each mote is positioned via its own inline
     left/top/width/height, so it never needed the wrapper for layout
     anyway -- removing it was a pure win, not a tradeoff. */
  .mote {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: var(--ui, Figtree, system-ui, sans-serif);
    font-weight: 700;
    color: #2e6041;
    opacity: 0;
    transform: translate(var(--dx), var(--dy)) rotate(var(--rot)) scale(0.82);
    animation: mote-in 1100ms cubic-bezier(0.22, 0.61, 0.36, 1) forwards;
  }

  @keyframes mote-in {
    to {
      opacity: var(--op);
      transform: translate(0, 0) rotate(var(--rot)) scale(1);
    }
  }

  .wordmark {
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }

  .line {
    font-family: var(--display, "Instrument Serif", Georgia, serif);
    font-size: clamp(58px, 21vw, 92px);
    line-height: 1;
    letter-spacing: 0.01em;
    color: #fff;
    opacity: 0;
    transform: translateY(14px);
    animation: rise 850ms cubic-bezier(0.22, 0.61, 0.36, 1) forwards;
  }
  .one { animation-delay: 2700ms; }
  .two { animation-delay: 2920ms; }

  .verse {
    position: absolute;
    left: 44px;
    right: 44px;
    bottom: calc(92px + env(safe-area-inset-bottom));
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
  }

  /* full-opacity white — alpha-muted cream would not clear 4.5:1 on the green */
  .text,
  .ref {
    margin: 0;
    color: #fff;
    text-align: center;
    text-wrap: pretty;
    opacity: 0;
    transform: translateY(10px);
    animation: rise 1000ms cubic-bezier(0.22, 0.61, 0.36, 1) forwards;
  }

  .text {
    font-family: var(--ui, Figtree, system-ui, sans-serif);
    font-size: 15px;
    line-height: 1.65;
    animation-delay: 4600ms;
  }

  .ref {
    font-family: var(--ui, Figtree, system-ui, sans-serif);
    font-size: 13px;
    font-weight: 700;
    line-height: 1.4;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    animation-delay: 5000ms;
  }

  @keyframes rise {
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .mote,
    .line,
    .text,
    .ref {
      animation-duration: 1ms;
      animation-delay: 0ms;
    }
  }
</style>
