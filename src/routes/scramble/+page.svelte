<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { selectionStore } from "$lib/stores/selection.svelte";
  import ConfirmModal from "$lib/components/ConfirmModal.svelte";
  import { operationsStore } from "$lib/stores/operations.svelte";

  interface ScrambleSummary {
    files_scrambled: number;
    manifests_invalidated: number;
    failed: number;
    failures: string[];
  }

  interface ScrambleProgress {
    processed: number;
    total: number;
    current_file: string;
    scrambled: number;
    failed: number;
  }

  let scrambleAll = $state(false);
  let showConfirm = $state(false);
  let running = $state(false);

  async function doScramble() {
    showConfirm = false;
    running = true;

    const label = scrambleAll
      ? "Scrambling all files"
      : `Scrambling ${selectionStore.count} file(s)`;
    const id = operationsStore.add(label);

    const unlisten = await listen<ScrambleProgress>("scramble:progress", (event) => {
      const p = event.payload;
      operationsStore.updateProgress(id, {
        current: p.processed,
        total: p.total,
        detail: p.current_file.split("/").at(-1),
      });
    });

    try {
      const summary = await invoke<ScrambleSummary>("scramble", {
        backupEntryIds: scrambleAll ? [] : selectionStore.array,
        scrambleAll,
      });
      const parts = [
        `${summary.files_scrambled} scrambled`,
        summary.manifests_invalidated > 0
          ? `${summary.manifests_invalidated} manifests invalidated`
          : null,
        summary.failed > 0 ? `${summary.failed} failed` : null,
      ].filter(Boolean);
      operationsStore.complete(id, parts.join(", "));
    } catch (e) {
      operationsStore.fail(id, String(e));
    } finally {
      unlisten();
      running = false;
    }
  }

  const hasSelection = $derived(selectionStore.count > 0);
</script>

<div class="page">
  <h2 class="page-title">Scramble (Re-encrypt)</h2>
  <p class="description">
    Re-encrypts files with new random keys, changing their S3 object paths. This invalidates any
    active share manifests that reference the scrambled files.
  </p>

  <div class="radio-group">
    <label class="radio-label">
      <input type="radio" name="scope" checked={!scrambleAll} onchange={() => scrambleAll = false} />
      Selected files only ({selectionStore.count} selected)
    </label>
    <label class="radio-label">
      <input type="radio" name="scope" checked={scrambleAll} onchange={() => scrambleAll = true} />
      All files
    </label>
  </div>

  <button
    onclick={() => showConfirm = true}
    disabled={running || (!scrambleAll && !hasSelection)}
    class="btn-warning"
  >
    {running ? "Scrambling..." : "Scramble"}
  </button>

  {#if showConfirm}
    <ConfirmModal
      title="Confirm Scramble"
      message="This will re-encrypt {scrambleAll ? 'ALL' : selectionStore.count} file(s) and invalidate any share manifests referencing them. This cannot be undone."
      confirmLabel="Scramble"
      danger={true}
      onconfirm={doScramble}
      oncancel={() => showConfirm = false}
    />
  {/if}
</div>

<style>
  .page { display: flex; flex-direction: column; gap: 1rem; max-width: 32rem; }
  .page-title { font-size: 1.25rem; font-weight: 700; margin: 0; }
  .description { font-size: 0.875rem; color: #475569; margin: 0; }
  .radio-group { display: flex; flex-direction: column; gap: 0.5rem; }
  .radio-label { display: flex; align-items: center; gap: 0.5rem; font-size: 0.875rem; cursor: pointer; }

  .btn-warning {
    align-self: flex-start; padding: 0.5rem 1rem; background: #f59e0b; color: white;
    border-radius: 0.5rem; border: none; font-size: 0.875rem; font-weight: 500;
    cursor: pointer; transition: opacity 0.15s;
  }
  .btn-warning:hover { opacity: 0.9; }
  .btn-warning:disabled { opacity: 0.5; cursor: not-allowed; }

  @media (prefers-color-scheme: dark) {
    .description { color: #94a3b8; }
  }
</style>
