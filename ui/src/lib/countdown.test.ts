import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { countdown } from './countdown';

describe('countdown', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('ends after its time', () => {
    const done = vi.fn();
    countdown(3000, done);
    vi.advanceTimersByTime(2999);
    expect(done).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(done).toHaveBeenCalledOnce();
  });

  it('waits while held, then finishes what was left', () => {
    const done = vi.fn();
    const c = countdown(3000, done);
    vi.advanceTimersByTime(1000);
    c.hold(true);
    vi.advanceTimersByTime(10_000);
    expect(done).not.toHaveBeenCalled();
    c.hold(false);
    vi.advanceTimersByTime(1999);
    expect(done).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(done).toHaveBeenCalledOnce();
  });

  it('ignores holding twice, and a cancel is final', () => {
    const done = vi.fn();
    const c = countdown(1000, done);
    c.hold(true);
    c.hold(true);
    c.hold(false);
    c.cancel();
    vi.advanceTimersByTime(5000);
    expect(done).not.toHaveBeenCalled();
  });
});
