<script lang="ts">
  import { onMount } from "svelte";
  import { closeWindow, minimiseWindow, onNavigate } from "./lib/api";
  import FeaturePage from "./lib/FeaturePage.svelte";
  import GeneralPage from "./lib/GeneralPage.svelte";
  import Icon from "./lib/Icon.svelte";
  import LogPage from "./lib/LogPage.svelte";
  import { app, startStore } from "./lib/store.svelte";

  let page = $state("general");

  const feature = $derived(app.state?.features.find((f) => f.id === page) ?? null);

  onMount(() => {
    startStore(true);
    return onNavigate((id) => (page = id));
  });
</script>

<div class="shell">
  <div class="titlebar" data-tauri-drag-region>
    <span class="app-icon" data-tauri-drag-region><Icon name="sliders" size={14} /></span>
    <span class="app-name" data-tauri-drag-region>Switchboard</span>
    <div class="wbtns">
      <button type="button" class="wbtn" aria-label="Minimise" onclick={() => minimiseWindow()}>
        <Icon name="minus" size={15} />
      </button>
      <button type="button" class="wbtn close" aria-label="Close" onclick={() => closeWindow()}>
        <Icon name="x" size={15} />
      </button>
    </div>
  </div>

  <div class="main">
    <nav aria-label="Pages">
      <button type="button" class="item" class:sel={page === "general"} onclick={() => (page = "general")}>
        <Icon name="settings" size={16} />
        <span>General</span>
      </button>

      {#if app.state?.features.length}
        <div class="group-label">Features</div>
      {/if}
      {#each app.state?.features ?? [] as f (f.id)}
        <button type="button" class="item" class:sel={page === f.id} onclick={() => (page = f.id)}>
          <Icon name={f.icon} size={16} />
          <span>{f.name}</span>
          <span
            class="dot {f.enabled ? (f.status?.tone ?? 'ok') : 'off'}"
            title={f.enabled ? (f.status?.text ?? "On") : "Off"}
          ></span>
        </button>
      {/each}

      <div class="spacer"></div>
      <button type="button" class="item" class:sel={page === "log"} onclick={() => (page = "log")}>
        <Icon name="scroll-text" size={16} />
        <span>Log</span>
      </button>
    </nav>

    <section class="content" class:fill={page === "log"}>
      {#if !app.state}
        <div class="loading"><Icon name="loader" size={20} /></div>
      {:else if page === "general"}
        <GeneralPage app={app.state} />
      {:else if page === "log"}
        <LogPage />
      {:else if feature}
        {#key feature.id}
          <FeaturePage {feature} />
        {/key}
      {:else}
        <p class="missing">That feature isn't available.</p>
      {/if}
    </section>
  </div>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .titlebar {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    flex: none;
    padding-left: 12px;
  }
  .app-icon {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    background: var(--accent);
    color: var(--on-accent);
  }
  .app-name {
    font-size: 12px;
    color: var(--text-2);
  }
  .wbtns {
    display: flex;
    margin-left: auto;
    height: 100%;
  }
  .wbtn {
    display: grid;
    place-items: center;
    width: 46px;
    height: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    transition: background 120ms var(--ease);
  }
  .wbtn:hover {
    background: var(--sidebar-active);
  }
  .wbtn:active {
    background: var(--sidebar-hover);
  }
  .wbtn.close:hover {
    background: #c42b1c;
    color: #fff;
  }
  .wbtn:focus-visible {
    outline-offset: -3px;
  }
  .main {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 224px;
    flex: none;
    padding: 4px 10px 12px;
    overflow-y: auto;
  }
  .group-label {
    margin: 12px 10px 4px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .spacer {
    flex: 1;
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px 0 12px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    transition: background 120ms var(--ease);
  }
  .item span:nth-child(2) {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item:hover {
    background: var(--sidebar-hover);
  }
  .item:active {
    background: var(--sidebar-active);
  }
  .item.sel {
    background: var(--sidebar-active);
  }
  .item::before {
    content: "";
    position: absolute;
    left: 0;
    top: 50%;
    width: 3px;
    height: 16px;
    margin-top: -8px;
    border-radius: 2px;
    background: var(--accent);
    transform: scaleY(0);
    transition: transform 180ms var(--ease);
  }
  .item.sel::before {
    transform: scaleY(1);
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 22px 28px 28px;
    background: var(--layer);
    border-top: 1px solid var(--border);
    border-left: 1px solid var(--border);
    border-top-left-radius: 8px;
  }
  .content.fill {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .content.fill > :global(.page) {
    flex: 1;
    min-height: 0;
  }
  .loading,
  .missing {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--text-2);
  }
</style>
