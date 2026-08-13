<script lang="ts">
  // The four dead ends the app can land in, in one component: no paired
  // device, no filter files, a control session that dropped, and Companion
  // paired without MRP/AirPlay (so there's no live position to time cues
  // against). One honest sentence and one way out each -- replaces the
  // bare `.hint.centered` + button pairs that used to stand in for these.
  type Kind = "no-device" | "no-filters" | "lost" | "companion-only";

  let {
    kind,
    detail = "",
    onPrimary,
    onSecondary,
  }: {
    kind: Kind;
    detail?: string;
    onPrimary?: () => void;
    onSecondary?: () => void;
  } = $props();

  const COPY: Record<
    Kind,
    { icon: string; tone: string; title: string; body: string; detail: string; primary: string; secondary: string }
  > = {
    "no-device": {
      icon: "📺",
      tone: "neutral",
      title: "No Apple TV yet",
      body: "Family Filter needs to pair with an Apple TV before it can mute or skip anything.",
      detail: "",
      primary: "Pair an Apple TV",
      secondary: "Browse filters in the meantime",
    },
    "no-filters": {
      icon: "🎬",
      tone: "neutral",
      title: "No filter files yet",
      body: "Add a filter file or folder, or record your own from what's playing right now.",
      detail: "Filter files are plain JSON: a title, an optional service, and a list of cues.",
      primary: "Add a filter file…",
      secondary: "Record one instead",
    },
    lost: {
      icon: "⚡",
      tone: "error",
      title: "Lost the Apple TV",
      body: "The connection dropped, so nothing is being muted or skipped right now.",
      detail: "",
      primary: "Reconnect",
      secondary: "Open Devices",
    },
    "companion-only": {
      icon: "⏱",
      tone: "warn",
      title: "Companion only",
      body: "Mute and skip work, but without MRP or AirPlay there's no live position — so cues can't fire on their own.",
      detail: "Pair MRP or AirPlay from Devices to get the timeline back.",
      primary: "Pair MRP or AirPlay",
      secondary: "Keep using manual controls",
    },
  };

  let copy = $derived(COPY[kind]);
  let detailText = $derived(detail || copy.detail);
</script>

<section class="empty-state">
  <div class="empty-icon" data-tone={copy.tone}>{copy.icon}</div>

  <div>
    <h2 class="empty-title">{copy.title}</h2>
    <p class="empty-body">{copy.body}</p>
  </div>

  {#if detailText}
    <p class="empty-detail">{detailText}</p>
  {/if}

  <div class="empty-actions">
    <button type="button" class="btn-primary" onclick={onPrimary}>{copy.primary}</button>
    <button type="button" class="link-btn" onclick={onSecondary}>{copy.secondary}</button>
  </div>
</section>
