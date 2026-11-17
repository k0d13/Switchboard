<script lang="ts">
  import { quit, setAutostart } from "./api";
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";
  import type { AppState } from "./types";

  let { app }: { app: AppState } = $props();
  let error = $state("");

  async function toggle(next: boolean) {
    error = "";
    try {
      await setAutostart(next);
    } catch (e) {
      error = String(e);
    }
  }
</script>

<div class="page">
  <h1>General</h1>

  {#if error}
    <div class="error-msg" role="alert"><span>{error}</span></div>
  {/if}

  <div class="card list">
    <div class="row">
      <div>
        <div class="t">Start at login</div>
        <div class="d">Open Switchboard in the tray when you sign in to Windows.</div>
      </div>
      <Toggle label="Start at login" checked={app.autostart} onchange={toggle} />
    </div>
    <div class="row">
      <div class="t">Version</div>
      <div class="v">{app.version}</div>
    </div>
  </div>

  <div>
    <button type="button" class="btn" onclick={() => quit()}>
      <Icon name="power" size={15} />
      Quit Switchboard
    </button>
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
    max-width: 660px;
    animation: in 220ms var(--ease);
  }
  h1 {
    margin: 0 0 2px;
    font-size: 22px;
    font-weight: 600;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 12px 14px;
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .t {
    font-weight: 500;
  }
  .d,
  .v {
    color: var(--text-2);
    font-size: 12.5px;
  }
  .v {
    font-size: 14px;
    user-select: text;
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
