import { invoke } from "@tauri-apps/api/core";

export interface Profile {
  id: number;
  name: string;
  mode: string;
  s3_endpoint: string;
  s3_region: string | null;
  s3_bucket: string;
  extra_env: string | null;
  relative_path: string | null;
  temp_directory: string | null;
  s3_key_prefix: string | null;
  upload_chunk_size_mb: number | null;
  is_active: boolean;
  created_at: string;
}

let activeProfile = $state<Profile | null>(null);
let profiles = $state<Profile[]>([]);
let loading = $state(true);

export const profileStore = {
  get active() { return activeProfile; },
  get profiles() { return profiles; },
  get loading() { return loading; },
  get isReadOnly() { return activeProfile?.mode === "read-only"; },

  async load() {
    loading = true;
    try {
      const [active, all] = await Promise.all([
        invoke<Profile | null>("get_active_profile"),
        invoke<Profile[]>("list_profiles"),
      ]);
      activeProfile = active;
      profiles = all;
    } finally {
      loading = false;
    }
  },

  async switchProfile(id: number) {
    activeProfile = await invoke<Profile>("switch_profile", { profileId: id });
    await this.load();
  },
};
