export const fullCases = [
  "action-tab-state",
  "alarms-clamp",
  "alarms-create-forms",
  "alarms-order",
  "commands-inventory",
  "i18n-fallback",
  "i18n-placeholders",
  "i18n-predefined",
  "navigation-document",
  "permissions-inventory",
  "permissions-request-gesture",
  "ports-disconnect",
  "runtime-callback-promise",
  "runtime-last-error-unchecked",
  "runtime-response-styles",
  "storage-bytes",
  "storage-get-forms",
  "storage-on-changed",
  "storage-quota",
  "storage-session-access",
  "tabs-lifecycle",
  "tabs-query-permission",
  "tabs-query-redaction",
  "windows-lifecycle",
];

export const omittedCalls = {
  "scripting-register-lifecycle": [
    "existing document is unchanged",
    "new document uses registered script",
    "new document uses updated script",
    "unregistration preserves existing injection",
    "later document has no injection",
  ],
};

const missing = {
  "dnr-block-allow": "Real network interception and no installed DNR backend.",
  "dnr-dynamic-session":
    "Unavailable runtime: no installed DNR registry backend.",
  "dnr-extension-redirect":
    "Real extension resource network redirect and no installed DNR backend.",
  "dnr-matched-permission":
    "Unavailable runtime: no installed DNR feedback backend.",
  "dnr-regex-support": "Unavailable runtime: no installed DNR regex backend.",
  "dnr-upgrade-scheme":
    "Real network scheme upgrade and no installed DNR backend.",
  "injection-all-frames": "Native content injection into a real frame tree.",
  "injection-css": "Native isolated CSS injection and computed page styles.",
  "injection-exclusions-globs": "Native document matching and page injection.",
  "injection-lexical-entries":
    "Native content script lexical environments across entries.",
  "injection-lexical-files":
    "Native content script lexical environments across files.",
  "injection-match-about-blank":
    "Native about:blank/srcdoc frame origins and injection.",
  "injection-match-origin-fallback":
    "Native blob/data frame origins and injection.",
  "injection-run-at":
    "Real parser, document readiness, script worlds and injection ordering.",
  "navigation-fragment":
    "Real location.hash/location.replace execution and native same-document events.",
  "navigation-frames":
    "Real child-frame loading and native parent/child event ordering.",
  "navigation-history":
    "Real page History API execution and native same-document events.",
  "origin-context-csp":
    "Real extension origin, CSP enforcement and page evaluation.",
  "origin-fetch-permissions":
    "Real extension/content fetch, CORS and host authorization.",
  "ports-background-stop":
    "Real background worker teardown and native disconnect delivery.",
  "ports-navigation":
    "Real content document teardown and replacement during navigation.",
  "resources-origin-access":
    "Real extension resource protocol and cross-origin access enforcement.",
  "runtime-sender-frames":
    "Real injected frame principals and native sender metadata.",
  "scripting-execute-errors":
    "Native page execution, promise rejection and file execution.",
  "scripting-execute-frames":
    "Native execution across a real document/frame tree.",
  "scripting-execute-sync-async":
    "Native page function execution and promise settlement.",
  "scripting-execute-worlds":
    "Native MAIN/ISOLATED world separation and page DOM.",
  "tabs-active-tab":
    "Real browser action gesture, content execution and navigation revocation.",
  "tabs-message-frames":
    "Real content injection, frame routing and sender documents.",
  "worker-classic-environment":
    "Real classic service worker environment and import cache lifecycle.",
  "worker-install-activate":
    "Real service worker installation and activation lifecycle.",
  "worker-module-environment":
    "Real module service worker environment and module loading.",
};

export const supportedCases = [
  ...fullCases,
  ...Object.keys(omittedCalls),
].sort();

export function inventory(caseIds) {
  return caseIds.map((id) => {
    if (fullCases.includes(id)) return { id, coverage: "full" };
    if (Object.hasOwn(omittedCalls, id))
      return {
        id,
        coverage: "partial",
        reason:
          "Registration metadata only; native page injection remains unexecuted.",
        omittedCalls: omittedCalls[id],
      };
    if (!Object.hasOwn(missing, id))
      throw new Error(`Unclassified Node case ${id}`);
    return { id, coverage: "unavailable", reason: missing[id] };
  });
}

// Projection is explicit, label based, and checked against the immutable oracle.
export function projectReference(reference, omitted) {
  const labels = new Set(
    omitted.flatMap((label) => [`${label}:call`, `${label}:result`]),
  );
  for (const label of labels) {
    if (
      reference.observations.filter((row) => row.label === label).length !== 1
    )
      throw new Error(
        `Missing or duplicate projected reference observation ${label}`,
      );
  }
  return {
    ...reference,
    observations: reference.observations.filter(
      (row) => !labels.has(row.label),
    ),
  };
}
