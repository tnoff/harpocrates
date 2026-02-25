<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  let dirPath = $state("");
  let skipPatterns = $state<string[]>([]);
  let newPattern = $state("");
  let forceChecksum = $state(false);
  let error = $state("");

  async function pickDirectory() {
    const path = await open({ directory: true });
    if (path) dirPath = path;
  }

  function addPattern() {
    const p = newPattern.trim();
    if (p && !skipPatterns.includes(p)) {
      skipPatterns = [...skipPatterns, p];
      newPattern = "";
    }
  }

  function removePattern(index: number) {
    skipPatterns = skipPatterns.filter((_, i) => i !== index);
  }

  async function startBackup() {
    if (!dirPath) return;
    error = "";
    try {
      await invoke("backup_directory", { dirPath, skipPatterns, forceChecksum });
      onclose();
    } catch (e) {
      error = String(e);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="overlay" onclick={onclose} onkeydown={(e) => e.key === "Escape" && onclose()} role="presentation">
  <div
    role="dialog"
    aria-modal="true"
    aria-labelledby="backup-dir-title"
    tabindex="-1"
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
  >
    <h3 id="backup-dir-title" class="dialog-title">Backup Directory</h3>

    <div class="section">
      <div class="field">
        <label class="field-label" for="dir-path">Directory</label>
        <div class="field-row">
          <input id="dir-path" value={dirPath} readonly class="text-input flex-1" placeholder="Select a directory..." />
          <button onclick={pickDirectory} class="btn-secondary-sm">Browse</button>
        </div>
      </div>

      <div class="field">
        <label class="field-label" for="skip-pattern">Skip Patterns</label>
        <div class="field-row field-row-mb">
          <input
            id="skip-pattern"
            bind:value={newPattern}
            class="text-input flex-1"
            placeholder="e.g. *.log"
            onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), addPattern())}
          />
          <button onclick={addPattern} class="btn-secondary-sm">Add</button>
        </div>
        {#if skipPatterns.length > 0}
          <div class="tag-list">
            {#each skipPatterns as pattern, i}
              <span class="tag">
                {pattern}
                <button onclick={() => removePattern(i)} class="tag-remove">&times;</button>
              </span>
            {/each}
          </div>
        {/if}
      </div>

      <label class="checkbox-label">
        <input type="checkbox" bind:checked={forceChecksum} />
        Force checksum (re-upload even if size/mtime match)
      </label>

      {#if error}
        <p class="error-text">{error}</p>
      {/if}

      <div class="btn-row">
        <button onclick={startBackup} disabled={!dirPath} class="btn-primary flex-1">Start Backup</button>
        <button onclick={onclose} class="btn-secondary-sm">Cancel</button>
      </div>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }

  .dialog {
    background: white;
    border-radius: 0.75rem;
    box-shadow: 0 20px 25px -5px rgb(0 0 0 / 0.15);
    padding: 1.5rem;
    max-width: 28rem;
    width: calc(100% - 2rem);
    max-height: 90vh;
    overflow-y: auto;
  }

  .dialog-title { font-size: 1.125rem; font-weight: 600; margin: 0 0 1rem; }
  .section { display: flex; flex-direction: column; gap: 1rem; }
  .field { display: flex; flex-direction: column; gap: 0.375rem; }
  .field-label { font-size: 0.875rem; font-weight: 500; }
  .field-row { display: flex; gap: 0.5rem; }
  .field-row-mb { margin-bottom: 0.5rem; }
  .flex-1 { flex: 1; }

  .text-input {
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid #cbd5e1;
    background: white;
    font-size: 0.875rem;
    outline: none;
    transition: border-color 0.15s;
  }
  .text-input:focus { border-color: #3b82f6; box-shadow: 0 0 0 2px rgb(59 130 246 / 0.2); }

  .tag-list { display: flex; flex-wrap: wrap; gap: 0.25rem; }
  .tag {
    display: inline-flex; align-items: center; gap: 0.25rem;
    padding: 0.25rem 0.5rem; background: #f1f5f9;
    border-radius: 0.25rem; font-size: 0.75rem;
  }
  .tag-remove { background: none; border: none; cursor: pointer; color: #94a3b8; padding: 0; line-height: 1; font-size: 1rem; }
  .tag-remove:hover { color: #ef4444; }

  .checkbox-label { display: flex; align-items: center; gap: 0.5rem; font-size: 0.875rem; cursor: pointer; }
  .btn-row { display: flex; gap: 0.75rem; }

  .error-text { font-size: 0.8125rem; color: #ef4444; margin: 0; }

  .btn-primary {
    padding: 0.5rem 1rem; background: #3b82f6; color: white;
    border-radius: 0.5rem; border: none; font-size: 0.875rem;
    font-weight: 500; cursor: pointer; transition: background-color 0.15s;
  }
  .btn-primary:hover { background: #2563eb; }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

  .btn-secondary-sm {
    padding: 0.5rem 0.75rem; background: #f1f5f9; color: #334155;
    border-radius: 0.5rem; border: none; font-size: 0.875rem;
    cursor: pointer; white-space: nowrap; transition: background-color 0.15s;
  }
  .btn-secondary-sm:hover { background: #e2e8f0; }

  @media (prefers-color-scheme: dark) {
    .dialog { background: #1e293b; color: #f1f5f9; }
    .text-input { background: #0f172a; border-color: #475569; color: #f1f5f9; }
    .tag { background: #334155; color: #cbd5e1; }
    .btn-secondary-sm { background: #334155; color: #cbd5e1; }
    .btn-secondary-sm:hover { background: #475569; }
  }
</style>
