<script lang="ts">
  import { runAction, setEnabled } from "./api";
  import HotkeyBox from "./HotkeyBox.svelte";
  import Icon from "./Icon.svelte";
  import PromptCard from "./PromptCard.svelte";
  import SettingsForm from "./SettingsForm.svelte";
  import Toggle from "./Toggle.svelte";
  import type { FeatureState } from "./types";

  let { feature }: { feature: FeatureState } = $props();

  let running = $state<string | null>(null);
  let errors = $state<Record<string, string>>({});
  let toggleError = $state("");

  async function run(actionId: string) {
    running = actionId;
    delete errors[actionId];
    try {
      await runAction(feature.id, actionId);
    } catch (e) {
      errors[actionId] = String(e);
    } finally {
      running = null;
    }
  }

  async function toggle(next: boolean) {
    toggleError = "";
    try {
      await setEnabled(feature.id, next);
    } catch (e) {
      toggleError = String(e);
    }
  }
</script>

<div class="page">
  <header>
    <span class="hicon"><Icon name={feature.icon} size={22} /></span>
    <div class="htext">
      <h1>{feature.name}</h1>
      <p>{feature.description}</p>
    </div>
    <label class="enabled">
      <span>{feature.enabled ? "On" : "Off"}</span>
      <Toggle label="Enabled" checked={feature.enabled} onchange={toggle} />
    </label>
  </header>

  {#if toggleError}
    <div class="error-msg" role="alert"><span>{toggleError}</span></div>
  {/if}

  {#if feature.status}
    <div class="status card">
      <span class="dot {feature.enabled ? feature.status.tone : 'off'}"></span>
      <span>{feature.enabled ? feature.status.text : "Switched off"}</span>
    </div>
  {/if}

  {#if feature.prompt}
    <PromptCard prompt={feature.prompt} />
  {/if}

  <div class="body" class:disabled={!feature.enabled}>
    {#if feature.actions.length}
      <section>
        <h2>Actions</h2>
        <div class="card list">
          {#each feature.actions as a (a.id)}
            <div class="arow">
              <div class="aline">
                <span class="aicon" class:on={a.active}><Icon name={a.icon} size={16} /></span>
                <div class="alabel">
                  <span>{a.label}</span>
                  {#if a.active}<span class="pill">Current</span>{/if}
                </div>
                {#if a.hotkeyable}
                  <HotkeyBox featureId={feature.id} action={a} />
                {/if}
                <button
                  type="button"
                  class="btn small"
                  disabled={feature.busy || running !== null || !feature.enabled}
                  onclick={() => run(a.id)}
                >
                  {#if running === a.id}
                    <Icon name="loader" size={14} />
                  {:else}
                    <Icon name="play" size={12} />
                  {/if}
                  Run
                </button>
              </div>
              {#if errors[a.id]}
                <div class="error-msg" role="alert">
                  <span>{errors[a.id]}</span>
                  <button type="button" class="icon-btn" aria-label="Dismiss" onclick={() => delete errors[a.id]}>
                    <Icon name="x" size={14} />
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <SettingsForm featureId={feature.id} settings={feature.settings} />
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
  header {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .hicon {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: 11px;
    background: var(--accent-subtle);
    color: var(--accent-text);
  }
  .htext {
    flex: 1;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
    line-height: 1.2;
  }
  .htext p {
    margin: 2px 0 0;
    color: var(--text-2);
  }
  .enabled {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-2);
  }
  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 18px;
    transition: opacity 200ms var(--ease);
  }
  .body.disabled {
    opacity: 0.55;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 16px;
    font-weight: 600;
  }
  .arow {
    padding: 8px 14px;
  }
  .arow + .arow {
    border-top: 1px solid var(--border);
  }
  .aline {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 36px;
  }
  .aicon {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 7px;
    color: var(--text-2);
    background: var(--sidebar-hover);
    transition:
      background 200ms var(--ease),
      color 200ms var(--ease);
  }
  .aicon.on {
    background: var(--accent);
    color: var(--on-accent);
  }
  .alabel {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    font-weight: 500;
  }
  .pill {
    padding: 1px 8px;
    border-radius: 10px;
    font-size: 11.5px;
    font-weight: 500;
    background: var(--accent-subtle);
    color: var(--accent-text);
  }
  .arow .error-msg {
    margin: 6px 0 2px;
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
