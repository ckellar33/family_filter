<script lang="ts">
  import type { Tab } from "$lib/types";

  const TABS: { id: Tab; icon: string; label: string }[] = [
    { id: "controls", icon: "🎛️", label: "Open Controls" },
    { id: "select-filter", icon: "🎬", label: "Select Filter" },
    { id: "create-filter", icon: "✏️", label: "Create Filter" },
  ];

  let { active, onSelect }: { active: Tab; onSelect: (tab: Tab) => void } = $props();
</script>

<nav class="tabbar">
  {#each TABS as tab (tab.id)}
    <button type="button" class="tab-btn" class:active={active === tab.id} onclick={() => onSelect(tab.id)}>
      <span class="tab-icon">{tab.icon}</span>
      <span>{tab.label}</span>
    </button>
  {/each}
</nav>
