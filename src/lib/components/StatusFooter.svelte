<script lang="ts">
  import { operationsStore } from "$lib/stores/operations.svelte";

  let expanded = $state(false);
  let expandedFiles = $state(new Set<string>());
  let expandedPending = $state(new Set<string>());

  function toggleFiles(id: string) {
    const next = new Set(expandedFiles);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expandedFiles = next;
  }

  function togglePending(id: string) {
    const next = new Set(expandedPending);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expandedPending = next;
  }

  function fmtBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function barPercent(op: { progress?: { current: number; total: number; fileBytesDone?: number; fileBytesTotal?: number; filePhase?: string } }): number {
    if (!op.progress) return 0;
    const { current, total, fileBytesDone, fileBytesTotal } = op.progress;
    if (fileBytesDone !== undefined && fileBytesTotal && fileBytesTotal > 0) {
      return Math.round((fileBytesDone / fileBytesTotal) * 100);
    }
    return total > 0 ? Math.round((current / total) * 100) : 0;
  }

  const activeOp = $derived(operationsStore.list.find((o) => o.status === "running"));
  const pendingOps = $derived(operationsStore.list.filter((o) => o.status === "pending"));
  const finishedOps = $derived(
    operationsStore.list.filter((o) => o.status === "done" || o.status === "error")
  );
</script>

{#if operationsStore.hasAny}
  <div class="footer">
    <!-- Expanded op list — rendered first so it grows upward above the header -->
    {#if expanded}
      <div class="op-list">
        {#each operationsStore.list as op (op.id)}
          <div
            class="op-row"
            class:op-pending={op.status === "pending"}
            class:op-done={op.status === "done"}
            class:op-error={op.status === "error"}
            class:op-cancelling={op.cancelling}
          >
            <span class="op-icon" class:spinning={op.status === "running" && !op.cancelling}>
              {#if op.cancelling}◌{:else if op.status === "pending"}…{:else if op.status === "running"}⟳{:else if op.status === "done"}✓{:else}✕{/if}
            </span>

            <div class="op-body">
              <div class="op-top">
                <span class="op-label">{op.label}</span>
                {#if op.cancelling}
                  <span class="op-count cancelling-text">Cancelling…</span>
                {:else if op.status === "running" && op.progress}
                  <span class="op-count">{op.progress.current} / {op.progress.total}</span>
                {:else if op.status === "pending"}
                  <span class="op-count">Queued</span>
                {/if}
              </div>

              {#if op.status === "running" && op.progress && !op.cancelling}
                <div class="op-progress-row">
                  <div class="progress-track">
                    {#if barPercent(op) > 0}
                      <div class="progress-bar" style="width: {barPercent(op)}%"></div>
                    {:else}
                      <div class="progress-bar indeterminate"></div>
                    {/if}
                  </div>
                  {#if op.progress.filePhase}
                    <span class="op-detail">
                      {op.progress.filePhase}…
                      {#if op.progress.filePhaseDone !== undefined && op.progress.filePhaseTotal}
                        ({fmtBytes(op.progress.filePhaseDone)} / {fmtBytes(op.progress.filePhaseTotal)})
                      {/if}
                    </span>
                  {:else if op.progress.fileBytesDone !== undefined && op.progress.fileBytesTotal}
                    <span class="op-detail">
                      {fmtBytes(op.progress.fileBytesDone)} / {fmtBytes(op.progress.fileBytesTotal)}
                    </span>
                  {:else if op.progress.detail}
                    <span class="op-detail">{op.progress.detail}</span>
                  {/if}
                </div>
              {:else if op.result}
                <span class="op-result">{op.result}</span>
              {/if}

              <!-- Per-op file list toggles -->
              {#if op.files.length > 0 || op.pendingFiles.length > 0}
                <div class="files-toggle-row">
                  {#if op.files.length > 0}
                    <button class="btn-files-toggle" onclick={() => toggleFiles(op.id)}>
                      {expandedFiles.has(op.id) ? "▼" : "▶"} {op.files.length} file{op.files.length !== 1 ? "s" : ""}
                    </button>
                  {/if}
                  {#if op.pendingFiles.length > 0}
                    <button class="btn-files-toggle btn-pending" onclick={() => togglePending(op.id)}>
                      {expandedPending.has(op.id) ? "▼" : "▶"} {op.pendingFiles.length} remaining
                    </button>
                  {/if}
                </div>
                {#if expandedFiles.has(op.id)}
                  {@const displayed = op.files.slice().reverse().slice(0, 100)}
                  <div class="file-list">
                    {#each displayed as file, i (i)}
                      <div class="file-entry" class:file-active={file.status === "active"}>
                        <span class="file-bullet">{file.status === "active" ? "●" : "·"}</span>
                        <span class="file-name">{file.name}</span>
                      </div>
                    {/each}
                    {#if op.files.length > 100}
                      <div class="file-overflow">+{op.files.length - 100} more</div>
                    {/if}
                  </div>
                {/if}
                {#if expandedPending.has(op.id) && op.pendingFiles.length > 0}
                  {@const displayPending = op.pendingFiles.slice(0, 100)}
                  <div class="file-list pending-list">
                    {#each displayPending as name, i (i)}
                      <div class="file-entry">
                        <span class="file-bullet">·</span>
                        <span class="file-name">{name}</span>
                      </div>
                    {/each}
                    {#if op.pendingFiles.length > 100}
                      <div class="file-overflow">+{op.pendingFiles.length - 100} more</div>
                    {/if}
                  </div>
                {/if}
              {/if}
            </div>

            <div class="op-actions">
              {#if !op.cancelling && (op.status === "running" || op.status === "pending")}
                <button class="btn-cancel" onclick={() => operationsStore.cancel(op.id)}>
                  Cancel
                </button>
              {/if}
              {#if op.status === "done" || op.status === "error"}
                <button class="btn-dismiss" onclick={() => operationsStore.dismiss(op.id)}>
                  ×
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}

    <!-- Always-visible header strip -->
    <div class="footer-header">
      <div class="header-summary">
        {#if activeOp}
          <span class="summary-icon spinning">⟳</span>
          <div class="summary-center">
            <div class="summary-top">
              <span class="summary-label">
                {activeOp.cancelling ? "Cancelling…" : activeOp.label}
              </span>
              {#if activeOp.progress && !activeOp.cancelling}
                <span class="summary-progress">
                  {#if activeOp.progress.filePhase}
                    {activeOp.progress.filePhase}…
                    {#if activeOp.progress.filePhaseDone !== undefined && activeOp.progress.filePhaseTotal}
                      ({fmtBytes(activeOp.progress.filePhaseDone)} / {fmtBytes(activeOp.progress.filePhaseTotal)})
                    {/if}
                  {:else if activeOp.progress.fileBytesDone !== undefined && activeOp.progress.fileBytesTotal}
                    {fmtBytes(activeOp.progress.fileBytesDone)} / {fmtBytes(activeOp.progress.fileBytesTotal)}
                  {:else}
                    {activeOp.progress.current}/{activeOp.progress.total}
                  {/if}
                </span>
              {/if}
            </div>
            {#if activeOp.progress && !activeOp.cancelling}
              <div class="header-progress-track">
                {#if barPercent(activeOp) > 0}
                  <div class="header-progress-bar" style="width: {barPercent(activeOp)}%"></div>
                {:else}
                  <div class="header-progress-bar indeterminate"></div>
                {/if}
              </div>
            {/if}
          </div>
        {:else if pendingOps.length > 0}
          <span class="summary-icon muted">…</span>
          <span class="summary-label muted">
            {pendingOps.length} operation{pendingOps.length !== 1 ? "s" : ""} queued
          </span>
        {:else}
          <span class="summary-label muted">
            {finishedOps.length} operation{finishedOps.length !== 1 ? "s" : ""} finished
          </span>
        {/if}
      </div>

      <div class="header-actions">
        {#if pendingOps.length > 0}
          <span class="queued-badge">{pendingOps.length} queued</span>
        {/if}
        {#if finishedOps.length > 0 && !expanded}
          <span class="finished-badge">{finishedOps.length} done</span>
        {/if}
        <button
          class="btn-expand"
          onclick={() => (expanded = !expanded)}
          aria-label={expanded ? "Collapse queue" : "Expand queue"}
        >
          {expanded ? "▼" : "▲"}
        </button>
      </div>
    </div>

  </div>
{/if}

<style>
  .footer {
    border-top: 1px solid #e2e8f0;
    background: #f8fafc;
    flex-shrink: 0;
  }

  /* ── Header strip ───────────────────────────────────────────────────────── */

  .footer-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.375rem 0.75rem;
    min-height: 2.25rem;
  }

  .header-summary {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
    flex: 1;
  }

  .summary-center {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    min-width: 0;
    flex: 1;
  }

  .summary-top {
    display: flex;
    align-items: baseline;
    gap: 0.375rem;
    min-width: 0;
  }

  .header-progress-track {
    height: 3px;
    background: #e2e8f0;
    border-radius: 9999px;
    overflow: hidden;
  }

  .header-progress-bar {
    height: 100%;
    background: #3b82f6;
    border-radius: 9999px;
    transition: width 0.3s ease;
  }

  .summary-icon {
    font-size: 0.8125rem;
    flex-shrink: 0;
    width: 1rem;
    text-align: center;
  }

  .summary-icon.spinning {
    display: inline-block;
    animation: spin 1s linear infinite;
    color: #3b82f6;
  }

  .summary-icon.muted { color: #94a3b8; }

  .summary-label {
    font-size: 0.8125rem;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .summary-label.muted { color: #64748b; font-weight: 400; }

  .summary-progress {
    font-size: 0.75rem;
    color: #64748b;
    flex-shrink: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-shrink: 0;
  }

  .queued-badge {
    font-size: 0.6875rem;
    padding: 0.125rem 0.4rem;
    background: #dbeafe;
    color: #1d4ed8;
    border-radius: 9999px;
    font-weight: 500;
  }

  .finished-badge {
    font-size: 0.6875rem;
    padding: 0.125rem 0.4rem;
    background: #dcfce7;
    color: #15803d;
    border-radius: 9999px;
    font-weight: 500;
  }

  .btn-expand {
    font-size: 0.625rem;
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    border: 1px solid #cbd5e1;
    background: white;
    color: #64748b;
    cursor: pointer;
    line-height: 1;
    transition: background-color 0.1s;
  }

  .btn-expand:hover { background: #f1f5f9; }

  /* ── Expanded op list ───────────────────────────────────────────────────── */

  .op-list {
    border-bottom: 1px solid #e2e8f0;
    max-height: 24rem;
    overflow-y: auto;
  }

  .op-row {
    display: flex;
    align-items: flex-start;
    gap: 0.625rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid #e2e8f0;
  }

  .op-row:last-child { border-bottom: none; }

  .op-row.op-pending { background: #f8fafc; }
  .op-row.op-done { background: #f0fdf4; }
  .op-row.op-error { background: #fef2f2; }
  .op-row.op-cancelling { opacity: 0.65; }

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

  .op-pending .op-icon { color: #94a3b8; }
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

  .op-pending .op-label { color: #64748b; }

  .op-count {
    font-size: 0.75rem;
    color: #64748b;
    flex-shrink: 0;
  }

  .cancelling-text { color: #f59e0b; }

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

  .progress-bar.indeterminate,
  .header-progress-bar.indeterminate {
    width: 40% !important;
    background: #3b82f6;
    animation: indeterminate-slide 1.4s ease-in-out infinite;
  }

  @keyframes indeterminate-slide {
    0% { transform: translateX(-150%); }
    100% { transform: translateX(350%); }
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

  /* ── Per-op file list ───────────────────────────────────────────────────── */

  .files-toggle-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.125rem;
  }

  .btn-files-toggle {
    font-size: 0.6875rem;
    padding: 0.0625rem 0.375rem;
    border-radius: 0.25rem;
    border: 1px solid #e2e8f0;
    background: none;
    color: #64748b;
    cursor: pointer;
    transition: background-color 0.1s, color 0.1s;
    line-height: 1.4;
  }

  .btn-files-toggle:hover {
    background: #e2e8f0;
    color: #334155;
  }

  .btn-files-toggle.btn-pending {
    color: #7c3aed;
    border-color: #ede9fe;
  }

  .btn-files-toggle.btn-pending:hover {
    background: #ede9fe;
    color: #6d28d9;
  }

  .file-list {
    margin-top: 0.125rem;
    max-height: 8rem;
    overflow-y: auto;
    border: 1px solid #e2e8f0;
    border-radius: 0.25rem;
    background: white;
    padding: 0.25rem 0;
  }

  .pending-list {
    border-color: #ede9fe;
    background: #faf5ff;
  }

  .file-entry {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.0625rem 0.5rem;
    font-size: 0.6875rem;
    color: #64748b;
    font-family: monospace;
  }

  .file-entry.file-active {
    color: #1e40af;
    font-weight: 600;
    background: rgb(59 130 246 / 0.06);
  }

  .file-bullet {
    flex-shrink: 0;
    width: 0.75rem;
    text-align: center;
    font-size: 0.5rem;
    font-family: sans-serif;
  }

  .file-entry.file-active .file-bullet {
    font-size: 0.6rem;
    color: #3b82f6;
  }

  .file-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1;
  }

  .file-overflow {
    font-size: 0.6875rem;
    color: #94a3b8;
    padding: 0.125rem 0.5rem;
    font-style: italic;
    font-family: sans-serif;
  }

  /* ── Dark mode ──────────────────────────────────────────────────────────── */
  @media (prefers-color-scheme: dark) {
    .footer { background: #0f172a; border-top-color: #334155; }
    .header-progress-track { background: #334155; }
    .summary-label.muted, .summary-progress { color: #64748b; }
    .queued-badge { background: rgb(29 78 216 / 0.2); color: #93c5fd; }
    .finished-badge { background: rgb(21 128 61 / 0.2); color: #4ade80; }
    .btn-expand { background: #1e293b; border-color: #475569; color: #94a3b8; }
    .btn-expand:hover { background: #334155; }
    .op-list { border-bottom-color: #334155; }
    .op-row { border-bottom-color: #1e293b; }
    .op-row.op-pending { background: #0f172a; }
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
    .btn-files-toggle { border-color: #334155; color: #94a3b8; }
    .btn-files-toggle:hover { background: #334155; color: #cbd5e1; }
    .btn-files-toggle.btn-pending { color: #a78bfa; border-color: rgb(109 40 217 / 0.3); }
    .btn-files-toggle.btn-pending:hover { background: rgb(109 40 217 / 0.15); color: #c4b5fd; }
    .file-list { background: #0f172a; border-color: #334155; }
    .pending-list { background: rgb(109 40 217 / 0.05); border-color: rgb(109 40 217 / 0.25); }
    .file-entry { color: #475569; }
    .file-entry.file-active { color: #93c5fd; background: rgb(59 130 246 / 0.1); }
    .file-overflow { color: #475569; }
  }
</style>
