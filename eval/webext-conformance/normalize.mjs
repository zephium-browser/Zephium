const identityKeys = {
  tabId: "tab",
  windowId: "window",
  openerTabId: "tab",
  frameId: "frame",
  parentFrameId: "frame",
  documentId: "document",
  parentDocumentId: "document",
  requestId: "request",
  processId: "process",
};
export function normalize(
  value,
  { origins = {}, extensionId = "", profile = "" } = {},
) {
  const maps = new Map();
  function identity(kind, value) {
    if (typeof value === "number" && value < 0) return value;
    if (kind === "frame" && value === 0) return 0;
    if (!maps.has(kind)) maps.set(kind, new Map());
    const ids = maps.get(kind);
    if (!ids.has(value)) ids.set(value, `<${kind}:${ids.size + 1}>`);
    return ids.get(value);
  }
  function visit(value, key = "", parent = {}) {
    if (value === null || value === undefined) return value;
    if (identityKeys[key] && ["string", "number"].includes(typeof value))
      return identity(identityKeys[key], value);
    if (key === "id" && typeof value === "number") {
      if ("index" in parent && "windowId" in parent)
        return identity("tab", value);
      if ("focused" in parent && "type" in parent)
        return identity("window", value);
    }
    if (
      typeof value === "number" &&
      ["timeStamp", "timestamp", "scheduledTime", "lastAccessed"].includes(key)
    )
      return "<time>";
    if (typeof value === "string") {
      let result = value;
      for (const [name, origin] of Object.entries(origins)) {
        result = result.replaceAll(origin, `<origin:${name}>`);
        result = result.replaceAll(
          origin.replace("http:", "https:"),
          `<origin:${name}:https>`,
        );
      }
      if (extensionId) result = result.replaceAll(extensionId, "<extension>");
      if (profile) result = result.replaceAll(profile, "<profile>");
      result = result.replace(
        /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi,
        (id) => identity("document", id),
      );
      result = result.replace(
        /No tab with id: (\d+)\b/g,
        (_, id) => `No tab with id: ${identity("tab", Number(id))}`,
      );
      result = result.replace(
        /(in tab with id )(\d+)/g,
        (_, prefix, id) => `${prefix}${identity("tab", Number(id))}`,
      );
      result = result.replace(
        /No window with id: (\d+)\b/g,
        (_, id) => `No window with id: ${identity("window", Number(id))}`,
      );
      return result;
    }
    if (Array.isArray(value)) return value.map((item) => visit(item));
    if (typeof value === "object")
      return Object.fromEntries(
        Object.keys(value)
          .sort()
          .map((key) => [key, visit(value[key], key, value)]),
      );
    return value;
  }
  return visit(value);
}
