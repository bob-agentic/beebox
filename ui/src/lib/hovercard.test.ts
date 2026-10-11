// The hover card's triggers: a resting mouse, a holding finger. Timing and
// who-gets-the-long-press are the parts that broke in testing, so they are
// pinned down here.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { card, hideCard, hovercard, tildify, type CardOptions } from './hovercard.svelte';

function row(opts: Partial<CardOptions> = {}) {
  const el = document.createElement('div');
  el.innerHTML = '<span class="name">name</span><span class="x">x</span>';
  document.body.replaceChildren(el);
  const action = hovercard(el, {
    content: () => ({ title: 'name', rows: [] }),
    side: 'right',
    ...opts,
  });
  return { el, name: el.querySelector('.name')!, x: el.querySelector('.x')!, action };
}

function ptr(type: string, pointerType: string, x = 0, y = 0, buttons = 0) {
  // jsdom's PointerEvent drops pointerType, so it is set by hand.
  const e = new MouseEvent(type, { bubbles: true, clientX: x, clientY: y, buttons, cancelable: true });
  Object.defineProperty(e, 'pointerType', { value: pointerType });
  return e;
}

beforeEach(() => {
  vi.useFakeTimers();
  hideCard();
});
afterEach(() => vi.useRealTimers());

describe('mouse', () => {
  it('opens after resting, closes on leave', () => {
    const { el, name } = row();
    name.dispatchEvent(ptr('pointermove', 'mouse'));
    vi.advanceTimersByTime(300);
    expect(card.open).toBeNull();
    vi.advanceTimersByTime(100);
    expect(card.open?.owner).toBe(el);
    el.dispatchEvent(ptr('pointerleave', 'mouse'));
    expect(card.open).toBeNull();
  });

  it('stays shut after a click until the mouse leaves', () => {
    const { el, name } = row();
    name.dispatchEvent(ptr('pointerdown', 'mouse'));
    name.dispatchEvent(ptr('pointermove', 'mouse'));
    vi.advanceTimersByTime(1000);
    expect(card.open).toBeNull();
    el.dispatchEvent(ptr('pointerleave', 'mouse'));
    name.dispatchEvent(ptr('pointermove', 'mouse'));
    vi.advanceTimersByTime(400);
    expect(card.open?.owner).toBe(el);
  });

  it('ignores the parts that have their own job', () => {
    const { x } = row({ ignore: '.x' });
    x.dispatchEvent(ptr('pointermove', 'mouse'));
    vi.advanceTimersByTime(1000);
    expect(card.open).toBeNull();
  });
});

describe('touch', () => {
  it('opens on the long-press, below, and swallows the click that follows', () => {
    const { el, name } = row();
    const clicked = vi.fn();
    el.addEventListener('click', clicked);
    name.dispatchEvent(ptr('pointerdown', 'touch'));
    name.dispatchEvent(ptr('contextmenu', 'touch'));
    expect(card.open?.side).toBe('below');
    name.dispatchEvent(ptr('pointerup', 'touch'));
    name.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(clicked).not.toHaveBeenCalled();
    // The next tap is a real one.
    vi.advanceTimersByTime(400);
    name.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(clicked).toHaveBeenCalledOnce();
  });

  it('falls back to a timer where there is no long-press event', () => {
    const { el, name } = row();
    name.dispatchEvent(ptr('pointerdown', 'touch'));
    vi.advanceTimersByTime(700);
    expect(card.open?.owner).toBe(el);
  });

  it('leaves the long-press to a row that has its own menu', () => {
    const { name } = row({ menu: () => true });
    name.dispatchEvent(ptr('pointerdown', 'touch'));
    name.dispatchEvent(ptr('contextmenu', 'touch'));
    vi.advanceTimersByTime(1000);
    expect(card.open).toBeNull();
  });

  it('gives up once the finger moves as far as a drag', () => {
    const { name } = row();
    name.dispatchEvent(ptr('pointerdown', 'touch', 0, 0));
    name.dispatchEvent(ptr('pointermove', 'touch', 5, 0));
    name.dispatchEvent(ptr('contextmenu', 'touch'));
    vi.advanceTimersByTime(1000);
    expect(card.open).toBeNull();
  });

  it('does nothing for a quick tap', () => {
    const { name } = row();
    name.dispatchEvent(ptr('pointerdown', 'touch'));
    vi.advanceTimersByTime(100);
    name.dispatchEvent(ptr('pointerup', 'touch'));
    vi.advanceTimersByTime(1000);
    expect(card.open).toBeNull();
  });
});

it('shortens a home folder to ~', () => {
  expect(tildify('/Users/bob/repo/x')).toBe('~/repo/x');
  expect(tildify('/home/bob')).toBe('~');
  expect(tildify('/tmp/x')).toBe('/tmp/x');
});
