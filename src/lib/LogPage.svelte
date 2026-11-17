<script lang="ts">
  import { tick } from "svelte";
  import { app } from "./store.svelte";

  let hidden = $state<string[]>([]);
  let scroller: HTMLDivElement;
  let atBottom = true;

  const sources = $derived([...new Set(app.logs.map((l) => l.source))].sort());
  const lines = $derived(app.logs.filter((l) => !hidden.includes(l.source)));

  const fmt = (t: number) => new Date(t).toLocaleTimeString("en-GB", { hour12: false });

  function onscroll() {
    atBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
  }

  function toggle(s: string) {
    hidden = hidden.includes(s) ? hidden.filter((x) => x !== s) : [...hidden, s];
  }

  // Stick to the bottom as lines arrive, unless the user has scrolled up to read.
  $effect(() => {
    lines.length;
    tick().then(() => {
      if (atBottom && scroller) scroller.scrollTop = scroller.scrollHeight;
    });
  });
</script>

<div class="page">
  <h1>Log</h1>

  <div class="chips" role="group" aria-label="Filter by source">
    {#each sources as s (s)}
      <button type="button" class="chip" class:off={hidden.includes(s)} aria-pressed={!hidden.includes(s)} onclick={() => toggle(s)}>
        {s}
      </button>
    {/each}
  </div>

  <div class="card log" bind:this={scroller} {onscroll} role="log" aria-label="Log lines">
    {#each lines as l, i (i + ":" + l.time)}
      <div class="line {l.level}">
        <span class="time">{fmt(l.time)}</span>
        <span class="src">{l.source}</span>
        <span class="msg">{l.message}</span>
      </div>
    {:else}
      <div class="none">No log lines to show.</div>
    {/each}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 12px;
    height: 100%;
    animation: in 220ms var(--ease);
  }
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    height: 26px;
    padding: 0 12px;
    border-radius: 13px;
    border: 1px solid color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-subtle);
    color: var(--accent-text);
    font-size: 12.5px;
    transition:
      background 140ms var(--ease),
      color 140ms var(--ease),
      border-color 140ms var(--ease);
  }
  .chip:hover {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .chip.off {
    border-color: var(--border);
    background: transparent;
    color: var(--text-3);
    text-decoration: line-through;
  }
  .chip.off:hover {
    background: var(--sidebar-hover);
  }
  .log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px 4px;
    font-family: var(--mono);
    font-size: 12.5px;
    line-height: 1.5;
    user-select: text;
  }
  .line {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 2px 10px;
    border-radius: 4px;
  }
  .line:hover {
    background: var(--sidebar-hover);
  }
  .time {
    flex: none;
    color: var(--text-3);
  }
  .src {
    flex: none;
    min-width: 98px;
    padding: 0 7px;
    border-radius: 9px;
    background: var(--sidebar-active);
    color: var(--text-2);
    font-family: var(--font);
    font-size: 11.5px;
    text-align: center;
  }
  .msg {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .line.warn .msg {
    color: var(--tone-warn);
  }
  .line.error .msg {
    color: var(--tone-error);
  }
  .none {
    padding: 20px;
    text-align: center;
    color: var(--text-3);
    font-family: var(--font);
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
