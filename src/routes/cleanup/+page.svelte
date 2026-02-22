<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import ConfirmModal from "$lib/components/ConfirmModal.svelte";
  import { toast } from "$lib/stores/toast.svelte";

  interface OrphanedLocalEntry { local_file_id: number; backup_entry_id: number; local_path: string; }
  interface OrphanedS3Object { key: string; size: number; }
  interface CleanupSummary { deleted_count: number; details: string[]; }

  let activeTab = $state<"local" | "s3">("local");

  // Local orphans
  let localOrphans = $state<OrphanedLocalEntry[]>([]);
  let selectedLocalIds = $state<Set<number>>(new Set());
  let scanningLocal = $state(false);
  let cleaningLocal = $state(false);
  let localDryRun = $state(true);
  let deleteS3 = $state(false);
  let localSummary = $state<CleanupSummary | null>(null);
  let showLocalConfirm = $state(false);

  async function scanLocal() {
    scanningLocal = true;
    localSummary = null;
    try {
      localOrphans = await invoke<OrphanedLocalEntry[]>("scan_orphaned_local_entries");
      selectedLocalIds = new Set(localOrphans.map(o => o.local_file_id));
    } catch (e) {
      toast.error(String(e));
    } finally {
      scanningLocal = false;
    }
  }

  function toggleLocalId(id: number) {
    const next = new Set(selectedLocalIds);
    if (next.has(id)) next.delete(id); else next.add(id);
    selectedLocalIds = next;
  }

  async function cleanupLocal() {
    showLocalConfirm = false;
    cleaningLocal = true;
    try {
      localSummary = await invoke<CleanupSummary>("cleanup_orphaned_local_entries", {
        localFileIds: [...selectedLocalIds],
        deleteS3: deleteS3,
        dryRun: localDryRun,
      });
    } catch (e) {
      toast.error(String(e));
    } finally {
      cleaningLocal = false;
    }
  }

  // S3 orphans
  let s3Orphans = $state<OrphanedS3Object[]>([]);
  let selectedS3Keys = $state<Set<string>>(new Set());
  let scanningS3 = $state(false);
  let cleaningS3 = $state(false);
  let s3DryRun = $state(true);
  let s3Summary = $state<CleanupSummary | null>(null);
  let showS3Confirm = $state(false);

  async function scanS3() {
    scanningS3 = true;
    s3Summary = null;
    try {
      s3Orphans = await invoke<OrphanedS3Object[]>("scan_orphaned_s3_objects");
      selectedS3Keys = new Set(s3Orphans.map(o => o.key));
    } catch (e) {
      toast.error(String(e));
    } finally {
      scanningS3 = false;
    }
  }

  function toggleS3Key(key: string) {
    const next = new Set(selectedS3Keys);
    if (next.has(key)) next.delete(key); else next.add(key);
    selectedS3Keys = next;
  }

  async function cleanupS3() {
    showS3Confirm = false;
    cleaningS3 = true;
    try {
      s3Summary = await invoke<CleanupSummary>("cleanup_orphaned_s3_objects", {
        objectKeys: [...selectedS3Keys],
        dryRun: s3DryRun,
      });
    } catch (e) {
      toast.error(String(e));
    } finally {
      cleaningS3 = false;
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div class="page">
  <h2 class="page-title">Cleanup</h2>

  <div class="tabs">
    <button class="tab-btn" class:active={activeTab === "local"} onclick={() => activeTab = "local"}>Orphaned Local Entries</button>
    <button class="tab-btn" class:active={activeTab === "s3"} onclick={() => activeTab = "s3"}>Orphaned S3 Objects</button>
  </div>

  <!-- Local Orphans -->
  {#if activeTab === "local"}
    <div class="tab-content">
      <button onclick={scanLocal} disabled={scanningLocal} class="btn-primary">
        {scanningLocal ? "Scanning..." : "Scan for Orphaned Entries"}
      </button>

      {#if localOrphans.length > 0}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th class="col-check">
                  <input type="checkbox"
                    checked={selectedLocalIds.size === localOrphans.length}
                    onchange={() => {
                      if (selectedLocalIds.size === localOrphans.length) selectedLocalIds = new Set();
                      else selectedLocalIds = new Set(localOrphans.map(o => o.local_file_id));
                    }}
                  />
                </th>
                <th>Local Path</th>
                <th>Entry ID</th>
              </tr>
            </thead>
            <tbody>
              {#each localOrphans as orphan}
                <tr>
                  <td><input type="checkbox" checked={selectedLocalIds.has(orphan.local_file_id)} onchange={() => toggleLocalId(orphan.local_file_id)} /></td>
                  <td class="col-path" title={orphan.local_path}>{orphan.local_path}</td>
                  <td>{orphan.backup_entry_id}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <div class="options-row">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={localDryRun} /> Dry run
          </label>
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={deleteS3} /> Also delete S3 objects
          </label>
          <button
            onclick={() => showLocalConfirm = true}
            disabled={cleaningLocal || selectedLocalIds.size === 0}
            class="btn-danger"
          >
            {cleaningLocal ? "Cleaning..." : `Delete ${selectedLocalIds.size} entries`}
          </button>
        </div>

      {:else if !scanningLocal}
        <p class="muted-text">No orphaned local entries found. Click scan to check.</p>
      {/if}

      {#if localSummary}
        <div class="result-box">
          <p class="result-heading">{localDryRun ? "Dry Run Results" : "Cleanup Complete"}</p>
          <p class="result-text">Deleted: {localSummary.deleted_count}</p>
          {#if localSummary.details.length > 0}
            <ul class="details-list">
              {#each localSummary.details as d}<li>{d}</li>{/each}
            </ul>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  <!-- S3 Orphans -->
  {#if activeTab === "s3"}
    <div class="tab-content">
      <button onclick={scanS3} disabled={scanningS3} class="btn-primary">
        {scanningS3 ? "Scanning..." : "Scan for Orphaned S3 Objects"}
      </button>

      {#if s3Orphans.length > 0}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th class="col-check">
                  <input type="checkbox"
                    checked={selectedS3Keys.size === s3Orphans.length}
                    onchange={() => {
                      if (selectedS3Keys.size === s3Orphans.length) selectedS3Keys = new Set();
                      else selectedS3Keys = new Set(s3Orphans.map(o => o.key));
                    }}
                  />
                </th>
                <th>Object Key</th>
                <th>Size</th>
              </tr>
            </thead>
            <tbody>
              {#each s3Orphans as orphan}
                <tr>
                  <td><input type="checkbox" checked={selectedS3Keys.has(orphan.key)} onchange={() => toggleS3Key(orphan.key)} /></td>
                  <td class="col-path col-mono" title={orphan.key}>{orphan.key}</td>
                  <td>{formatSize(orphan.size)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <div class="options-row">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={s3DryRun} /> Dry run
          </label>
          <button
            onclick={() => showS3Confirm = true}
            disabled={cleaningS3 || selectedS3Keys.size === 0}
            class="btn-danger"
          >
            {cleaningS3 ? "Cleaning..." : `Delete ${selectedS3Keys.size} objects`}
          </button>
        </div>

      {:else if !scanningS3}
        <p class="muted-text">No orphaned S3 objects found. Click scan to check.</p>
      {/if}

      {#if s3Summary}
        <div class="result-box">
          <p class="result-heading">{s3DryRun ? "Dry Run Results" : "Cleanup Complete"}</p>
          <p class="result-text">Deleted: {s3Summary.deleted_count}</p>
          {#if s3Summary.details.length > 0}
            <ul class="details-list">
              {#each s3Summary.details as d}<li>{d}</li>{/each}
            </ul>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  {#if showLocalConfirm}
    <ConfirmModal
      title="Cleanup Orphaned Entries"
      message="{localDryRun ? 'Dry run: ' : ''}Delete {selectedLocalIds.size} orphaned local entries{deleteS3 ? ' and their S3 objects' : ''}?"
      confirmLabel={localDryRun ? "Dry Run" : "Delete"}
      danger={!localDryRun}
      onconfirm={cleanupLocal}
      oncancel={() => showLocalConfirm = false}
    />
  {/if}

  {#if showS3Confirm}
    <ConfirmModal
      title="Cleanup Orphaned S3 Objects"
      message="{s3DryRun ? 'Dry run: ' : ''}Delete {selectedS3Keys.size} orphaned S3 objects?"
      confirmLabel={s3DryRun ? "Dry Run" : "Delete"}
      danger={!s3DryRun}
      onconfirm={cleanupS3}
      oncancel={() => showS3Confirm = false}
    />
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .page-title {
    font-size: 1.25rem;
    font-weight: 700;
    margin: 0;
  }

  /* Tabs */
  .tabs {
    display: flex;
    gap: 0.25rem;
    border-bottom: 1px solid #e2e8f0;
  }

  .tab-btn {
    padding: 0.5rem 1rem;
    font-size: 0.875rem;
    font-weight: 500;
    border-radius: 0.375rem 0.375rem 0 0;
    border: 1px solid transparent;
    border-bottom: none;
    background: none;
    cursor: pointer;
    color: #64748b;
    transition: color 0.15s;
    margin-bottom: -1px;
  }

  .tab-btn:hover { color: #334155; }

  .tab-btn.active {
    background: white;
    border-color: #e2e8f0;
    border-bottom-color: white;
    color: #0f172a;
  }

  .tab-content {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  /* Table */
  .table-wrap {
    overflow: auto;
    border: 1px solid #e2e8f0;
    border-radius: 0.5rem;
  }

  table {
    width: 100%;
    font-size: 0.875rem;
    border-collapse: collapse;
  }

  thead {
    background: #f8fafc;
    text-align: left;
  }

  th {
    padding: 0.5rem 0.75rem;
    font-weight: 500;
    color: #475569;
    border-bottom: 1px solid #e2e8f0;
    white-space: nowrap;
  }

  .col-check { width: 2rem; }

  .col-path {
    max-width: 28rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .col-mono { font-family: monospace; font-size: 0.75rem; }

  tbody tr { border-bottom: 1px solid #f1f5f9; transition: background-color 0.1s; }
  tbody tr:last-child { border-bottom: none; }
  tbody tr:hover { background: #f8fafc; }
  td { padding: 0.5rem 0.75rem; }

  /* Options row */
  .options-row {
    display: flex;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.875rem;
    cursor: pointer;
  }

  /* Result box */
  .result-box {
    background: #f0fdf4;
    border: 1px solid #86efac;
    border-radius: 0.5rem;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .result-heading {
    font-size: 0.875rem;
    font-weight: 500;
    color: #166534;
    margin: 0;
  }

  .result-text { font-size: 0.875rem; margin: 0; }

  .details-list {
    font-size: 0.75rem;
    color: #64748b;
    margin: 0.25rem 0 0 1rem;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .muted-text { font-size: 0.875rem; color: #64748b; margin: 0; }

  /* Buttons */
  .btn-primary {
    align-self: flex-start;
    padding: 0.5rem 1rem;
    background: #3b82f6;
    color: white;
    border-radius: 0.5rem;
    border: none;
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .btn-primary:hover { background: #2563eb; }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

  .btn-danger {
    padding: 0.5rem 1rem;
    background: #ef4444;
    color: white;
    border-radius: 0.5rem;
    border: none;
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .btn-danger:hover { background: #dc2626; }
  .btn-danger:disabled { opacity: 0.5; cursor: not-allowed; }

  /* Dark mode */
  @media (prefers-color-scheme: dark) {
    .tabs { border-bottom-color: #334155; }
    .tab-btn { color: #94a3b8; }
    .tab-btn:hover { color: #cbd5e1; }
    .tab-btn.active { background: #1e293b; border-color: #334155; border-bottom-color: #1e293b; color: #f1f5f9; }
    .table-wrap { border-color: #334155; }
    thead { background: #0f172a; }
    th { color: #94a3b8; border-bottom-color: #334155; }
    tbody tr { border-bottom-color: #0f172a; }
    tbody tr:hover { background: rgb(30 41 59 / 0.5); }
    .result-box { background: rgb(21 128 61 / 0.1); border-color: rgb(21 128 61 / 0.4); }
    .result-heading { color: #4ade80; }
    .details-list, .muted-text { color: #94a3b8; }
  }
</style>
