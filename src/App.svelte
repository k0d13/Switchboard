<script lang="ts">
  import Flyout from "./Flyout.svelte";
  import Settings from "./Settings.svelte";
  import type { WindowKind } from "./lib/api";

  let { kind }: { kind: WindowKind } = $props();
</script>

{#if kind === "flyout"}
  <Flyout />
{:else}
  <Settings />
{/if}
