import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toast } from './toast.svelte';

beforeEach(() => {
  vi.useRealTimers();
  // Dismiss all existing toasts for a clean slate.
  [...toast.items].forEach((t) => toast.dismiss(t.id));
});

describe('toastStore', () => {
  it('success() creates a success toast', () => {
    toast.success('done');
    expect(toast.items).toHaveLength(1);
    expect(toast.items[0].type).toBe('success');
    expect(toast.items[0].message).toBe('done');
  });

  it('error() creates an error toast', () => {
    toast.error('oops');
    expect(toast.items[0].type).toBe('error');
  });

  it('warning() creates a warning toast', () => {
    toast.warning('watch out');
    expect(toast.items[0].type).toBe('warning');
  });

  it('info() creates an info toast', () => {
    toast.info('fyi');
    expect(toast.items[0].type).toBe('info');
  });

  it('each toast gets a unique id', () => {
    toast.info('a');
    toast.info('b');
    toast.info('c');
    const ids = toast.items.map((t) => t.id);
    expect(new Set(ids).size).toBe(3);
  });

  it('dismiss(id) removes only that toast', () => {
    toast.info('first');
    toast.info('second');
    const [first, second] = toast.items;
    toast.dismiss(first.id);
    expect(toast.items).toHaveLength(1);
    expect(toast.items[0].id).toBe(second.id);
  });

  it('success auto-dismisses after 3500 ms', () => {
    vi.useFakeTimers();
    toast.success('hi');
    expect(toast.items).toHaveLength(1);
    vi.advanceTimersByTime(3499);
    expect(toast.items).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(toast.items).toHaveLength(0);
  });

  it('error auto-dismisses after 6000 ms', () => {
    vi.useFakeTimers();
    toast.error('bad');
    vi.advanceTimersByTime(5999);
    expect(toast.items).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(toast.items).toHaveLength(0);
  });

  it('warning auto-dismisses after 5000 ms', () => {
    vi.useFakeTimers();
    toast.warning('careful');
    vi.advanceTimersByTime(4999);
    expect(toast.items).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(toast.items).toHaveLength(0);
  });

  it('multiple toasts have independent timers', () => {
    vi.useFakeTimers();
    toast.success('quick');
    toast.error('slow');
    expect(toast.items).toHaveLength(2);
    vi.advanceTimersByTime(3500);
    expect(toast.items).toHaveLength(1);
    expect(toast.items[0].type).toBe('error');
  });

  it('info auto-dismisses after 3500 ms', () => {
    vi.useFakeTimers();
    toast.info('fyi');
    vi.advanceTimersByTime(3499);
    expect(toast.items).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(toast.items).toHaveLength(0);
  });

  it('dismiss with unknown id is a no-op', () => {
    toast.info('hello');
    toast.dismiss(99999);
    expect(toast.items).toHaveLength(1);
  });
});
