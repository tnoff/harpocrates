<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { selectionStore } from "$lib/stores/selection.svelte";
  import ConfirmModal from "$lib/components/ConfirmModal.svelte";

  let scrambleAll = $state(false);
  let showConfirm = $state(false);
  let submitting = $state(false);

  async function doScramble() {
    showConfirm = false;
    submitting = true;
    try {
      await invoke("scramble", {
        backupEntryIds: scrambleAll ? [] : selectionStore.array,
        scrambleAll,
      });
    } catch {
      // enqueue failure is rare; result will appear in StatusFooter
    } finally {
      submitting = false;
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
    disabled={submitting || (!scrambleAll && !hasSelection)}
    class="btn-warning"
  >
    Scramble
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
