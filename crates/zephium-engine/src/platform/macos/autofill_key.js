(() => {
  // The focused sign-in field shows WebKit's own AutoFill key, as in Safari.
  // Nothing runs until a field takes focus.
  const named = /user|login|email|account/i;
  const signIn = (input) => {
    const purpose = input.autocomplete.toLowerCase();
    if (/\b(new-password|one-time-code)\b/.test(purpose)) return false;
    if (input.type === "password") return true;
    if (!["text", "email", "tel"].includes(input.type)) return false;
    if (/\b(username|email)\b/.test(purpose)) return true;
    const password = 'input[type="password"]';
    if (input.form) return input.form.querySelector(password) !== null;
    return (
      (input.type === "email" || named.test(input.name) || named.test(input.id)) &&
      input.getRootNode().querySelector(password) !== null
    );
  };
  const target = (event) => event.composedPath()[0];
  addEventListener(
    "focusin",
    (event) => {
      const input = target(event);
      if (globalThis.__zephiumFillingOff) return;
      if (input instanceof HTMLInputElement && signIn(input)) {
        input.autofillButtonType = "credentials";
      }
    },
    true,
  );
  addEventListener(
    "focusout",
    (event) => {
      const input = target(event);
      if (input instanceof HTMLInputElement) input.autofillButtonType = "none";
    },
    true,
  );
})();
