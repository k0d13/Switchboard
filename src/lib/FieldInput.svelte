<script lang="ts">
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";
  import type { Field } from "./types";

  interface Props {
    id: string;
    field: Field;
    value: unknown;
    onchange: (v: unknown) => void;
  }
  let { id, field, value, onchange }: Props = $props();

  const hexOf = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? "0x" + v.toString(16).toUpperCase() : "");
  const listOf = (v: unknown) => (Array.isArray(v) ? v.map(String) : []);

  // Drafts for kinds where what's typed isn't always a valid value yet.
  let hexDraft = $state("");
  let listDraft = $state("");
  let numberDraft = $state("");
  let invalid = $state(false);

  $effect(() => {
    // Pull in external changes without clobbering something half-typed that maps to the same value.
    if (field.kind === "hex") {
      const parsed = parseHex(hexDraft);
      if (parsed !== value) hexDraft = hexOf(value);
    } else if (field.kind === "list") {
      const cur = listOf(value);
      const mine = splitList(listDraft);
      if (cur.length !== mine.length || cur.some((x, i) => x !== mine[i])) listDraft = cur.join("\n");
    } else if (field.kind === "number") {
      if (Number(numberDraft) !== value || numberDraft.trim() === "") numberDraft = value == null ? "" : String(value);
    }
  });

  function parseHex(s: string): number | null {
    const t = s.trim();
    if (!/^(0x)?[0-9a-f]+$/i.test(t)) return null;
    return parseInt(t.replace(/^0x/i, ""), 16);
  }
  function splitList(s: string) {
    return s.split("\n").map((x) => x.trim()).filter(Boolean);
  }
  function inRange(n: number) {
    return (field.min == null || n >= field.min) && (field.max == null || n <= field.max);
  }

  function onNumber(e: Event) {
    numberDraft = (e.currentTarget as HTMLInputElement).value;
    const n = Number(numberDraft);
    invalid = numberDraft.trim() === "" || !Number.isFinite(n) || !inRange(n);
    if (!invalid) onchange(n);
  }
  function onNumberBlur() {
    if (invalid) {
      invalid = false;
      numberDraft = value == null ? "" : String(value);
    }
  }
  function onHex(e: Event) {
    hexDraft = (e.currentTarget as HTMLInputElement).value;
    const n = parseHex(hexDraft);
    invalid = n === null || !inRange(n);
    if (!invalid) onchange(n);
  }
  function onHexBlur() {
    invalid = false;
    hexDraft = hexOf(value);
  }
  function onList(e: Event) {
    listDraft = (e.currentTarget as HTMLTextAreaElement).value;
    onchange(splitList(listDraft));
  }

  // Selects carry strings in the DOM but the options may be numbers.
  const options = $derived.by(() => {
    const o = field.options ?? [];
    if (field.kind === "select" && !o.some((x) => x.value === value)) {
      return [...o, { value: value as string | number, label: `Other (${String(value)})` }];
    }
    return o;
  });
  function onSelect(e: Event) {
    const i = (e.currentTarget as HTMLSelectElement).selectedIndex;
    onchange(options[i].value);
  }
</script>

{#if field.kind === "toggle"}
  <Toggle label={field.label} checked={value === true} onchange={onchange} />
{:else if field.kind === "select"}
  <div class="select">
    <select {id} onchange={onSelect}>
      {#each options as o, i (i)}
        <option selected={o.value === value}>{o.label}</option>
      {/each}
    </select>
    <Icon name="chevron-down" size={14} />
  </div>
{:else if field.kind === "number"}
  <input
    {id}
    class="input narrow"
    class:bad={invalid}
    type="number"
    min={field.min ?? undefined}
    max={field.max ?? undefined}
    placeholder={field.placeholder ?? undefined}
    value={numberDraft}
    oninput={onNumber}
    onblur={onNumberBlur}
    aria-invalid={invalid}
  />
{:else if field.kind === "hex"}
  <input
    {id}
    class="input narrow mono"
    class:bad={invalid}
    type="text"
    spellcheck="false"
    placeholder={field.placeholder ?? "0x00"}
    value={hexDraft}
    oninput={onHex}
    onblur={onHexBlur}
    aria-invalid={invalid}
  />
{:else if field.kind === "list"}
  <textarea
    {id}
    class="input mono"
    rows="4"
    spellcheck="false"
    placeholder={field.placeholder ?? "One per line"}
    value={listDraft}
    oninput={onList}
  ></textarea>
{:else}
  <input
    {id}
    class="input"
    type="text"
    spellcheck="false"
    placeholder={field.placeholder ?? undefined}
    value={typeof value === "string" ? value : ""}
    oninput={(e) => onchange(e.currentTarget.value)}
  />
{/if}

<style>
  .input,
  select {
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    border-bottom: 1px solid var(--border-strong);
    background: var(--field);
    transition:
      border-color 120ms var(--ease),
      background 120ms var(--ease),
      box-shadow 120ms var(--ease);
  }
  .input:hover,
  select:hover {
    background: var(--control-hover);
  }
  .input:focus,
  select:focus {
    border-bottom-color: var(--accent);
    box-shadow: 0 1px 0 var(--accent);
    outline: none;
  }
  select:focus-visible {
    outline: none;
  }
  .input::placeholder {
    color: var(--text-3);
  }
  .input.narrow {
    width: 140px;
  }
  .input.bad {
    border-bottom-color: var(--tone-error);
    box-shadow: 0 1px 0 var(--tone-error);
  }
  .mono {
    font-family: var(--mono);
    font-size: 13px;
  }
  textarea.input {
    height: auto;
    padding: 7px 10px;
    resize: vertical;
    line-height: 1.5;
    min-height: 72px;
  }
  .select {
    position: relative;
    width: 260px;
    max-width: 100%;
    color: var(--text-2);
  }
  select {
    appearance: none;
    padding-right: 30px;
    color: var(--text);
    cursor: default;
  }
  select option {
    background: Canvas;
    color: CanvasText;
  }
  .select :global(svg) {
    position: absolute;
    right: 10px;
    top: 9px;
    pointer-events: none;
  }
  input[type="number"]::-webkit-inner-spin-button {
    opacity: 0.5;
  }
</style>
