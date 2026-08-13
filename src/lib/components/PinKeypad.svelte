<script lang="ts">
  // Four-box PIN entry with an on-screen keypad -- replaces the single
  // `.pin-input` text field in the pairing wizard. Submits itself on the
  // fourth digit (the Apple TV's code is always four digits), so there's
  // no separate Submit button to reach for mid-pairing.
  let { value = $bindable(""), onSubmit }: { value?: string; onSubmit: () => void } = $props();

  const KEYS = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "C", "0", "⌫"];

  function press(key: string) {
    if (key === "C") {
      value = "";
      return;
    }
    if (key === "⌫") {
      value = value.slice(0, -1);
      return;
    }
    if (value.length >= 4) return;
    value = value + key;
    // Small delay so the fourth box visibly paints before the wizard moves on.
    if (value.length === 4) setTimeout(onSubmit, 200);
  }
</script>

<p class="hint centered">Enter the PIN showing on your Apple TV</p>

<div class="pin-boxes">
  {#each [0, 1, 2, 3] as i (i)}
    <div class="pin-box" class:active={value.length === i}>{value[i] ?? ""}</div>
  {/each}
</div>

<div class="keypad">
  {#each KEYS as key (key)}
    <button type="button" class="key" class:ghost={key === "C" || key === "⌫"} onclick={() => press(key)}>{key}</button>
  {/each}
</div>
