<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { save, open } from "@tauri-apps/plugin-dialog";
  import ProfileForm from "$lib/components/ProfileForm.svelte";
  import ConfirmModal from "$lib/components/ConfirmModal.svelte";
  import { profileStore, type Profile } from "$lib/stores/profile.svelte";
  import { toast } from "$lib/stores/toast.svelte";

  type EditingProfile = Profile & { s3_access_key?: string; s3_secret_key?: string };
  let editingProfile = $state<EditingProfile | null>(null);

  // ── Profile config export / import ───────────────────

  async function exportProfileConfig(profile: Profile) {
    const path = await save({
      defaultPath: `${profile.name.replace(/\s+/g, '-')}-profile.json`,
      filters: [{ name: "Harpocrates Profile", extensions: ["json"] }],
    });
    if (!path) return;
    try {
      await invoke("export_profile_config", { profileId: profile.id, filePath: path });
      toast.success(`Profile exported to ${path}`);
    } catch (e) {
      toast.error(String(e));
    }
  }

  interface ImportState { filePath: string; encryptionKey: string; mode: string; }
  let importState = $state<ImportState | null>(null);
  let importing = $state(false);

  async function startImport() {
    const path = await open({ filters: [{ name: "Harpocrates Profile", extensions: ["json"] }] });
    if (!path) return;
    importState = { filePath: path as string, encryptionKey: "", mode: "read-write" };
  }

  async function confirmImport() {
    if (!importState) return;
    importing = true;
    try {
      await invoke("import_profile_config", {
        filePath: importState.filePath,
        encryptionKey: importState.encryptionKey,
        modeOverride: importState.mode,
      });
      toast.success("Profile imported");
      importState = null;
      await profileStore.load();
    } catch (e) {
      toast.error(String(e));
    } finally {
      importing = false;
    }
  }

  async function startEditProfile(profile: Profile) {
    try {
      const creds = await invoke<{ s3_access_key: string; s3_secret_key: string }>(
        "get_profile_credentials",
        { profileId: profile.id },
      );
      editingProfile = { ...profile, ...creds };
    } catch {
      // Keychain unavailable — open form with blank credential fields
      editingProfile = { ...profile };
    }
  }
  let deletingProfile = $state<Profile | null>(null);
  let testingId = $state<number | null>(null);
  let testResult = $state<{ id: number; ok: boolean; message: string } | null>(null);

  async function handleUpdate(data: Record<string, unknown>) {
    if (!editingProfile) return;
    try {
      await invoke("update_profile", { input: { id: editingProfile.id, ...data } });
      toast.success("Profile updated");
      editingProfile = null;
      await profileStore.load();
    } catch (e) {
      toast.error(String(e));
      throw e;
    }
  }

  async function handleDelete() {
    if (!deletingProfile) return;
    try {
      await invoke("delete_profile", { profileId: deletingProfile.id });
      toast.success(`Profile "${deletingProfile.name}" deleted`);
      deletingProfile = null;
      await profileStore.load();
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function testProfile(profile: Profile) {
    testingId = profile.id;
    testResult = null;
    try {
      await profileStore.switchProfile(profile.id);
      const msg = await invoke<string>("test_connection");
      testResult = { id: profile.id, ok: true, message: msg };
    } catch (e) {
      testResult = { id: profile.id, ok: false, message: String(e) };
    } finally {
      testingId = null;
    }
  }

  // ── Throttle ─────────────────────────────────────────

  interface ThrottleLimits { upload_bps: number; download_bps: number; }

  let uploadKbps = $state(0);
  let downloadKbps = $state(0);
  let throttleSaved = $state(false);

  async function loadThrottle() {
    try {
      const limits = await invoke<ThrottleLimits>("get_throttle_limits");
      uploadKbps = limits.upload_bps ? Math.round(limits.upload_bps / 1024) : 0;
      downloadKbps = limits.download_bps ? Math.round(limits.download_bps / 1024) : 0;
    } catch { /* ignore */ }
  }

  async function applyThrottle() {
    try {
      await invoke("set_throttle_limits", {
        uploadBps: uploadKbps > 0 ? uploadKbps * 1024 : 0,
        downloadBps: downloadKbps > 0 ? downloadKbps * 1024 : 0,
      });
      throttleSaved = true;
      setTimeout(() => (throttleSaved = false), 2000);
    } catch (e) {
      toast.error(String(e));
    }
  }

  $effect(() => { loadThrottle(); });

  // ── Database location ────────────────────────────────

  interface AppConfig { database_path: string; }

  let currentDbPath = $state("");
  let newDbPath = $state("");
  let copyExisting = $state(true);
  let dbPendingRestart = $state(false);

  async function loadDbPath() {
    try {
      const cfg = await invoke<AppConfig>("get_config");
      currentDbPath = cfg.database_path;
      newDbPath = cfg.database_path;
    } catch { /* ignore */ }
  }

  async function browseDbPath() {
    const p = await save({
      defaultPath: "harpocrates.db",
      filters: [{ name: "SQLite Database", extensions: ["db"] }],
    });
    if (p) newDbPath = p;
  }

  async function applyDbPath() {
    try {
      await invoke("set_database_path", { newPath: newDbPath, copyExisting });
      currentDbPath = newDbPath;
      dbPendingRestart = true;
    } catch (e) {
      toast.error(String(e));
    }
  }

  $effect(() => { loadDbPath(); });

  // ── Database export / import ─────────────────────────

  async function exportDb() {
    const path = await save({ defaultPath: "harpocrates-export.json", filters: [{ name: "JSON", extensions: ["json"] }] });
    if (!path) return;
    try {
      await invoke("export_database", { filePath: path });
      toast.success(`Exported to ${path}`);
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function importDb() {
    const path = await open({ filters: [{ name: "JSON", extensions: ["json"] }] });
    if (!path) return;
    try {
      await invoke("import_database", { filePath: path });
      toast.success("Database imported successfully");
      await profileStore.load();
    } catch (e) {
      toast.error(String(e));
    }
  }

  const blankProfile: Profile = {
    id: 0, name: "", mode: "read-write", s3_endpoint: "", s3_region: null,
    s3_bucket: "", extra_env: null, relative_path: null, temp_directory: null,
    s3_key_prefix: null, upload_chunk_size_mb: null, is_active: false, created_at: "",
  };
</script>

<div class="page">
  <h2 class="page-title">Settings</h2>

  <!-- Profiles -->
  <section class="section">
    <h3 class="section-title">Profiles</h3>

    <div class="profile-list">
      {#each profileStore.profiles as profile}
        <div class="profile-row">
          <div class="profile-info">
            <div class="profile-name">
              {profile.name}
              {#if profile.is_active}
                <span class="badge-active">Active</span>
              {/if}
            </div>
            <div class="profile-meta">
              {profile.mode} &middot; {profile.s3_bucket} &middot; {profile.s3_endpoint}
            </div>
          </div>

          <div class="profile-actions">
            <button
              onclick={() => testProfile(profile)}
              disabled={testingId === profile.id}
              class="btn-xs btn-neutral"
            >
              {testingId === profile.id ? "Testing..." : "Test"}
            </button>
            <button onclick={() => startEditProfile(profile)} class="btn-xs btn-neutral">Edit</button>
            <button onclick={() => exportProfileConfig(profile)} class="btn-xs btn-neutral">Export</button>
            <button onclick={() => deletingProfile = profile} class="btn-xs btn-destructive">Delete</button>
          </div>
        </div>

        {#if testResult && testResult.id === profile.id}
          <p class="test-result" class:test-ok={testResult.ok} class:test-error={!testResult.ok}>
            {testResult.message}
          </p>
        {/if}
      {/each}
    </div>

    <div class="btn-row">
      <button onclick={() => editingProfile = blankProfile} class="btn-primary">
        Add Profile
      </button>
      <button onclick={startImport} class="btn-secondary">
        Import Profile
      </button>
    </div>
  </section>

  <!-- Bandwidth -->
  <section class="section">
    <h3 class="section-title">Bandwidth</h3>
    <p class="section-hint">Set to 0 for unlimited. Changes take effect on the next chunk during an active transfer.</p>
    <div class="throttle-row">
      <div class="throttle-field">
        <label class="field-label" for="upload-kbps">Upload limit (KB/s)</label>
        <input id="upload-kbps" type="number" min="0" bind:value={uploadKbps} class="number-input" />
      </div>
      <div class="throttle-field">
        <label class="field-label" for="download-kbps">Download limit (KB/s)</label>
        <input id="download-kbps" type="number" min="0" bind:value={downloadKbps} class="number-input" />
      </div>
      <button onclick={applyThrottle} class="btn-primary" class:btn-saved={throttleSaved}>
        {throttleSaved ? "Saved ✓" : "Apply"}
      </button>
    </div>
  </section>

  <!-- Database -->
  <section class="section">
    <h3 class="section-title">Database</h3>

    <div>
      <label class="field-label" for="db-path">Database File Location</label>
      <div class="path-row">
        <input
          id="db-path"
          bind:value={newDbPath}
          class="path-input"
          placeholder="/path/to/harpocrates.db"
        />
        <button type="button" onclick={browseDbPath} class="btn-secondary" style="flex: none;">
          Browse
        </button>
      </div>
      <p class="section-hint">Takes effect after restarting the app.</p>
    </div>

    {#if newDbPath && newDbPath !== currentDbPath}
      <label class="copy-label">
        <input type="checkbox" bind:checked={copyExisting} />
        Copy existing database to new location
      </label>
      <button onclick={applyDbPath} class="btn-primary">Apply</button>
    {/if}

    {#if dbPendingRestart}
      <p class="restart-notice">⚠ Restart Harpocrates to use the new database location.</p>
    {/if}

    <div class="btn-row">
      <button onclick={exportDb} class="btn-secondary">Export Database</button>
      <button onclick={importDb} class="btn-secondary">Import Database</button>
    </div>
  </section>
</div>

<!-- Import Profile Modal -->
{#if importState}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="overlay"
    onclick={() => importState = null}
    onkeydown={(e) => e.key === "Escape" && (importState = null)}
    role="presentation"
  >
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="import-profile-title"
      tabindex="-1"
      class="modal-dialog"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <h3 id="import-profile-title" class="modal-title">Import Profile</h3>
      <p class="import-hint">
        Enter the encryption key for this profile. The mode can be changed below — importing as
        read-only is useful if you only need to restore or verify files.
      </p>

      <div class="import-filepath">{importState.filePath}</div>

      <div class="import-field">
        <label class="form-label" for="import-enc-key">Encryption Key</label>
        <input
          id="import-enc-key"
          type="password"
          class="form-input"
          placeholder="Paste your encryption key"
          bind:value={importState.encryptionKey}
        />
      </div>

      <div class="import-field">
        <label class="form-label" for="import-mode">Access Mode</label>
        <select id="import-mode" class="form-input" bind:value={importState.mode}>
          <option value="read-write">Read-write</option>
          <option value="read-only">Read-only</option>
        </select>
      </div>

      <button
        onclick={confirmImport}
        disabled={importing || !importState.encryptionKey.trim()}
        class="btn-primary"
        style="margin-top: 0.5rem;"
      >
        {importing ? "Importing..." : "Import"}
      </button>
      <button onclick={() => importState = null} class="btn-cancel-link">Cancel</button>
    </div>
  </div>
{/if}

<!-- Edit / Create Profile Modal -->
{#if editingProfile}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="overlay"
    onclick={() => editingProfile = null}
    onkeydown={(e) => e.key === "Escape" && (editingProfile = null)}
    role="presentation"
  >
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="edit-profile-title"
      tabindex="-1"
      class="modal-dialog"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <h3 id="edit-profile-title" class="modal-title">
        {editingProfile.id ? "Edit Profile" : "New Profile"}
      </h3>

      {#if editingProfile.id}
        <ProfileForm initial={editingProfile} onsubmit={handleUpdate} submitLabel="Save Changes" />
      {:else}
        <ProfileForm onsubmit={async (data) => {
          try {
            await invoke("create_profile", { input: data });
            toast.success("Profile created");
            editingProfile = null;
            await profileStore.load();
          } catch (e) {
            toast.error(String(e));
            throw e;
          }
        }} submitLabel="Create Profile" />
      {/if}

      <button onclick={() => editingProfile = null} class="btn-cancel-link">Cancel</button>
    </div>
  </div>
{/if}

<!-- Delete Confirmation -->
{#if deletingProfile}
  <ConfirmModal
    title="Delete Profile"
    message="Are you sure you want to delete '{deletingProfile.name}'? This cannot be undone."
    confirmLabel="Delete"
    danger={true}
    onconfirm={handleDelete}
    oncancel={() => deletingProfile = null}
  />
{/if}

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 2rem;
  }

  .page-title {
    font-size: 1.25rem;
    font-weight: 700;
    margin: 0;
  }

  /* Sections */
  .section {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .section-title {
    font-size: 1.0625rem;
    font-weight: 600;
    margin: 0;
  }

  .section-hint {
    font-size: 0.75rem;
    color: #64748b;
    margin: 0;
  }

  /* Profile list */
  .profile-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .profile-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid #e2e8f0;
    background: white;
  }

  .profile-info {
    flex: 1;
    min-width: 0;
  }

  .profile-name {
    font-size: 0.875rem;
    font-weight: 500;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .profile-meta {
    font-size: 0.75rem;
    color: #64748b;
    margin-top: 0.125rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge-active {
    display: inline-block;
    font-size: 0.6875rem;
    font-weight: 500;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    background: rgb(59 130 246 / 0.15);
    color: #3b82f6;
  }

  .profile-actions {
    display: flex;
    gap: 0.375rem;
    flex-shrink: 0;
  }

  .test-result {
    font-size: 0.75rem;
    margin: 0 0 0 0.75rem;
  }

  .test-result.test-ok { color: #22c55e; }
  .test-result.test-error { color: #ef4444; }

  /* Throttle */
  .throttle-row {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
    align-items: flex-end;
  }

  .throttle-field {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .field-label {
    font-size: 0.875rem;
    font-weight: 500;
  }

  .number-input {
    width: 8rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid #cbd5e1;
    background: white;
    font-size: 0.875rem;
    outline: none;
    transition: border-color 0.15s;
    /* hide browser spinner arrows */
    -moz-appearance: textfield;
  }

  .number-input::-webkit-outer-spin-button,
  .number-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }

  .number-input:focus {
    border-color: #3b82f6;
    box-shadow: 0 0 0 2px rgb(59 130 246 / 0.2);
  }

  /* Database path */
  .path-row {
    display: flex;
    gap: 0.5rem;
  }

  .path-input {
    flex: 1;
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid #cbd5e1;
    background: white;
    font-size: 0.875rem;
    font-family: monospace;
    outline: none;
    transition: border-color 0.15s;
    min-width: 0;
  }

  .path-input:focus {
    border-color: #3b82f6;
    box-shadow: 0 0 0 2px rgb(59 130 246 / 0.2);
  }

  .copy-label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.875rem;
    cursor: pointer;
  }

  .restart-notice {
    font-size: 0.8125rem;
    color: #b45309;
    margin: 0;
    padding: 0.5rem 0.75rem;
    background: #fffbeb;
    border: 1px solid #fcd34d;
    border-radius: 0.5rem;
  }

  /* Button row */
  .btn-row {
    display: flex;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

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
  .btn-saved { background: #16a34a !important; }

  .btn-secondary {
    padding: 0.5rem 1rem;
    background: #e2e8f0;
    color: #334155;
    border-radius: 0.5rem;
    border: none;
    font-size: 0.875rem;
    cursor: pointer;
    transition: background-color 0.15s;
  }
  .btn-secondary:hover { background: #cbd5e1; }

  .btn-xs {
    padding: 0.25rem 0.625rem;
    border-radius: 0.375rem;
    border: none;
    font-size: 0.75rem;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .btn-neutral { background: #f1f5f9; color: #334155; }
  .btn-neutral:hover { background: #e2e8f0; }
  .btn-neutral:disabled { opacity: 0.5; cursor: not-allowed; }

  .btn-destructive { background: #fee2e2; color: #ef4444; }
  .btn-destructive:hover { background: #fecaca; }

  /* Import modal */
  .import-hint {
    font-size: 0.8125rem;
    color: #64748b;
    margin: 0 0 0.75rem;
  }

  .import-filepath {
    font-size: 0.75rem;
    font-family: monospace;
    color: #475569;
    background: #f8fafc;
    border: 1px solid #e2e8f0;
    border-radius: 0.375rem;
    padding: 0.375rem 0.625rem;
    margin-bottom: 0.75rem;
    word-break: break-all;
  }

  .import-field {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    margin-bottom: 0.75rem;
  }

  /* Modal */
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    z-index: 50;
    overflow-y: auto;
    padding: 2rem 1rem;
  }

  .modal-dialog {
    background: white;
    border-radius: 0.75rem;
    box-shadow: 0 20px 25px -5px rgb(0 0 0 / 0.15);
    padding: 1.5rem;
    max-width: 44rem;
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .modal-title {
    font-size: 1.125rem;
    font-weight: 600;
    margin: 0 0 1rem;
  }

  .btn-cancel-link {
    align-self: flex-start;
    margin-top: 0.75rem;
    background: none;
    border: none;
    font-size: 0.875rem;
    color: #64748b;
    cursor: pointer;
    padding: 0;
    transition: color 0.15s;
  }
  .btn-cancel-link:hover { color: #334155; }

  /* Dark mode */
  @media (prefers-color-scheme: dark) {
    .import-hint { color: #94a3b8; }
    .import-filepath { background: #0f172a; border-color: #334155; color: #94a3b8; }
    .section-hint { color: #94a3b8; }
    .profile-row { background: #1e293b; border-color: #334155; }
    .profile-meta { color: #94a3b8; }
    .badge-active { background: rgb(59 130 246 / 0.2); color: #60a5fa; }
    .number-input { background: #1e293b; border-color: #475569; color: #f1f5f9; }
    .path-input { background: #1e293b; border-color: #475569; color: #f1f5f9; }
    .copy-label { color: #cbd5e1; }
    .restart-notice { background: #1c1a0e; border-color: #78350f; color: #fcd34d; }
    .btn-secondary { background: #334155; color: #cbd5e1; }
    .btn-secondary:hover { background: #475569; }
    .btn-neutral { background: #334155; color: #cbd5e1; }
    .btn-neutral:hover { background: #475569; }
    .btn-destructive { background: rgb(127 29 29 / 0.3); color: #f87171; }
    .btn-destructive:hover { background: rgb(127 29 29 / 0.5); }
    .modal-dialog { background: #1e293b; color: #f1f5f9; }
    .btn-cancel-link { color: #94a3b8; }
    .btn-cancel-link:hover { color: #cbd5e1; }
  }
</style>
