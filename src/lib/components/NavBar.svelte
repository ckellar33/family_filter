<script lang="ts">
  // Sticky header: an optional back button (left), the current screen's
  // title (center), and an optional Devices entry point (right) --
  // VidAngel's top-right "Services" button, renamed for what this app
  // actually manages.
  let {
    title,
    canGoBack = false,
    onBack,
    showDevices = false,
    onDevices,
  }: {
    title: string;
    canGoBack?: boolean;
    onBack?: () => void;
    showDevices?: boolean;
    onDevices?: () => void;
  } = $props();
</script>

<header class="navbar">
  {#if canGoBack}
    <button type="button" class="nav-back" onclick={onBack}>‹ Back</button>
  {/if}
  <h1>{title}</h1>
  {#if showDevices}
    <button type="button" class="nav-devices" onclick={onDevices}>Devices</button>
  {/if}
</header>
