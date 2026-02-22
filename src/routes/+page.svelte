<script lang="ts">
  import { goto } from "$app/navigation";
  import { profileStore } from "$lib/stores/profile.svelte";

  $effect(() => {
    if (!profileStore.loading) {
      if (profileStore.active) {
        goto("/files");
      } else {
        goto("/setup");
      }
    }
  });
</script>
