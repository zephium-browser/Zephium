import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';

const source = fs.readFileSync(new URL('../src/runtime/idle_callback.js', import.meta.url), 'utf8');
function realm(globals = {}) {
  let clock = 0, next = 0;
  const timers = new Map();
  const context = vm.createContext({
    performance: { now: () => clock },
    setTimeout: (fn, delay) => { timers.set(++next, {fn, delay}); return next; },
    clearTimeout: id => timers.delete(id), ...globals,
  });
  vm.runInContext(source, context);
  return { context, timers, advance: ms => clock += ms };
}

test('idle budget decreases to zero; cancellation and timed-out deadlines', () => {
  const { context: c, timers, advance } = realm();
  let deadline;
  const cancelled = c.requestIdleCallback(() => assert.fail('cancelled callback'));
  c.cancelIdleCallback(cancelled);
  c.cancelIdleCallback(cancelled);
  assert.equal(timers.size, 0);
  c.requestIdleCallback(value => { deadline = value; });
  advance(1);
  timers.get(2).fn();
  assert.equal(deadline.didTimeout, false);
  assert.equal(deadline.timeRemaining(), 50);
  advance(20);
  assert.equal(deadline.timeRemaining(), 30);
  advance(100);
  assert.equal(deadline.timeRemaining(), 0);
  c.requestIdleCallback(value => { deadline = value; }, { timeout: 5 });
  advance(6);
  timers.get(3).fn();
  assert.equal(deadline.didTimeout, true);
  assert.equal(deadline.timeRemaining(), 0);
});

test('native functions survive bootstrap and each world owns its globals', () => {
  const native = () => {};
  for (const name of ['requestIdleCallback', 'cancelIdleCallback']) {
    const { context } = realm({ [name]: native });
    assert.equal(context[name], native);
  }
  const isolated = realm();
  const installed = isolated.context.requestIdleCallback;
  vm.runInContext(source, isolated.context);
  assert.equal(isolated.context.requestIdleCallback, installed);
  assert.equal(vm.runInNewContext('typeof requestIdleCallback'), 'undefined');
});

test('invalid callbacks and timeout conversions fail before scheduling', () => {
  const { context: c, timers } = realm();
  for (const callback of [null, {}, 1]) {
    assert.throws(() => c.requestIdleCallback(callback), /Callback must be a function/);
  }
  assert.throws(() => c.requestIdleCallback(() => {}, { get timeout() { throw Error('getter'); } }), /getter/);
  assert.throws(() => c.requestIdleCallback(() => {}, { timeout: Symbol() }), { name: 'TypeError' });
  assert.equal(timers.size, 0);
});
