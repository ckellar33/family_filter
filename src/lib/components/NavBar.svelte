<script lang="ts">
  // Sticky header: an optional back button (left), the current screen's
  // title (center), and an optional Devices entry point (right) --
  // VidAngel's top-right "Services" button, renamed for what this app
  // actually manages. The Devices button doubles as the connection
  // indicator: a live session shows a green dot and the device's host,
  // otherwise it reads "Not connected" in grey.
  //
  // Title sits on the LEFT next to the app mark (the mark gives way to the
  // back button on pushed screens); the Devices button stays hard right.
  let {
    title,
    canGoBack = false,
    onBack,
    showDevices = false,
    onDevices,
    connected = false,
    deviceLabel = "Devices",
  }: {
    title: string;
    canGoBack?: boolean;
    onBack?: () => void;
    showDevices?: boolean;
    onDevices?: () => void;
    connected?: boolean;
    deviceLabel?: string;
  } = $props();
</script>

<header class="navbar">
  <div class="nav-lead">
    {#if canGoBack}
      <button type="button" class="nav-back" onclick={onBack}>‹ Back</button>
    {:else}
      <span class="nav-mark" aria-hidden="true"></span>
    {/if}
    <h1>{title}</h1>
  </div>
  {#if showDevices}
    <button
      type="button"
      class="nav-devices"
      onclick={onDevices}
      style={connected ? "" : "color: var(--tertiary-label)"}
    >
      {#if connected}<span class="dot"></span>{/if}
      <span class="label">{deviceLabel}</span>
    </button>
  {/if}
</header>
