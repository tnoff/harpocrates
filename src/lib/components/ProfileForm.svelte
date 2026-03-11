<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { homeDir } from "@tauri-apps/api/path";
  import { untrack } from "svelte";

  interface Props {
    initial?: {
      id?: number;
      name?: string;
      mode?: string;
      s3_endpoint?: string;
      s3_region?: string | null;
      s3_bucket?: string;
      s3_access_key?: string;
      s3_secret_key?: string;
      extra_env?: string | null;
      relative_path?: string | null;
      temp_directory?: string | null;
      s3_key_prefix?: string | null;
      upload_chunk_size_mb?: number | null;
    };
    onsubmit: (data: Record<string, unknown>) => Promise<void>;
    submitLabel?: string;
  }

  let { initial = {}, onsubmit, submitLabel = "Create Profile" }: Props = $props();

  // Snapshot the prop values once at mount. The parent always destroys and
  // recreates this component when the editing target changes ({#if editingProfile}),
  // so untracked reads here are intentional — the prop won't change mid-lifetime.
  const iv = untrack(() => ({
    name: initial.name ?? "",
    mode: initial.mode ?? "read-write",
    s3Endpoint: initial.s3_endpoint ?? "",
    s3Region: initial.s3_region ?? "",
    s3Bucket: initial.s3_bucket ?? "",
    s3AccessKey: initial.s3_access_key ?? "",
    s3SecretKey: initial.s3_secret_key ?? "",
    extraEnv: initial.extra_env ?? "",
    relativePath: initial.relative_path ?? "",
    tempDirectory: initial.temp_directory ?? "",
    s3KeyPrefix: initial.s3_key_prefix ?? "",
    uploadChunkSizeMb: initial.upload_chunk_size_mb ?? 256,
  }));

  let name = $state(iv.name);
  let mode = $state(iv.mode);
  let s3Endpoint = $state(iv.s3Endpoint);
  let s3Region = $state(iv.s3Region);
  let s3Bucket = $state(iv.s3Bucket);
  let s3AccessKey = $state(iv.s3AccessKey);
  let s3SecretKey = $state(iv.s3SecretKey);
  let extraEnv = $state(iv.extraEnv);
  let relativePath = $state(iv.relativePath);
  let tempDirectory = $state(iv.tempDirectory);
  let s3KeyPrefix = $state(iv.s3KeyPrefix);
  let uploadChunkSizeMb = $state(iv.uploadChunkSizeMb);
  let importKey = $state("");


  let isReadOnly = $derived(mode === "read-only");


  let submitting = $state(false);
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; message: string } | null>(null);
  let error = $state("");

  async function handleSubmit() {
    error = "";

    if (s3KeyPrefix.trim()) {
      const cleaned = s3KeyPrefix.trim().replace(/^\/+|\/+$/g, '');
      if (cleaned.length > 200) {
        error = 'S3 key prefix must not exceed 200 characters';
        return;
      }
      if (cleaned.includes('//')) {
        error = 'S3 key prefix must not contain consecutive slashes';
        return;
      }
    }

    submitting = true;
    try {
      await onsubmit({
        name,
        mode,
        s3_endpoint: s3Endpoint,
        s3_region: s3Region || null,
        s3_bucket: s3Bucket,
        s3_access_key: s3AccessKey,
        s3_secret_key: s3SecretKey,
        extra_env: extraEnv || null,
        relative_path: relativePath || null,
        temp_directory: tempDirectory || null,
        import_encryption_key: importKey || null,
        s3_key_prefix: s3KeyPrefix.trim() || null,
        upload_chunk_size_mb: uploadChunkSizeMb > 0 ? uploadChunkSizeMb : null,
      });
    } catch (e) {
      error = String(e);
    } finally {
      submitting = false;
    }
  }

  async function testConnection() {
    testing = true;
    testResult = null;
    try {
      const msg = await invoke<string>("test_connection_params", {
        endpoint: s3Endpoint,
        region: s3Region || null,
        bucket: s3Bucket,
        accessKey: s3AccessKey,
        secretKey: s3SecretKey,
        extraEnv: extraEnv || null,
      });
      testResult = { ok: true, message: msg };
    } catch (e) {
      testResult = { ok: false, message: String(e) };
    } finally {
      testing = false;
    }
  }

</script>

<form onsubmit={(e) => { e.preventDefault(); handleSubmit(); }} style="display: flex; flex-direction: column; gap: 2rem; max-width: 40rem;">
  <div>
    <label class="form-label" for="pf-name">Profile Name</label>
    <input id="pf-name" bind:value={name} required class="form-input" placeholder="My Harpocrates" />
    <p class="form-hint">A friendly label to identify this connection in the app.</p>
  </div>

  <div>
    <label class="form-label" for="pf-mode">Mode</label>
    <select id="pf-mode" bind:value={mode} class="form-input">
      <option value="read-write">Read-Write</option>
      <option value="read-only">Read-Only</option>
    </select>
    <p class="form-hint">Read-Write allows uploading and managing files. Read-Only restricts to downloads only.</p>
  </div>

  <fieldset style="display: flex; flex-direction: column; gap: 1.5rem; border-radius: 0.5rem; padding: 1.25rem;" class="border border-slate-200 dark:border-slate-600">
    <legend class="text-sm font-semibold px-2 text-slate-700 dark:text-slate-300">S3 Connection</legend>

    <div>
      <label class="form-label" for="pf-endpoint">Endpoint URL</label>
      <input id="pf-endpoint" bind:value={s3Endpoint} required class="form-input" placeholder="https://s3.amazonaws.com" />
      <p class="form-hint">The base URL of your S3-compatible provider. For AWS use <code class="font-mono">https://s3.amazonaws.com</code>; for others check your provider's docs.</p>
    </div>

    <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 1rem;">
      <div>
        <label class="form-label" for="pf-region">Region <span class="font-normal text-slate-400 dark:text-slate-500">(optional)</span></label>
        <input id="pf-region" bind:value={s3Region} class="form-input" placeholder="us-east-1" />
        <p class="form-hint">Required by AWS and some providers. Leave blank if your provider doesn't use regions.</p>
      </div>
      <div>
        <label class="form-label" for="pf-bucket">Bucket</label>
        <input id="pf-bucket" bind:value={s3Bucket} required class="form-input" placeholder="my-vault-bucket" />
        <p class="form-hint">The S3 bucket where your encrypted files will be stored.</p>
      </div>
    </div>

    <div>
      <label class="form-label" for="pf-access-key">Access Key</label>
      <input id="pf-access-key" bind:value={s3AccessKey} required class="form-input" placeholder="AKIAIOSFODNN7EXAMPLE" />
      <p class="form-hint">Your S3 access key ID. For AWS, generate one under IAM → Users → Security credentials.</p>
    </div>

    <div>
      <label class="form-label" for="pf-secret-key">Secret Key</label>
      <input id="pf-secret-key" bind:value={s3SecretKey} required type="password" class="form-input" placeholder="••••••••••••••••••••" />
      <p class="form-hint">Your S3 secret access key. Stored locally on this device only.</p>
    </div>

    <div>
      <label class="form-label" for="pf-extra-env">
        Extra Environment Variables <span class="font-normal text-slate-400 dark:text-slate-500">(optional)</span>
      </label>
      <input id="pf-extra-env" bind:value={extraEnv} class="form-input" placeholder="KEY=val,KEY2=val2" />
      <p class="form-hint">Comma-separated <code class="font-mono">KEY=value</code> pairs passed to the S3 client. Useful for proxy settings or custom TLS configuration.</p>
    </div>

    <div>
      <label class="form-label" for="s3-key-prefix">Key Prefix <span class="font-normal text-slate-400 dark:text-slate-500">(optional)</span></label>
      <input
        id="s3-key-prefix"
        type="text"
        bind:value={s3KeyPrefix}
        class="form-input"
        placeholder="e.g. team-alpha"
      />
      <p class="form-hint">Objects will be stored at <code class="font-mono">{s3KeyPrefix ? s3KeyPrefix.replace(/^\/+|\/+$/g, '') + '/' : ''}&lt;uuid&gt;</code>. Useful for per-prefix IAM policies.</p>
    </div>
  </fieldset>

  <fieldset style="display: flex; flex-direction: column; gap: 1.5rem; border-radius: 0.5rem; padding: 1.25rem;" class="border border-slate-200 dark:border-slate-600">
    <legend class="text-sm font-semibold px-2 text-slate-700 dark:text-slate-300">Paths <span class="font-normal text-slate-400 dark:text-slate-500">(optional)</span></legend>
    <div>
      <label class="form-label" for="pf-relative-path">Relative Path <span class="font-normal text-slate-400 dark:text-slate-500">(base directory)</span></label>
      <div style="display: flex; gap: 0.5rem;">
        <input id="pf-relative-path" bind:value={relativePath} class="form-input" placeholder="/home/user/documents" />
        <button
          type="button"
          onclick={async () => { const p = await open({ directory: true, defaultPath: await homeDir() }); if (p) relativePath = p; }}
          class="btn-secondary"
          style="flex: none; padding-left: 0.75rem; padding-right: 0.75rem;"
        >Browse</button>
      </div>
      <p class="form-hint">Local base directory to strip when storing file paths. For example, with a base of <code class="font-mono">/home/user/docs</code>, the file <code class="font-mono">/home/user/docs/report.pdf</code> is stored and restored as <code class="font-mono">report.pdf</code> relative to that path.</p>
    </div>
    <div>
      <label class="form-label" for="pf-temp-dir">Temp Directory</label>
      <div style="display: flex; gap: 0.5rem;">
        <input id="pf-temp-dir" bind:value={tempDirectory} class="form-input" placeholder="/tmp/harpocrates" />
        <button
          type="button"
          onclick={async () => { const p = await open({ directory: true, defaultPath: await homeDir() }); if (p) tempDirectory = p; }}
          class="btn-secondary"
          style="flex: none; padding-left: 0.75rem; padding-right: 0.75rem;"
        >Browse</button>
      </div>
      <p class="form-hint">Local folder for temporary files during transfers. Defaults to your system temp folder if left blank.</p>
    </div>
    <div>
      <label class="form-label" for="pf-chunk-size">Upload chunk size (MB)</label>
      <input id="pf-chunk-size" type="number" min="5" max="10240" bind:value={uploadChunkSizeMb} class="form-input" style="max-width: 12rem;" />
      <p class="form-hint">Files are encrypted and uploaded in chunks of this size. Larger chunks mean fewer S3 requests and faster transfers, but each chunk uses ~2× this amount of RAM. Default: 256 MB.</p>
    </div>
  </fieldset>

  {#if !initial.id}
    <fieldset style="display: flex; flex-direction: column; gap: 1.5rem; border-radius: 0.5rem; padding: 1.25rem;" class="border border-slate-200 dark:border-slate-600">
      <legend class="text-sm font-semibold px-2 text-slate-700 dark:text-slate-300">Encryption Key</legend>

      {#if isReadOnly}
        <div>
          <label class="form-label" for="pf-import-key">Encryption Key</label>
          <input id="pf-import-key" bind:value={importKey} type="password" required class="form-input" placeholder="Paste your 64-character hex key" />
          <p class="form-hint">Read-only profiles can only decrypt — provide the key from the original vault.</p>
        </div>
      {:else}
        <div>
          <label class="form-label" for="pf-import-key">Encryption Key</label>
          <input id="pf-import-key" bind:value={importKey} type="password" class="form-input" placeholder="Paste your 64-character hex key" />
          <p class="form-hint">If you have an existing key, paste it here. Leave blank to generate a new one — save it after creation, it cannot be recovered if lost.</p>
        </div>
      {/if}
    </fieldset>
  {/if}

  {#if testResult}
    <p style="font-size: 0.875rem; color: {testResult.ok ? '#22c55e' : '#ef4444'};">{testResult.message}</p>
  {/if}

  {#if error}
    <p style="font-size: 0.875rem; color: #ef4444;">{error}</p>
  {/if}

  <div style="display: flex; gap: 0.75rem;">
    <button type="submit" disabled={submitting} class="btn-primary">
      {submitting ? "Saving..." : submitLabel}
    </button>
    <button
      type="button"
      onclick={testConnection}
      disabled={testing}
      class="btn-secondary"
    >
      {testing ? "Testing..." : "Test Connection"}
    </button>
  </div>
</form>

