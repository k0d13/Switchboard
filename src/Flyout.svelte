<script lang="ts">
  import { onMount } from "svelte";
  import { closeWindow, flyoutResize, openSettings, runAction } from "./lib/api";
  import Icon from "./lib/Icon.svelte";
  import PromptCard from "./lib/PromptCard.svelte";
  import { pretty } from "./lib/hotkey";
  import { app, startStore } from "./lib/store.svelte";

  let root: HTMLElement;
  let running = $state<Record<string, string>>({});
  let errors = $state<Record<string, string>>({});

  const features = $derived((app.state?.features ?? []).filter((f) => f.enabled));

  onMount(() => {
    startStore(false);
    let last = 0;
    const ro = new ResizeObserver(() => {
      const h = Math.ceil(root.getBoundingClientRect().height);
      if (h > 0 && h !== last) {
        last = h;
        flyoutResize(h);
      }
    });
    ro.observe(root);
    return () => ro.disconnect();
  });

  async function run(featureId: string, actionId: string) {
    running[featureId] = actionId;
    delete errors[featureId];
    try {
      await runAction(featureId, actionId);
    } catch (e) {
      errors[featureId] = String(e);
    } finally {
      delete running[featureId];
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") closeWindow();
  }
</script>

<svelte:window {onkeydown} />

<main bind:this={root}>
  <header data-tauri-drag-region>
    <span class="title" data-tauri-drag-region>Switchboard</span>
    <button type="button" class="icon-btn" title="Settings" aria-label="Open settings" onclick={() => openSettings()}>
      <Icon name="settings" size={17} />
    </button>
  </header>

  {#each features as f (f.id)}
    {@const locked = f.busy || !!running[f.id]}
    {@const tiles = f.actions.filter((a) => a.flyout)}
    <section class="card feature">
      <div class="top">
        <span class="ficon"><Icon name={f.icon} size={18} /></span>
        <div class="meta">
          <div class="name">{f.name}</div>
          {#if f.status}
            <div class="status">
              <span class="dot {f.status.tone}"></span>
              <span class="stext">{f.status.text}</span>
            </div>
          {/if}
        </div>
      </div>

      {#if tiles.length}
        <div class="tiles">
          {#each tiles as a (a.id)}
            {@const isRunning = running[f.id] === a.id}
            <button
              type="button"
              class="tile"
              class:active={a.active}
              disabled={locked}
              aria-busy={isRunning}
              onclick={() => run(f.id, a.id)}
            >
              <span class="tile-icon">
                <Icon name={isRunning ? "loader" : a.icon} size={20} />
              </span>
              <span class="tile-label">{a.label}</span>
              {#if a.hotkey}
                <span class="tile-key">{pretty(a.hotkey)}</span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}

      {#if f.prompt}
        <PromptCard prompt={f.prompt} />
      {/if}

      {#if errors[f.id]}
        <div class="error-msg" role="alert">
          <span>{errors[f.id]}</span>
          <button type="button" class="icon-btn" aria-label="Dismiss" onclick={() => delete errors[f.id]}>
            <Icon name="x" size={14} />
          </button>
        </div>
      {/if}
    </section>
  {:else}
    {#if app.state}
      <p class="empty">Nothing is switched on. Turn a feature on in settings.</p>
    {/if}
  {/each}
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 6px 12px 12px;
    font-size: 13px;
  }
  :global(html.mock.win-flyout) main {
    max-width: 360px;
    margin: 24px auto;
    border-radius: 10px;
    background: color-mix(in srgb, Canvas 55%, transparent);
    border: 1px solid var(--border);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.18);
    backdrop-filter: blur(24px);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 38px;
    padding-left: 4px;
  }
  .title {
    font-weight: 600;
    font-size: 13px;
    color: var(--text-2);
    letter-spacing: 0.01em;
  }
  .feature {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .ficon {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    background: var(--accent-subtle);
    color: var(--accent-text);
  }
  .meta {
    min-width: 0;
  }
  .name {
    font-weight: 600;
    font-size: 14px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    font-size: 12.5px;
  }
  .stext {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tiles {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    min-height: 76px;
    padding: 10px 12px;
    text-align: left;
    border-radius: 8px;
    border: 1px solid var(--border);
    border-bottom-color: var(--border-strong);
    background: var(--control);
    transition:
      background 140ms var(--ease),
      border-color 140ms var(--ease),
      transform 140ms var(--ease);
  }
  .tile:hover:not(:disabled) {
    background: var(--control-hover);
  }
  .tile:active:not(:disabled) {
    background: var(--control-press);
    transform: scale(0.98);
  }
  .tile:disabled {
    opacity: 0.5;
  }
  .tile[aria-busy="true"] {
    opacity: 1;
  }
  .tile.active {
    background: var(--accent);
    border-color: transparent;
    color: var(--on-accent);
  }
  .tile.active:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .tile.active:active:not(:disabled) {
    background: var(--accent-press);
  }
  .tile-label {
    font-weight: 600;
    font-size: 13px;
    line-height: 1.2;
  }
  .tile-key {
    position: absolute;
    right: 10px;
    top: 10px;
    font-size: 11px;
    opacity: 0.6;
  }
  .empty {
    margin: 12px 4px;
    color: var(--text-2);
    text-align: center;
  }
</style>
