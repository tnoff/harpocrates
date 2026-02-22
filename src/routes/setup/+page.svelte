<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { goto } from "$app/navigation";
  import ProfileForm from "$lib/components/ProfileForm.svelte";
  import { profileStore, type Profile } from "$lib/stores/profile.svelte";
  import { toast } from "$lib/stores/toast.svelte";

  interface CreateProfileResult {
    profile: Profile;
    encryption_key: string;
  }

  let encryptionKey = $state<string | null>(null);
  let copied = $state(false);

  async function handleCreate(data: Record<string, unknown>) {
    try {
      const result = await invoke<CreateProfileResult>("create_profile", { input: data });
      await profileStore.load();
      if (data.import_encryption_key) {
        // Key was provided by the user — no need to show it, go straight to the app
        goto("/files");
      } else {
        // New key was generated — user must save it before continuing
        encryptionKey = result.encryption_key;
      }
    } catch (e) {
      toast.error(String(e));
      throw e; // re-throw so ProfileForm can also show the inline error
    }
  }

  async function copyKey() {
    if (encryptionKey) {
      await navigator.clipboard.writeText(encryptionKey);
      copied = true;
      setTimeout(() => copied = false, 2000);
    }
  }

  function proceed() {
    goto("/files");
  }
</script>

<div style="min-height: 100vh; overflow-y: auto; display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 1rem;" class="dark:bg-surface-dark">
  <div style="width: 100%; max-width: 40rem;">
    <h1 style="font-size: 2rem; font-weight: 700; text-align: center; margin-bottom: 0.5rem;">Welcome to Vault</h1>
    <p style="font-size: 1.125rem; text-align: center; margin-bottom: 2rem; opacity: 0.6;">Set up your first profile to get started.</p>

    {#if encryptionKey}
      <div style="background: #fffbeb; border: 1px solid #fcd34d; border-radius: 0.5rem; padding: 1.25rem; display: flex; flex-direction: column; gap: 0.75rem;">
        <h2 style="font-size: 1rem; font-weight: 600; color: #92400e; margin: 0;">⚠ Save Your Encryption Key</h2>
        <p style="font-size: 0.875rem; color: #b45309; margin: 0;">
          This key is required to decrypt your files. Store it somewhere safe — it cannot be recovered if lost.
        </p>
        <div style="display: flex; gap: 0.5rem; align-items: stretch;">
          <code style="flex: 1; background: white; border: 1px solid #fcd34d; border-radius: 0.375rem; padding: 0.5rem 0.75rem; font-size: 0.8rem; font-family: monospace; word-break: break-all; user-select: all;">
            {encryptionKey}
          </code>
          <button onclick={copyKey} style="padding: 0.5rem 0.75rem; background: #fde68a; border: none; border-radius: 0.375rem; font-size: 0.875rem; cursor: pointer;">
            {copied ? "Copied!" : "Copy"}
          </button>
        </div>
        <button onclick={proceed} class="btn-primary" style="margin-top: 0.25rem;">
          I've saved my key — Continue to App
        </button>
      </div>
    {:else}
      <ProfileForm onsubmit={handleCreate} submitLabel="Create Profile" />
    {/if}
  </div>
</div>
