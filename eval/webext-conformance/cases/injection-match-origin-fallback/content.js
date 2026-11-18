const fallbackKind =
  self === top
    ? "top"
    : location.protocol === "data:"
      ? "data"
      : location.protocol === "blob:"
        ? "blob"
        : location.href === "about:srcdoc"
          ? "srcdoc"
          : location.href === "about:blank"
            ? "blank"
            : new URL(location.href).searchParams.get("kind");
chrome.runtime.sendMessage({
  probe: "origin-fallback",
  kind: fallbackKind,
  protocol: location.protocol,
});
