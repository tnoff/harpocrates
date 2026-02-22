<script lang="ts">
  interface Props {
    title: string;
    message: string;
    confirmLabel?: string;
    danger?: boolean;
    onconfirm: () => void;
    oncancel: () => void;
  }

  let { title, message, confirmLabel = "Confirm", danger = false, onconfirm, oncancel }: Props = $props();
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="overlay" onclick={oncancel} onkeydown={(e) => e.key === 'Escape' && oncancel()} role="presentation">
  <div
    role="dialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
    tabindex="-1"
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
  >
    <h3 id="confirm-title" class="dialog-title">{title}</h3>
    <p class="dialog-message">{message}</p>
    <div class="btn-row">
      <button onclick={oncancel} class="btn-cancel">Cancel</button>
      <button onclick={onconfirm} class="btn-confirm" class:btn-danger={danger} class:btn-primary={!danger}>
        {confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }

  .dialog {
    background: white;
    border-radius: 0.75rem;
    box-shadow: 0 20px 25px -5px rgb(0 0 0 / 0.15);
    padding: 1.5rem;
    max-width: 28rem;
    width: calc(100% - 2rem);
  }

  .dialog-title {
    font-size: 1.125rem;
    font-weight: 600;
    margin: 0 0 0.5rem;
  }

  .dialog-message {
    font-size: 0.875rem;
    color: #475569;
    margin: 0 0 1rem;
  }

  .btn-row {
    display: flex;
    gap: 0.75rem;
    justify-content: flex-end;
  }

  .btn-cancel, .btn-confirm {
    padding: 0.5rem 1rem;
    border-radius: 0.5rem;
    font-size: 0.875rem;
    border: none;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .btn-cancel {
    background: #f1f5f9;
    color: #334155;
  }

  .btn-cancel:hover { background: #e2e8f0; }

  .btn-primary {
    background: #3b82f6;
    color: white;
    font-weight: 500;
  }

  .btn-primary:hover { background: #2563eb; }

  .btn-danger {
    background: #ef4444;
    color: white;
    font-weight: 500;
  }

  .btn-danger:hover { background: #dc2626; }

  @media (prefers-color-scheme: dark) {
    .dialog { background: #1e293b; color: #f1f5f9; }
    .dialog-message { color: #94a3b8; }
    .btn-cancel { background: #334155; color: #cbd5e1; }
    .btn-cancel:hover { background: #475569; }
  }
</style>
