import assert from 'node:assert/strict';
import { test } from 'node:test';
import { extensionContext } from './test/harness.mjs';

test('ordinary event errors use Chromium EventEmitter and gin wording', () => {
  const { chrome, host } = extensionContext('background', { permissions: ['webNavigation'] });
  const event = chrome.runtime.onInstalled;
  assert.throws(() => event.addListener.call({}, () => {}), {
    name: 'TypeError',
    message: 'Illegal invocation: Function must be called on an object of type Event',
  });
  event.addListener('not a callback');
  assert.equal(event.hasListeners(), false);
  assert.equal(host.callsTo('zephium.addListener').length, 0);
  assert.throws(() => event.addListener(() => {}, {}), {
    name: 'TypeError', message: 'This event does not support filters',
  });
  const navigation = chrome.webNavigation.onCommitted;
  assert.throws(() => navigation.addListener(() => {}, null), {
    name: 'TypeError', message: 'Invalid invocation',
  });
  assert.throws(() => navigation.addListener(() => {}, { url: 'bad' }), {
    name: 'TypeError', message: 'filters.url should be an array.',
  });
  assert.throws(() => navigation.addListener(() => {}, { url: [], serviceType: 4 }), {
    name: 'TypeError', message: 'filters.serviceType should be a string.',
  });
  assert.throws(() => navigation.addListener(() => {}, { url: [] }, []), {
    name: 'TypeError', message: 'This event does not support options',
  });
  assert.equal(host.callsTo('zephium.addListener').length, 0);
});

test('webRequest custom invocation names and signatures exclude the callback', () => {
  const { chrome } = extensionContext('background', { permissions: ['webRequest'] });
  const event = chrome.webRequest.onAuthRequired;
  const prefix = 'Error in invocation of webRequest.onAuthRequired(webRequest.RequestFilter filter, optional array extraInfoSpec): ';
  assert.throws(() => event.addListener(() => {}), {
    name: 'TypeError', message: prefix + 'No matching signature.',
  });
  assert.throws(() => event.addListener(() => {}, {}), {
    name: 'TypeError', message: prefix + "Error at parameter 'filter': Missing required property 'urls'.",
  });
  assert.throws(() => event.addListener(() => {}, { urls: 4 }), {
    name: 'TypeError', message: prefix + "Error at parameter 'filter': Error at property 'urls': Invalid type: expected array, found integer.",
  });
  assert.throws(() => event.addListener(() => {}, { urls: [false] }), {
    name: 'TypeError', message: prefix + "Error at parameter 'filter': Error at property 'urls': Error at index 0: Invalid type: expected string, found boolean.",
  });
});

test('port receiver checks use the native Port type name', () => {
  const { chrome } = extensionContext();
  const port = chrome.runtime.connect();
  for (const method of ['postMessage', 'disconnect']) {
    assert.throws(() => port[method].call({}, 'message'), {
      name: 'TypeError',
      message: 'Illegal invocation: Function must be called on an object of type Port',
    });
  }
});
