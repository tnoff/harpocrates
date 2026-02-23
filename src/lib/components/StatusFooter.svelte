<script lang="ts">
  import { operationsStore } from "$lib/stores/operations.svelte";
</script>

{#if operationsStore.hasAny}
  <div class="footer">
    {#each operationsStore.list as op (op.id)}
      <div class="op-row" class:op-done={op.status === "done"} class:op-error={op.status === "error"}>
        <span class="op-icon" class:spinning={op.status === "running"}>
          {#if op.status === "running"}⟳{:else if op.status === "done"}✓{:else}✕{/if}
        </span>

        <div class="op-body">
          <div class="op-top">
            <span class="op-label">{op.label}</span>
            {#if op.status === "running" && op.progress}
              <span class="op-count">{op.progress.current} / {op.progress.total}</span>
            {/if}
          </div>

          {#if op.status === "running" && op.progress}
            <div class="op-progress-row">
              <div class="progress-track">
                <div
                  class="progress-bar"
                  style="width: {op.progress.total > 0
                    ? Math.round((op.progress.current / op.progress.total) * 100)
                    : 0}%"
                ></div>
              </div>
              {#if op.progress.detail}
                <span class="op-detail">{op.progress.detail}</span>
              {/if}
            </div>
          {:else if op.result}
            <span class="op-result">{op.result}</span>
          {/if}
        </div>

        <div class="op-actions">
          {#if op.status === "running" && op.oncancel}
            <button class="btn-cancel" onclick={op.oncancel}>Cancel</button>
          {/if}
          {#if op.status !== "running"}
            <button class="btn-dismiss" onclick={() => operationsStore.dismiss(op.id)}>×</button>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .footer {
    border-top: 1px solid #e2e8f0;
    background: #f8fafc;
    max-height: 12rem;
    overflow-y: auto;
    flex-shrink: 0;
  }

  .op-row {
    display: flex;
    align-items: flex-start;
    gap: 0.625rem;
    padding: 0.625rem 1rem;
    border-bottom: 1px solid #e2e8f0;
  }

  .op-row:last-child {
    border-bottom: none;
  }

  .op-row.op-done { background: #f0fdf4; }
  .op-row.op-error { background: #fef2f2; }

  .op-icon {
    font-size: 0.875rem;
    flex-shrink: 0;
    margin-top: 0.1rem;
    width: 1rem;
    text-align: center;
  }

  .op-icon.spinning {
    display: inline-block;
    animation: spin 1s linear infinite;
    color: #3b82f6;
  }

  .op-done .op-icon { color: #22c55e; }
  .op-error .op-icon { color: #ef4444; }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .op-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .op-top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
    min-width: 0;
  }

  .op-label {
    font-size: 0.8125rem;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .op-count {
    font-size: 0.75rem;
    color: #64748b;
    flex-shrink: 0;
  }

  .op-progress-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
  }

  .progress-track {
    flex: 1;
    height: 4px;
    background: #e2e8f0;
    border-radius: 9999px;
    overflow: hidden;
  }

  .progress-bar {
    height: 100%;
    background: #3b82f6;
    border-radius: 9999px;
    transition: width 0.2s ease;
  }

  .op-detail {
    font-size: 0.7rem;
    color: #64748b;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 12rem;
    flex-shrink: 0;
  }

  .op-result {
    font-size: 0.75rem;
    color: #64748b;
    word-break: break-word;
    white-space: pre-wrap;
  }

  .op-error .op-result { color: #ef4444; }
  .op-done .op-result { color: #16a34a; }

  .op-actions {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-shrink: 0;
  }

  .btn-cancel {
    font-size: 0.75rem;
    padding: 0.125rem 0.5rem;
    border-radius: 0.25rem;
    border: 1px solid #cbd5e1;
    background: white;
    color: #475569;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .btn-cancel:hover { background: #f1f5f9; }

  .btn-dismiss {
    font-size: 1rem;
    line-height: 1;
    width: 1.25rem;
    height: 1.25rem;
    border-radius: 0.25rem;
    border: none;
    background: none;
    color: #94a3b8;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: color 0.1s, background-color 0.1s;
  }

  .btn-dismiss:hover { color: #475569; background: #e2e8f0; }

  @media (prefers-color-scheme: dark) {
    .footer { background: #0f172a; border-top-color: #334155; }
    .op-row { border-bottom-color: #1e293b; }
    .op-row.op-done { background: rgb(21 128 61 / 0.1); }
    .op-row.op-error { background: rgb(185 28 28 / 0.1); }
    .op-count, .op-detail { color: #94a3b8; }
    .op-result { color: #94a3b8; }
    .op-done .op-result { color: #4ade80; }
    .op-error .op-result { color: #f87171; }
    .progress-track { background: #334155; }
    .btn-cancel { background: #1e293b; border-color: #475569; color: #94a3b8; }
    .btn-cancel:hover { background: #334155; }
    .btn-dismiss { color: #64748b; }
    .btn-dismiss:hover { color: #cbd5e1; background: #334155; }
  }
</style>
