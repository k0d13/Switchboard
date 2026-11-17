<script lang="ts">
  import { openUrl } from "./api";
  import Icon from "./Icon.svelte";
  import type { Prompt } from "./types";

  let { prompt }: { prompt: Prompt } = $props();

  let copied = $state(false);
  let failed = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    if (!prompt.code) return;
    try {
      await navigator.clipboard.writeText(prompt.code);
      copied = true;
      failed = false;
    } catch {
      failed = true;
      copied = false;
    }
    clearTimeout(timer);
    timer = setTimeout(() => {
      copied = false;
      failed = false;
    }, 1800);
  }
</script>

<div class="prompt" role="group" aria-label={prompt.title}>
  <div class="head">
    <Icon name="info" size={16} />
    <strong>{prompt.title}</strong>
  </div>
  <p>{prompt.message}</p>
  {#if prompt.code}
    <div class="code" aria-label="Code">{prompt.code}</div>
  {/if}
  <div class="buttons">
    {#if prompt.code}
      <button type="button" class="btn small" onclick={copy}>
        <Icon name={copied ? "check" : "copy"} size={14} />
        {copied ? "Copied" : failed ? "Couldn't copy" : "Copy"}
      </button>
    {/if}
    {#if prompt.url}
      <button type="button" class="btn small accent" onclick={() => openUrl(prompt.url!)}>
        <Icon name="external-link" size={14} />
        Open
      </button>
    {/if}
  </div>
</div>

<style>
  .prompt {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-radius: 8px;
    background: var(--accent-subtle);
    border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
    animation: in 200ms var(--ease);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--accent-text);
  }
  p {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
  }
  .code {
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--field);
    border: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 26px;
    font-weight: 600;
    letter-spacing: 0.22em;
    text-align: center;
    user-select: text;
  }
  .buttons {
    display: flex;
    gap: 8px;
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>
