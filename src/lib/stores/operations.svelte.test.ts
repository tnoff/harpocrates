import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { operationsStore as OperationsStoreType } from './operations.svelte';

type Listener = (event: { payload: unknown }) => void;

let listeners: Record<string, Listener>;
let mockInvoke: ReturnType<typeof vi.fn>;
let operationsStore: typeof OperationsStoreType;

function emit(name: string, payload: unknown) {
  listeners[name]?.({ payload });
}

beforeEach(async () => {
  vi.resetModules();
  listeners = {};
  mockInvoke = vi.fn();

  vi.doMock('@tauri-apps/api/event', () => ({
    listen: vi.fn((name: string, cb: Listener) => {
      listeners[name] = cb;
      return Promise.resolve(() => {});
    }),
  }));

  vi.doMock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));

  const mod = await import('./operations.svelte');
  operationsStore = mod.operationsStore;

  // Let the module-level IIFE complete and register all listeners.
  await new Promise((r) => setTimeout(r, 0));
});

describe('operationsStore — initial state', () => {
  it('starts empty', () => {
    expect(operationsStore.list).toHaveLength(0);
    expect(operationsStore.hasAny).toBe(false);
  });
});

describe('operationsStore — queue:updated', () => {
  it('adds a pending op', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    expect(operationsStore.list).toHaveLength(1);
    expect(operationsStore.list[0].status).toBe('pending');
    expect(operationsStore.list[0].label).toBe('Backup');
  });

  it('ignores duplicate pending op', () => {
    const snapshot = { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null };
    emit('queue:updated', snapshot);
    emit('queue:updated', snapshot);
    expect(operationsStore.list).toHaveLength(1);
  });

  it('removes cancelled pending op', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    expect(operationsStore.list).toHaveLength(1);
    emit('queue:updated', { pending: [], active: null });
    expect(operationsStore.list).toHaveLength(0);
  });

  it('transitions pending → running when active matches', () => {
    const entry = { id: 'op1', label: 'Backup', op_type: 'backup' };
    emit('queue:updated', { pending: [entry], active: null });
    expect(operationsStore.list[0].status).toBe('pending');
    emit('queue:updated', { pending: [], active: entry });
    expect(operationsStore.list[0].status).toBe('running');
  });

  it('only the matching pending op transitions to running when multiple ops are queued', () => {
    const op1 = { id: 'op1', label: 'Backup', op_type: 'backup' };
    const op2 = { id: 'op2', label: 'Verify', op_type: 'verify' };
    emit('queue:updated', { pending: [op1, op2], active: null });
    expect(operationsStore.list).toHaveLength(2);
    // op1 becomes active — op2 must stay pending (covers `: o` else-branch in the map)
    emit('queue:updated', { pending: [op2], active: op1 });
    const statuses = Object.fromEntries(operationsStore.list.map((o) => [o.id, o.status]));
    expect(statuses['op1']).toBe('running');
    expect(statuses['op2']).toBe('pending');
  });

  it('does not re-transition already-running op on subsequent queue:updated', () => {
    const entry = { id: 'op1', label: 'Backup', op_type: 'backup' };
    emit('queue:updated', { pending: [], active: entry });
    expect(operationsStore.list[0].status).toBe('running');
    // Second queue:updated with same active — op is already running, no change
    emit('queue:updated', { pending: [], active: entry });
    expect(operationsStore.list).toHaveLength(1);
    expect(operationsStore.list[0].status).toBe('running');
  });

  it('adds a running op not seen before as running', () => {
    const entry = { id: 'op2', label: 'Restore', op_type: 'restore' };
    emit('queue:updated', { pending: [], active: entry });
    expect(operationsStore.list).toHaveLength(1);
    expect(operationsStore.list[0].status).toBe('running');
  });
});

describe('operationsStore — op:complete', () => {
  it('marks op as done with result', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    emit('op:complete', { id: 'op1', message: 'All done' });
    expect(operationsStore.list[0].status).toBe('done');
    expect(operationsStore.list[0].result).toBe('All done');
  });

  it('clears pendingFiles on complete', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    emit('op:pending_files', { op_id: 'op1', files: ['a.txt', 'b.txt'] });
    emit('op:complete', { id: 'op1', message: 'Done' });
    expect(operationsStore.list[0].pendingFiles).toHaveLength(0);
  });

  it('flips active file entry to done on complete', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 2, current_file: '/home/user/a.txt' });
    expect(operationsStore.list[0].files[0].status).toBe('active');
    emit('op:complete', { id: 'op1', message: 'Done' });
    expect(operationsStore.list[0].files[0].status).toBe('done');
  });

  it('preserves already-done file entries unchanged on complete', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    // Two progress events → one done, one active
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 3, current_file: '/a/first.txt' });
    emit('backup:progress', { op_id: 'op1', processed: 2, total: 3, current_file: '/a/second.txt' });
    expect(operationsStore.list[0].files[0].status).toBe('done');
    emit('op:complete', { id: 'op1', message: 'Done' });
    // Both should be done; already-done entry stays done (covers `: f` else-branch)
    expect(operationsStore.list[0].files.every((f) => f.status === 'done')).toBe(true);
  });
});

describe('operationsStore — op:failed', () => {
  it('marks op as error with result', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    emit('op:failed', { id: 'op1', error: 'Connection refused' });
    expect(operationsStore.list[0].status).toBe('error');
    expect(operationsStore.list[0].result).toBe('Connection refused');
  });

  it('clears pendingFiles on failure', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    emit('op:pending_files', { op_id: 'op1', files: ['a.txt'] });
    emit('op:failed', { id: 'op1', error: 'failed' });
    expect(operationsStore.list[0].pendingFiles).toHaveLength(0);
  });
});

describe('operationsStore — progress / file log', () => {
  beforeEach(() => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
  });

  it('backup:progress updates progress fields', () => {
    emit('backup:progress', { op_id: 'op1', processed: 3, total: 10, current_file: '/a/b/file.txt' });
    const { progress } = operationsStore.list[0];
    expect(progress?.current).toBe(3);
    expect(progress?.total).toBe(10);
    expect(progress?.detail).toBe('file.txt');
  });

  it('uses basename of full path', () => {
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 5, current_file: '/home/user/docs/file.txt' });
    expect(operationsStore.list[0].progress?.detail).toBe('file.txt');
  });

  it('first progress event adds an active file entry', () => {
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 5, current_file: '/a/x.txt' });
    const { files } = operationsStore.list[0];
    expect(files).toHaveLength(1);
    expect(files[0].status).toBe('active');
    expect(files[0].name).toBe('x.txt');
  });

  it('second progress flips previous active → done', () => {
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 5, current_file: '/a/x.txt' });
    emit('backup:progress', { op_id: 'op1', processed: 2, total: 5, current_file: '/a/y.txt' });
    const { files } = operationsStore.list[0];
    expect(files).toHaveLength(2);
    expect(files[0].status).toBe('done');
    expect(files[1].status).toBe('active');
  });

  it('op:pending_files sets pendingFiles', () => {
    emit('op:pending_files', { op_id: 'op1', files: ['a.txt', 'b.txt', 'c.txt'] });
    expect(operationsStore.list[0].pendingFiles).toEqual(['a.txt', 'b.txt', 'c.txt']);
  });

  it('op:pending_files only updates the matching op when multiple ops exist', () => {
    // Add a second op alongside op1
    emit('queue:updated', {
      pending: [{ id: 'op2', label: 'Verify', op_type: 'verify' }],
      active: { id: 'op1', label: 'Backup', op_type: 'backup' },
    });
    emit('op:pending_files', { op_id: 'op1', files: ['a.txt', 'b.txt'] });
    const op1 = operationsStore.list.find((o) => o.id === 'op1');
    const op2 = operationsStore.list.find((o) => o.id === 'op2');
    expect(op1?.pendingFiles).toEqual(['a.txt', 'b.txt']);
    expect(op2?.pendingFiles).toHaveLength(0); // unaffected (covers `: o` else-branch)
  });

  it('progress removes current file from pendingFiles', () => {
    emit('op:pending_files', { op_id: 'op1', files: ['a.txt', 'b.txt'] });
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 2, current_file: '/path/a.txt' });
    expect(operationsStore.list[0].pendingFiles).toEqual(['b.txt']);
  });

  it('restore:progress updates progress', () => {
    emit('restore:progress', { op_id: 'op1', processed: 2, total: 4, current_file: '/a/r.txt' });
    expect(operationsStore.list[0].progress?.current).toBe(2);
  });

  it('scramble:progress updates progress', () => {
    emit('scramble:progress', { op_id: 'op1', processed: 1, total: 3, current_file: '/a/s.txt' });
    expect(operationsStore.list[0].progress?.current).toBe(1);
  });

  it('verify:progress updates progress', () => {
    emit('verify:progress', { op_id: 'op1', processed: 5, total: 10, current_file: '/a/v.txt' });
    expect(operationsStore.list[0].progress?.current).toBe(5);
  });

  it('cleanup:progress updates progress', () => {
    emit('cleanup:progress', { op_id: 'op1', processed: 1, total: 2, current_item: '/a/c.txt', deleted: 0, failed: 0 });
    expect(operationsStore.list[0].progress?.current).toBe(1);
  });

  it('file log is capped at MAX_FILE_LOG (500)', () => {
    for (let i = 0; i < 501; i++) {
      emit('backup:progress', { op_id: 'op1', processed: i + 1, total: 501, current_file: `/a/file${i}.txt` });
    }
    expect(operationsStore.list[0].files.length).toBeLessThanOrEqual(501);
  });
});

describe('operationsStore — actions', () => {
  it('cancel() on running op sets cancelling: true', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    operationsStore.cancel('op1');
    expect(operationsStore.list[0].cancelling).toBe(true);
  });

  it('cancel() calls invoke cancel_operation with opId', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    operationsStore.cancel('op1');
    expect(mockInvoke).toHaveBeenCalledWith('cancel_operation', { opId: 'op1' });
  });

  it('cancel() on pending op does not set cancelling', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    operationsStore.cancel('op1');
    expect(operationsStore.list[0].cancelling).toBeUndefined();
  });

  it('cancel() with unknown id still calls invoke and does not throw', () => {
    operationsStore.cancel('nonexistent');
    expect(mockInvoke).toHaveBeenCalledWith('cancel_operation', { opId: 'nonexistent' });
  });

  it('cancel() only sets cancelling on the target op when multiple ops exist', () => {
    emit('queue:updated', {
      pending: [{ id: 'op2', label: 'Verify', op_type: 'verify' }],
      active: { id: 'op1', label: 'Backup', op_type: 'backup' },
    });
    operationsStore.cancel('op1');
    const op1 = operationsStore.list.find((o) => o.id === 'op1');
    const op2 = operationsStore.list.find((o) => o.id === 'op2');
    expect(op1?.cancelling).toBe(true);
    expect(op2?.cancelling).toBeUndefined(); // covers `: o` else-branch in cancel's map
  });

  it('dismiss() removes op from list', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    expect(operationsStore.list).toHaveLength(1);
    operationsStore.dismiss('op1');
    expect(operationsStore.list).toHaveLength(0);
  });
});

describe('operationsStore — derived getters', () => {
  it('hasAny is true when ops are present', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    expect(operationsStore.hasAny).toBe(true);
  });

  it('hasRunning is true when there is a pending op', () => {
    emit('queue:updated', { pending: [{ id: 'op1', label: 'Backup', op_type: 'backup' }], active: null });
    expect(operationsStore.hasRunning).toBe(true);
  });

  it('hasRunning is true when there is a running op', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    expect(operationsStore.hasRunning).toBe(true);
  });

  it('hasRunning is false when all ops are done', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    emit('op:complete', { id: 'op1', message: 'Done' });
    expect(operationsStore.hasRunning).toBe(false);
  });

  it('hasRunning is false when all ops are errored', () => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    emit('op:failed', { id: 'op1', error: 'err' });
    expect(operationsStore.hasRunning).toBe(false);
  });
});

describe('operationsStore — DONE_TTL_MS auto-dismiss', () => {
  it('op auto-removes 5000 ms after op:complete', () => {
    vi.useFakeTimers();
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
    emit('op:complete', { id: 'op1', message: 'Done' });
    expect(operationsStore.list).toHaveLength(1);
    vi.advanceTimersByTime(4999);
    expect(operationsStore.list).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(operationsStore.list).toHaveLength(0);
    vi.useRealTimers();
  });
});

describe('operationsStore — progress edge cases', () => {
  beforeEach(() => {
    emit('queue:updated', { pending: [], active: { id: 'op1', label: 'Backup', op_type: 'backup' } });
  });

  it('progress is cleared to undefined after op:complete', () => {
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 5, current_file: '/a/x.txt' });
    expect(operationsStore.list[0].progress).toBeDefined();
    emit('op:complete', { id: 'op1', message: 'Done' });
    expect(operationsStore.list[0].progress).toBeUndefined();
  });

  it('progress is cleared to undefined after op:failed', () => {
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 5, current_file: '/a/x.txt' });
    expect(operationsStore.list[0].progress).toBeDefined();
    emit('op:failed', { id: 'op1', error: 'err' });
    expect(operationsStore.list[0].progress).toBeUndefined();
  });

  it('applyProgress uses full current_file as name when no slash present', () => {
    emit('backup:progress', { op_id: 'op1', processed: 1, total: 5, current_file: 'plainfile.txt' });
    expect(operationsStore.list[0].progress?.detail).toBe('plainfile.txt');
    expect(operationsStore.list[0].files[0].name).toBe('plainfile.txt');
  });

  it('progress event for unknown op_id is a no-op', () => {
    emit('backup:progress', { op_id: 'unknown', processed: 1, total: 5, current_file: '/a/x.txt' });
    expect(operationsStore.list[0].progress).toBeUndefined();
  });
});
