<script lang="ts">
  import { toast } from "$lib/stores/toast.svelte";

  const ICONS: Record<string, string> = {
    success: "✓",
    error: "✕",
    warning: "⚠",
    info: "ℹ",
  };
</script>

<div class="container">
  {#each toast.items as t (t.id)}
    <div class="toast toast-{t.type}">
      <span class="toast-icon">{ICONS[t.type]}</span>
      <span class="toast-message">{t.message}</span>
      <button onclick={() => toast.dismiss(t.id)} class="toast-dismiss" aria-label="Dismiss">
        &times;
      </button>
    </div>
  {/each}
</div>

<style>
  .container {
    position: fixed;
    bottom: 1rem;
    right: 1rem;
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-width: 24rem;
    width: 100%;
    pointer-events: none;
  }

  .toast {
    pointer-events: auto;
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    border-radius: 0.5rem;
    box-shadow: 0 4px 12px rgb(0 0 0 / 0.15);
    color: white;
    font-size: 0.875rem;
    line-height: 1.4;
  }

  .toast-success { background: #16a34a; }
  .toast-error   { background: #dc2626; }
  .toast-warning { background: #d97706; }
  .toast-info    { background: #2563eb; }

  .toast-icon {
    font-weight: 700;
    flex-shrink: 0;
    margin-top: 1px;
  }

  .toast-message { flex: 1; }

  .toast-dismiss {
    flex-shrink: 0;
    background: none;
    border: none;
    color: white;
    opacity: 0.7;
    cursor: pointer;
    font-size: 1.125rem;
    line-height: 1;
    margin-top: -1px;
    padding: 0;
  }

  .toast-dismiss:hover { opacity: 1; }
</style>
