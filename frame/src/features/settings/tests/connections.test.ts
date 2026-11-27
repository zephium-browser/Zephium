import { describe, expect, test } from "vitest";
import type { WorkServerRowV1 } from "$shared/ipc/bindings";
import {
  draftOf,
  emptyForm,
  formFault,
  formOf,
  joinCommand,
  serverId,
  splitCommand,
} from "../lib/connections";

describe("the server form", () => {
  test("pasted Windows commands keep path separators", () => {
    expect(
      splitCommand(
        String.raw`"C:\Program Files\Tools\server.exe" --config C:\Users\me\config.json`,
      ),
    ).toEqual([
      String.raw`C:\Program Files\Tools\server.exe`,
      "--config",
      String.raw`C:\Users\me\config.json`,
    ]);
    const words = [
      String.raw`C:\Tools\server.exe`,
      String.raw`C:\a b\config.json`,
      String.raw`C:\it's\server.exe`,
    ];
    expect(splitCommand(joinCommand(words))).toEqual(words);
    expect(splitCommand(String.raw`\\server\share\server.exe`)).toEqual([
      String.raw`\\server\share\server.exe`,
    ]);
  });

  test("command lines split and join the way a shell reads them", () => {
    expect(splitCommand(`npx -y "@scope/server" ~/My\\ Files 'a b'`)).toEqual([
      "npx",
      "-y",
      "@scope/server",
      "~/My Files",
      "a b",
    ]);
    expect(splitCommand(`npx "open`)).toBeNull();
    expect(joinCommand(["npx", "-y", "a b", "it's"])).toBe(`npx -y 'a b' 'it'\\''s'`);
  });

  test("ids are short, unique and stable across edits", () => {
    expect(serverId("Linear (work)", [])).toBe("linear-work");
    expect(serverId("Linear", ["linear"])).toBe("linear-2");
    expect(serverId("✨", [])).toBe("server");
  });

  test("secrets leave the server and go beside it", () => {
    const form = {
      ...emptyForm(),
      name: "Notion",
      kind: "stdio" as const,
      command: "npx -y @notionhq/notion-mcp-server",
      env: [
        { name: "NOTION_TOKEN", value: "secret", secret: true, held: false },
        { name: "LOG_LEVEL", value: "info", secret: false, held: false },
      ],
    };
    expect(formFault(form, null)).toBeNull();
    const draft = draftOf(form, null, ["notion"]);
    expect(draft.server.id).toBe("notion-2");
    expect(draft.server.transport).toEqual({
      kind: "stdio",
      command: "npx",
      args: ["-y", "@notionhq/notion-mcp-server"],
      env: [
        { name: "NOTION_TOKEN", secret: true, value: null },
        { name: "LOG_LEVEL", secret: false, value: "info" },
      ],
    });
    expect(draft.secrets).toEqual([{ account: "env.NOTION_TOKEN", value: "secret" }]);
  });

  test("an edit keeps what the Keychain holds and names the old id when renamed", () => {
    const row: WorkServerRowV1 = {
      server: {
        id: "tracker",
        name: "Tracker",
        transport: { kind: "http", url: "https://mcp.linear.app/mcp", auth: "bearer" },
        enabled: false,
      },
      secrets: ["bearer"],
      signed_in: false,
    };
    const form = formOf(row);
    expect(formFault(form, row)).toBeNull();
    expect(draftOf(form, row, ["tracker"])).toMatchObject({ secrets: [], previous: null });
    const renamed = draftOf({ ...form, name: "Linear" }, row, ["tracker"]);
    expect(renamed).toMatchObject({
      previous: "tracker",
      server: { id: "linear", enabled: false },
    });
    expect(formFault({ ...form, url: "http://example.com" }, row)).toBe("url");
    expect(formFault({ ...form, name: "" }, row)).toBe("name");
  });
});
