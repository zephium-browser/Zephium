// Chrome's error surfaces: `runtime.lastError` for the duration of a callback
// with the unchecked warning, invocation errors, and console reports for
// exceptions thrown by listeners and callbacks.
(function errors(Z) {
  'use strict';
  const I = Z.I;

  const describe = (error) => {
    let text;
    try {
      text = I.String(error);
    } catch {
      text = 'Error';
    }
    try {
      const stack = error !== null && typeof error === 'object' ? error.stack : undefined;
      if (typeof stack === 'string' && stack !== '' && I.StringIndexOf(stack, text) !== 0) return `${text}\n${stack}`;
      if (typeof stack === 'string' && stack !== '') return stack;
    } catch {
      // A throwing `stack` getter still leaves the text.
    }
    return text;
  };

  const reportListenerError = (error) => I.consoleError(`Error in event handler: ${describe(error)}`);
  const reportCallbackError = (error) => I.consoleError(`Error handling response: ${describe(error)}`);

  const messageOf = (failure) => {
    if (typeof failure === 'string') return failure;
    if (failure !== null && typeof failure === 'object' && typeof failure.message === 'string') return failure.message;
    return 'An unexpected error occurred';
  };

  const invocation = (name, signature, detail) =>
    new I.TypeError(`Error in invocation of ${name}(${signature}): ${detail}`);

  // One `lastError` slot shared by `runtime.lastError` and
  // `extension.lastError`, set only while a callback runs.
  function createLastError() {
    let current;
    let checked = true;
    const read = () => {
      checked = true;
      return current;
    };
    const run = (message, fn) => {
      const previous = current;
      const previousChecked = checked;
      current = message === null || message === undefined ? undefined : { message };
      checked = current === undefined;
      try {
        return fn();
      } finally {
        if (!checked) I.consoleError(`Unchecked runtime.lastError: ${message}`);
        current = previous;
        checked = previousChecked;
      }
    };
    const install = (target) => {
      I.defineProperty(target, 'lastError', { configurable: true, enumerable: false, get: read, set() {} });
    };
    return { __proto__: null, run, install, read };
  }

  // Calls `fn` with `args` as Chrome calls an API callback: failures go to
  // the console, never to the caller.
  const invokeCallback = (lastError, fn, args, errorMessage) => {
    try {
      lastError.run(errorMessage, () => I.apply(fn, undefined, args));
    } catch (error) {
      reportCallbackError(error);
    }
  };

  return I.freeze({
    __proto__: null,
    describe,
    reportListenerError,
    reportCallbackError,
    messageOf,
    invocation,
    createLastError,
    invokeCallback,
  });
})
