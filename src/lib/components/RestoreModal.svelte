<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";

  interface Props {
    selectedIds: number[];
    onclose: () => void;
  }

  let { selectedIds, onclose }: Props = $props();

  let useCustomDir = $state(false);
  let targetDir = $state("");
  let error = $state("");

  async function pickDirectory() {
    const path = await open({ directory: true });
    if (path) targetDir = path;
  }

  async function restore() {
    error = "";
    try {
      await invoke("restore_files", {
        backupEntryIds: selectedIds,
        targetDirectory: useCustomDir && targetDir ? targetDir : null,
      });
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
    aria-labelledby="restore-title"
    tabindex="-1"
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
  >
    <h3 id="restore-title" class="dialog-title">Restore Files</h3>

    <div class="section">
      <p class="muted-text">Restoring {selectedIds.length} file(s).</p>

      <div class="radio-group">
        <label class="radio-label">
          <input type="radio" name="restore-target" checked={!useCustomDir} onchange={() => useCustomDir = false} />
          Restore to original paths
        </label>
        <label class="radio-label">
          <input type="radio" name="restore-target" checked={useCustomDir} onchange={() => useCustomDir = true} />
          Restore to custom directory
        </label>
      </div>

      {#if useCustomDir}
        <div class="field-row">
          <input value={targetDir} readonly class="text-input flex-1" placeholder="Select directory..." />
          <button onclick={pickDirectory} class="btn-secondary-sm">Browse</button>
        </div>
      {/if}

      {#if error}
        <p class="error-text">{error}</p>
      {/if}

      <div class="btn-row">
        <button onclick={restore} disabled={useCustomDir && !targetDir} class="btn-primary flex-1">
          Restore
        </button>
        <button onclick={onclose} class="btn-secondary-sm">Cancel</button>
      </div>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed; inset: 0; background: rgba(0, 0, 0, 0.5);
    display: flex; align-items: center; justify-content: center; z-index: 50;
  }

  .dialog {
    background: white; border-radius: 0.75rem;
    box-shadow: 0 20px 25px -5px rgb(0 0 0 / 0.15);
    padding: 1.5rem; max-width: 28rem; width: calc(100% - 2rem);
  }

  .dialog-title { font-size: 1.125rem; font-weight: 600; margin: 0 0 1rem; }
  .section { display: flex; flex-direction: column; gap: 1rem; }
  .muted-text { font-size: 0.875rem; color: #64748b; margin: 0; }
  .radio-group { display: flex; flex-direction: column; gap: 0.5rem; }
  .radio-label { display: flex; align-items: center; gap: 0.5rem; font-size: 0.875rem; cursor: pointer; }
  .field-row { display: flex; gap: 0.5rem; }
  .flex-1 { flex: 1; }

  .text-input {
    padding: 0.5rem 0.75rem; border-radius: 0.5rem; border: 1px solid #cbd5e1;
    background: white; font-size: 0.875rem; outline: none;
  }

  .error-text { font-size: 0.8125rem; color: #ef4444; margin: 0; }
  .btn-row { display: flex; gap: 0.75rem; }

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
    .muted-text { color: #94a3b8; }
    .text-input { background: #0f172a; border-color: #475569; color: #f1f5f9; }
    .btn-secondary-sm { background: #334155; color: #cbd5e1; }
    .btn-secondary-sm:hover { background: #475569; }
  }
</style>
