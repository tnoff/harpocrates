<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { homeDir } from "@tauri-apps/api/path";
  import FileTable from "$lib/components/FileTable.svelte";
  import ConfirmModal from "$lib/components/ConfirmModal.svelte";
  import { selectionStore } from "$lib/stores/selection.svelte";
  import { profileStore } from "$lib/stores/profile.svelte";
  import { toast } from "$lib/stores/toast.svelte";

  interface FileEntry {
    id: number;
    object_uuid: string;
    filename: string;
    local_path: string;
    file_size: number;
    original_md5: string;
    created_at: string;
  }

  let files = $state<FileEntry[]>([]);
  let search = $state("");
  let loading = $state(false);
  let showDeleteConfirm = $state(false);
  let showBackupDir = $state(false);
  let showRestore = $state(false);
  let showVerify = $state(false);

  async function loadFiles() {
    loading = true;
    try {
      files = await invoke<FileEntry[]>("list_files", { search: search || null });
    } catch (e) {
      toast.error(String(e));
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    loadFiles();
  });

  let searchTimeout: ReturnType<typeof setTimeout>;
  function handleSearch(value: string) {
    search = value;
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(loadFiles, 300);
  }

  async function backupFile() {
    const path = await open({ multiple: false, defaultPath: await homeDir() });
    if (!path) return;
    try {
      const opId = await invoke<string>("backup_file", { filePath: path });
      let unlistenComplete: () => void;
      let unlistenFailed: () => void;
      unlistenComplete = await listen<{ id: string; message: string }>("op:complete", (event) => {
        if (event.payload.id === opId) {
          unlistenComplete();
          unlistenFailed();
          loadFiles();
        }
      });
      unlistenFailed = await listen<{ id: string; error: string }>("op:failed", (event) => {
        if (event.payload.id === opId) {
          unlistenComplete();
          unlistenFailed();
          toast.error(event.payload.error);
        }
      });
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function deleteSelected() {
    showDeleteConfirm = false;
    try {
      const count = await invoke<number>("delete_backup_entries", { backupEntryIds: selectionStore.array });
      toast.success(`Deleted ${count} entries`);
      selectionStore.clear();
      await loadFiles();
    } catch (e) {
      toast.error(String(e));
    }
  }
</script>

<div class="page">
  {#if !profileStore.isReadOnly}
    <div class="table-toolbar">
      <button onclick={backupFile} class="action-btn action-btn-primary">Backup File</button>
      <button onclick={() => showBackupDir = true} class="action-btn action-btn-primary">Backup Directory</button>
    </div>
  {/if}

  <div class="page-header">
    <h2>Files</h2>
    {#if selectionStore.count > 0}
      <span class="selection-count">{selectionStore.count} selected</span>
    {/if}
  </div>

  <div class="actions-bar">
    <input
      type="text"
      placeholder="Search files..."
      value={search}
      oninput={(e) => handleSearch(e.currentTarget.value)}
      class="search-input"
    />
    {#if selectionStore.count > 0}
      <button onclick={() => showRestore = true} class="action-btn action-btn-secondary">Restore</button>
      <button onclick={() => showVerify = true} class="action-btn action-btn-secondary">Verify</button>
      {#if !profileStore.isReadOnly}
        <button onclick={() => showDeleteConfirm = true} class="action-btn action-btn-danger">Delete</button>
      {/if}
    {/if}
  </div>

  {#if loading}
    <p class="loading-text">Loading files...</p>
  {:else}
    <FileTable {files} />
  {/if}

  {#if showDeleteConfirm}
    <ConfirmModal
      title="Delete Backups"
      message="Delete {selectionStore.count} selected backup entries? The S3 objects will also be removed."
      confirmLabel="Delete"
      danger={true}
      onconfirm={deleteSelected}
      oncancel={() => showDeleteConfirm = false}
    />
  {/if}

  {#if showBackupDir}
    {#await import("$lib/components/BackupDirectoryModal.svelte") then mod}
      <mod.default onclose={() => { showBackupDir = false; }} />
    {/await}
  {/if}

  {#if showRestore}
    {#await import("$lib/components/RestoreModal.svelte") then mod}
      <mod.default
        selectedIds={selectionStore.array}
        onclose={() => { showRestore = false; }}
      />
    {/await}
  {/if}

  {#if showVerify}
    {#await import("$lib/components/VerifyIntegrityModal.svelte") then mod}
      <mod.default selectedIds={selectionStore.array} onclose={() => { showVerify = false; }} />
    {/await}
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .page-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .page-header h2 {
    font-size: 1.25rem;
    font-weight: 700;
    margin: 0;
  }

  .selection-count {
    font-size: 0.875rem;
    color: #64748b;
  }

  .actions-bar {
    display: flex;
    gap: 0.75rem;
    flex-wrap: wrap;
    align-items: center;
  }

  .table-toolbar {
    display: flex;
    gap: 0.5rem;
  }

  .search-input {
    flex: 1;
    min-width: 12rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid #cbd5e1;
    background: white;
    font-size: 0.875rem;
    outline: none;
    transition: border-color 0.15s, box-shadow 0.15s;
  }

  .search-input:focus {
    border-color: #3b82f6;
    box-shadow: 0 0 0 2px rgb(59 130 246 / 0.2);
  }

  .action-btn {
    padding: 0.5rem 0.75rem;
    font-size: 0.875rem;
    border-radius: 0.5rem;
    border: none;
    cursor: pointer;
    white-space: nowrap;
    transition: background-color 0.15s;
  }

  .action-btn-primary { background: #3b82f6; color: white; }
  .action-btn-primary:hover { background: #2563eb; }

  .action-btn-secondary { background: #e2e8f0; color: #334155; }
  .action-btn-secondary:hover { background: #cbd5e1; }

  .action-btn-danger { background: #fee2e2; color: #ef4444; }
  .action-btn-danger:hover { background: #fecaca; }

  .loading-text {
    color: #64748b;
    font-size: 0.875rem;
  }

  @media (prefers-color-scheme: dark) {
    .selection-count { color: #94a3b8; }
    .search-input { background: #1e293b; border-color: #475569; color: #f1f5f9; }
    .search-input::placeholder { color: #64748b; }
    .action-btn-secondary { background: #334155; color: #cbd5e1; }
    .action-btn-secondary:hover { background: #475569; }
    .action-btn-danger { background: rgb(127 29 29 / 0.3); color: #f87171; }
    .action-btn-danger:hover { background: rgb(127 29 29 / 0.5); }
    .loading-text { color: #94a3b8; }
  }
</style>
