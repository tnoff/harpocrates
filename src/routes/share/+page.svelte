<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import ConfirmModal from "$lib/components/ConfirmModal.svelte";
  import { profileStore } from "$lib/stores/profile.svelte";
  import { selectionStore } from "$lib/stores/selection.svelte";
  import { toast } from "$lib/stores/toast.svelte";

  interface FileEntry { id: number; object_uuid: string; filename: string; local_path: string; file_size: number; original_md5: string; created_at: string; }
  interface ManifestFileEntry { uuid: string; filename: string; size: number; }
  interface ManifestFileList { manifest_uuid: string; files: ManifestFileEntry[]; }
  interface ShareManifest { id: number; profile_id: number; manifest_uuid: string; label: string | null; file_count: number; is_valid: boolean; created_at: string; }

  let activeTab = $state<"create" | "receive" | "manager">(profileStore.isReadOnly ? "receive" : "create");

  // ── Create tab ──────────────────────────────────────────────────────────────
  let createLabel = $state("");
  let creating = $state(false);
  let createdUuid = $state("");
  let copiedCreate = $state(false);

  // File picker state for the create tab
  let pickerFiles = $state<FileEntry[]>([]);
  let pickerSearch = $state("");
  let pickerLoading = $state(false);
  let shareSelectedIds = $state<Set<number>>(new Set(selectionStore.array));

  let filteredPickerFiles = $derived(
    pickerSearch.trim()
      ? pickerFiles.filter(f => {
          const q = pickerSearch.toLowerCase();
          return f.filename.toLowerCase().includes(q) || f.local_path.toLowerCase().includes(q);
        })
      : pickerFiles
  );

  async function loadPickerFiles() {
    pickerLoading = true;
    try {
      pickerFiles = await invoke<FileEntry[]>("list_files", { search: null });
    } catch (e) {
      toast.error(String(e));
    } finally {
      pickerLoading = false;
    }
  }

  function togglePickerFile(id: number) {
    const next = new Set(shareSelectedIds);
    if (next.has(id)) next.delete(id); else next.add(id);
    shareSelectedIds = next;
  }

  function toggleAllPicker() {
    const allFilteredIds = filteredPickerFiles.map(f => f.id);
    const allSelected = allFilteredIds.every(id => shareSelectedIds.has(id));
    const next = new Set(shareSelectedIds);
    if (allSelected) allFilteredIds.forEach(id => next.delete(id));
    else allFilteredIds.forEach(id => next.add(id));
    shareSelectedIds = next;
  }

  async function createManifest() {
    creating = true;
    createdUuid = "";
    try {
      createdUuid = await invoke<string>("create_share_manifest", {
        backupEntryIds: [...shareSelectedIds],
        label: createLabel || null,
      });
    } catch (e) {
      toast.error(String(e));
    } finally {
      creating = false;
    }
  }

  async function copyUuid(uuid: string) {
    await navigator.clipboard.writeText(uuid);
    copiedCreate = true;
    setTimeout(() => copiedCreate = false, 2000);
  }

  // ── Receive tab ─────────────────────────────────────────────────────────────
  let receiveUuid = $state("");
  let fetching = $state(false);
  let manifestFiles = $state<ManifestFileList | null>(null);
  let selectedFileUuids = $state<Set<string>>(new Set());
  let downloading = $state(false);

  async function fetchManifest() {
    fetching = true;
    manifestFiles = null;
    try {
      manifestFiles = await invoke<ManifestFileList>("receive_manifest", { manifestUuid: receiveUuid });
      selectedFileUuids = new Set(manifestFiles.files.map(f => f.uuid));
    } catch (e) {
      toast.error(String(e));
    } finally {
      fetching = false;
    }
  }

  function toggleFileUuid(uuid: string) {
    const next = new Set(selectedFileUuids);
    if (next.has(uuid)) next.delete(uuid); else next.add(uuid);
    selectedFileUuids = next;
  }

  async function downloadFiles() {
    const path = await open({ directory: true });
    if (!path || !manifestFiles) return;

    downloading = true;
    try {
      await invoke("download_from_manifest", {
        manifestUuid: manifestFiles.manifest_uuid,
        selectedUuids: [...selectedFileUuids],
        saveDirectory: path,
      });
    } catch (e) {
      toast.error(String(e));
    } finally {
      downloading = false;
    }
  }

  // ── Manager tab ─────────────────────────────────────────────────────────────
  let manifests = $state<ShareManifest[]>([]);
  let loadingManifests = $state(false);
  let copiedManagerUuid = $state<number | null>(null);
  let revokingManifest = $state<ShareManifest | null>(null);

  async function loadManifests() {
    loadingManifests = true;
    try {
      manifests = await invoke<ShareManifest[]>("list_share_manifests_cmd");
    } catch (e) {
      toast.error(String(e));
    } finally {
      loadingManifests = false;
    }
  }

  async function revokeManifest() {
    if (!revokingManifest) return;
    const id = revokingManifest.id;
    revokingManifest = null;
    try {
      await invoke("revoke_share_manifest", { manifestId: id });
      toast.success("Manifest revoked");
      await loadManifests();
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function copyManagerUuid(id: number, uuid: string) {
    await navigator.clipboard.writeText(uuid);
    copiedManagerUuid = id;
    setTimeout(() => copiedManagerUuid = null, 2000);
  }

  $effect(() => {
    if (activeTab === "manager") loadManifests();
    if (activeTab === "create") loadPickerFiles();
  });

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div class="page">
  <h2 class="page-title">Share</h2>

  <!-- Tabs -->
  <div class="tabs">
    {#if !profileStore.isReadOnly}
      <button class="tab-btn" class:active={activeTab === "create"} onclick={() => activeTab = "create"}>Create</button>
    {/if}
    <button class="tab-btn" class:active={activeTab === "receive"} onclick={() => activeTab = "receive"}>Receive</button>
    <button class="tab-btn" class:active={activeTab === "manager"} onclick={() => activeTab = "manager"}>Manager</button>
  </div>

  <!-- Create Tab -->
  {#if activeTab === "create"}
    <div class="tab-content">
      <div class="create-layout">
        <!-- File picker -->
        <div class="picker-section">
          <div class="picker-header">
            <span class="picker-title">Select files to share</span>
            <span class="picker-count">{shareSelectedIds.size} selected</span>
          </div>
          <input
            bind:value={pickerSearch}
            class="text-input picker-search"
            placeholder="Search files..."
          />
          {#if pickerLoading}
            <p class="muted-text">Loading files...</p>
          {:else if pickerFiles.length === 0}
            <p class="muted-text">No files in bucket.</p>
          {:else}
            <div class="table-wrap picker-table">
              <table>
                <thead>
                  <tr>
                    <th class="col-check">
                      <input
                        type="checkbox"
                        checked={filteredPickerFiles.length > 0 && filteredPickerFiles.every(f => shareSelectedIds.has(f.id))}
                        indeterminate={filteredPickerFiles.some(f => shareSelectedIds.has(f.id)) && !filteredPickerFiles.every(f => shareSelectedIds.has(f.id))}
                        onchange={toggleAllPicker}
                      />
                    </th>
                    <th>Filename</th>
                    <th>Size</th>
                  </tr>
                </thead>
                <tbody>
                  {#each filteredPickerFiles as file}
                    <tr onclick={() => togglePickerFile(file.id)} class="picker-row">
                      <td><input type="checkbox" checked={shareSelectedIds.has(file.id)} onchange={() => togglePickerFile(file.id)} onclick={(e) => e.stopPropagation()} /></td>
                      <td class="filename-cell">{file.filename}</td>
                      <td class="col-nowrap">{formatSize(file.file_size)}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
            {#if filteredPickerFiles.length === 0}
              <p class="muted-text">No files match your search.</p>
            {/if}
          {/if}
        </div>

        <!-- Create form -->
        <div class="create-form">
          <div class="field">
            <label class="field-label" for="share-label">Label <span class="optional">(optional)</span></label>
            <input id="share-label" bind:value={createLabel} class="text-input" placeholder="Share description" />
          </div>

          {#if createdUuid}
            <div class="success-box">
              <p class="success-heading">Share created!</p>
              <div class="uuid-row">
                <code class="uuid-code">{createdUuid}</code>
                <button onclick={() => copyUuid(createdUuid)} class="btn-copy">
                  {copiedCreate ? "Copied!" : "Copy"}
                </button>
              </div>
            </div>
          {/if}

          <button
            onclick={createManifest}
            disabled={creating || shareSelectedIds.size === 0}
            class="btn-primary"
          >
            {creating ? "Creating..." : `Create Share (${shareSelectedIds.size} file${shareSelectedIds.size === 1 ? "" : "s"})`}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Receive Tab -->
  {#if activeTab === "receive"}
    <div class="tab-content narrow">
      <div class="input-row">
        <input bind:value={receiveUuid} class="text-input flex-1" placeholder="Paste share UUID" />
        <button onclick={fetchManifest} disabled={!receiveUuid || fetching} class="btn-primary">
          {fetching ? "Fetching..." : "Fetch"}
        </button>
      </div>

      {#if manifestFiles}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th class="col-check">
                  <input type="checkbox"
                    checked={selectedFileUuids.size === manifestFiles.files.length}
                    onchange={() => {
                      if (selectedFileUuids.size === manifestFiles!.files.length) selectedFileUuids = new Set();
                      else selectedFileUuids = new Set(manifestFiles!.files.map(f => f.uuid));
                    }}
                  />
                </th>
                <th>Filename</th>
                <th>Size</th>
              </tr>
            </thead>
            <tbody>
              {#each manifestFiles.files as file}
                <tr>
                  <td><input type="checkbox" checked={selectedFileUuids.has(file.uuid)} onchange={() => toggleFileUuid(file.uuid)} /></td>
                  <td>{file.filename}</td>
                  <td>{formatSize(file.size)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <button onclick={downloadFiles} disabled={downloading || selectedFileUuids.size === 0} class="btn-primary">
          {downloading ? "Downloading..." : `Download ${selectedFileUuids.size} file(s)`}
        </button>
      {/if}
    </div>
  {/if}

  <!-- Manager Tab -->
  {#if activeTab === "manager"}
    <div class="tab-content">
      {#if loadingManifests}
        <p class="muted-text">Loading manifests...</p>
      {:else if manifests.length === 0}
        <p class="muted-text">No share manifests found.</p>
      {:else}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Label</th>
                <th>UUID</th>
                <th class="col-center">Files</th>
                <th class="col-center">Status</th>
                <th class="col-center">Created</th>
                <th class="col-center">Actions</th>
              </tr>
            </thead>
            <tbody>
              {#each manifests as manifest}
                <tr>
                  <td>{manifest.label ?? "—"}</td>
                  <td class="col-mono">{manifest.manifest_uuid.slice(0, 8)}...</td>
                  <td class="col-center">{manifest.file_count}</td>
                  <td class="col-center">
                    <span class="badge" class:badge-valid={manifest.is_valid} class:badge-revoked={!manifest.is_valid}>
                      {manifest.is_valid ? "Valid" : "Revoked"}
                    </span>
                  </td>
                  <td class="col-nowrap">{new Date(manifest.created_at).toLocaleDateString()}</td>
                  <td>
                    <div class="action-btns">
                      <button onclick={() => copyManagerUuid(manifest.id, manifest.manifest_uuid)} class="btn-xs btn-secondary">
                        {copiedManagerUuid === manifest.id ? "Copied!" : "Copy UUID"}
                      </button>
                      {#if manifest.is_valid}
                        <button onclick={() => revokingManifest = manifest} class="btn-xs btn-danger">Revoke</button>
                      {/if}
                    </div>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  {/if}
</div>

{#if revokingManifest}
  <ConfirmModal
    title="Revoke Share"
    message="Revoke '{revokingManifest.label ?? revokingManifest.manifest_uuid.slice(0, 8)}'? Recipients will no longer be able to download files from this share."
    confirmLabel="Revoke"
    danger={true}
    onconfirm={revokeManifest}
    oncancel={() => revokingManifest = null}
  />
{/if}

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

  /* Tab content */
  .tab-content {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .narrow { max-width: 32rem; }

  /* Create layout */
  .create-layout {
    display: grid;
    grid-template-columns: 1fr 20rem;
    gap: 1.5rem;
    align-items: start;
  }

  .picker-section {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .picker-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }

  .picker-title {
    font-size: 0.875rem;
    font-weight: 500;
  }

  .picker-count {
    font-size: 0.8rem;
    color: #64748b;
  }

  .picker-search { width: 100%; }

  .picker-table {
    max-height: 24rem;
    overflow-y: auto;
  }

  .picker-row { cursor: pointer; }

  .filename-cell {
    max-width: 0;
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .create-form {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    position: sticky;
    top: 1rem;
  }

  /* Form elements */
  .field { display: flex; flex-direction: column; gap: 0.375rem; }
  .field-label { font-size: 0.875rem; font-weight: 500; }
  .optional { font-weight: 400; color: #94a3b8; }

  .text-input {
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid #cbd5e1;
    background: white;
    font-size: 0.875rem;
    outline: none;
    transition: border-color 0.15s;
  }

  .text-input:focus {
    border-color: #3b82f6;
    box-shadow: 0 0 0 2px rgb(59 130 246 / 0.2);
  }

  .flex-1 { flex: 1; }

  .input-row { display: flex; gap: 0.5rem; }

  .muted-text { font-size: 0.875rem; color: #64748b; margin: 0; }

  /* Success box */
  .success-box {
    background: #f0fdf4;
    border: 1px solid #86efac;
    border-radius: 0.5rem;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .success-heading {
    font-size: 0.875rem;
    font-weight: 500;
    color: #166534;
    margin: 0;
  }

  .uuid-row { display: flex; gap: 0.5rem; align-items: stretch; }

  .uuid-code {
    flex: 1;
    background: white;
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    border: 1px solid #e2e8f0;
    font-size: 0.875rem;
    font-family: monospace;
    word-break: break-all;
    user-select: all;
  }

  .btn-copy {
    padding: 0.5rem 0.75rem;
    background: #bbf7d0;
    color: #166534;
    border: none;
    border-radius: 0.375rem;
    font-size: 0.875rem;
    cursor: pointer;
    white-space: nowrap;
    transition: background-color 0.15s;
  }

  .btn-copy:hover { background: #86efac; }

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
  .col-center { text-align: center; }
  .col-mono { font-family: monospace; font-size: 0.75rem; }
  .col-nowrap { white-space: nowrap; }

  tbody tr { border-bottom: 1px solid #f1f5f9; transition: background-color 0.1s; }
  tbody tr:last-child { border-bottom: none; }
  tbody tr:hover { background: #f8fafc; }

  td { padding: 0.5rem 0.75rem; }

  /* Badge */
  .badge {
    display: inline-block;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    font-size: 0.75rem;
  }

  .badge-valid { background: #dcfce7; color: #15803d; }
  .badge-revoked { background: #fee2e2; color: #b91c1c; }

  /* Action buttons */
  .action-btns { display: flex; gap: 0.5rem; }

  .btn-xs {
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    border: none;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .btn-xs.btn-secondary { background: #f1f5f9; color: #334155; }
  .btn-xs.btn-secondary:hover { background: #e2e8f0; }
  .btn-xs.btn-danger { background: #fee2e2; color: #ef4444; }
  .btn-xs.btn-danger:hover { background: #fecaca; }

  /* Primary button */
  .btn-primary {
    padding: 0.5rem 1rem;
    background: #3b82f6;
    color: white;
    border-radius: 0.5rem;
    border: none;
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
    white-space: nowrap;
  }

  .btn-primary:hover { background: #2563eb; }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

  /* Dark mode */
  @media (prefers-color-scheme: dark) {
    .tabs { border-bottom-color: #334155; }
    .tab-btn { color: #94a3b8; }
    .tab-btn:hover { color: #cbd5e1; }
    .tab-btn.active { background: #1e293b; border-color: #334155; border-bottom-color: #1e293b; color: #f1f5f9; }
    .text-input { background: #0f172a; border-color: #475569; color: #f1f5f9; }
    .text-input::placeholder { color: #64748b; }
    .picker-count { color: #64748b; }
    .muted-text { color: #94a3b8; }
    .success-box { background: rgb(21 128 61 / 0.1); border-color: rgb(21 128 61 / 0.4); }
    .success-heading { color: #4ade80; }
    .uuid-code { background: #0f172a; border-color: #334155; color: #f1f5f9; }
    .btn-copy { background: rgb(21 128 61 / 0.3); color: #4ade80; }
    .btn-copy:hover { background: rgb(21 128 61 / 0.5); }
    .table-wrap { border-color: #334155; }
    thead { background: #0f172a; }
    th { color: #94a3b8; border-bottom-color: #334155; }
    tbody tr { border-bottom-color: #0f172a; }
    tbody tr:hover { background: rgb(30 41 59 / 0.5); }
    .badge-valid { background: rgb(21 128 61 / 0.2); color: #4ade80; }
    .badge-revoked { background: rgb(185 28 28 / 0.2); color: #f87171; }
    .btn-xs.btn-secondary { background: #334155; color: #cbd5e1; }
    .btn-xs.btn-secondary:hover { background: #475569; }
    .btn-xs.btn-danger { background: rgb(127 29 29 / 0.3); color: #f87171; }
    .btn-xs.btn-danger:hover { background: rgb(127 29 29 / 0.5); }
  }
</style>
