/** A timer for a note that goes by itself — and that waits while the pointer
 *  is on it, the way notes do, so it is not taken away mid-read. */
export interface Timer {
  /** Stops the clock (true) or starts it again from what was left (false). */
  hold(on: boolean): void;
  cancel(): void;
}

export function countdown(ms: number, done: () => void): Timer {
  let left = ms;
  let since = performance.now();
  let held = false;
  let timer = setTimeout(done, ms);
  return {
    hold(on) {
      if (on === held) return;
      held = on;
      if (on) {
        clearTimeout(timer);
        left -= performance.now() - since;
      } else {
        since = performance.now();
        timer = setTimeout(done, left);
      }
    },
    cancel() {
      clearTimeout(timer);
    },
  };
}
