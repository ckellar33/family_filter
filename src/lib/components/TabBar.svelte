<script lang="ts">
  import type { Tab } from "$lib/types";
  import TabIcon from "$lib/components/TabIcon.svelte";

  // Labels shortened from the screen names -- three tabs at 10.5px need to
  // fit on a phone without wrapping.
  const TABS: { id: Tab; icon: "controls" | "filters" | "create"; label: string }[] = [
    { id: "controls", icon: "controls", label: "Now Playing" },
    { id: "select-filter", icon: "filters", label: "Filters" },
    { id: "create-filter", icon: "create", label: "Create" },
  ];

  let { active, onSelect }: { active: Tab; onSelect: (tab: Tab) => void } = $props();
</script>

<nav class="tabbar">
  {#each TABS as tab (tab.id)}
    <button type="button" class="tab-btn" class:active={active === tab.id} onclick={() => onSelect(tab.id)}>
      <TabIcon name={tab.icon} active={active === tab.id} />
      <span>{tab.label}</span>
    </button>
  {/each}
</nav>
