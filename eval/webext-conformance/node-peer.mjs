import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
export class RustPeer {
  constructor(executable) {
    this.next = 0;
    this.pending = new Map();
    this.contexts = new Map();
    this.child = spawn(
      executable,
      ["--ignored", "--exact", "stdio_host", "--nocapture"],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    this.stderr = "";
    this.child.stderr.on("data", (chunk) => {
      this.stderr = (this.stderr + chunk).slice(-16384);
    });
    this.child.on("error", (error) => this.fail(error));
    this.child.on("exit", (code) => {
      if (!this.closing)
        this.fail(new Error(`Rust dispatcher exited ${code}: ${this.stderr}`));
    });
    this.lines = createInterface({ input: this.child.stdout });
    this.lines.on("line", (line) => {
      const start = line.indexOf("ZEPHIUM_E2E:");
      if (start < 0) return;
      try {
        const record = JSON.parse(line.slice(start + "ZEPHIUM_E2E:".length));
        if (record.frame) {
          const realm = this.contexts.get(record.context);
          if (!realm?.receiver)
            throw new Error(`Unregistered context ${record.context}`);
          realm.receiver(new realm.Bytes(record.frame));
        } else {
          const request = this.pending.get(record.id);
          if (!request) throw new Error(`Unknown reply ${record.id}`);
          this.pending.delete(record.id);
          request.resolve(record.result);
        }
      } catch (error) {
        this.fail(error);
      }
    });
  }
  fail(error) {
    this.error = error;
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
  }
  command(op, data = {}) {
    if (this.error) return Promise.reject(this.error);
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.child.stdin.write(JSON.stringify({ id, op, ...data }) + "\n");
    });
  }
  async context(binding, manifest) {
    const label = "background";
    await this.command("context", { context: label, kind: "background" });
    const realm = binding.createRealm();
    realm.Bytes = realm.run("Uint8Array");
    const global = realm.run("globalThis");
    global.__zephiumPost = (bytes) => {
      this.command("frame", { context: label, frame: Array.from(bytes) }).catch(
        (error) => this.fail(error),
      );
      return true;
    };
    global.__zephiumRegisterReceiver = (receiver) => {
      realm.receiver = receiver;
      return true;
    };
    this.contexts.set(label, realm);
    realm.Z.api.page(
      binding.generatedSurface,
      binding.bootstrap({
        contextKind: "background",
        manifest,
        permissions: manifest.permissions || [],
        messages: {},
      }),
    );
    return realm;
  }
  async close() {
    this.closing = true;
    this.child.stdin.end();
    if (this.child.exitCode === null && this.child.signalCode === null)
      await new Promise((resolve) => {
        const timer = setTimeout(() => this.child.kill("SIGKILL"), 2000);
        this.child.once("exit", () => {
          clearTimeout(timer);
          resolve();
        });
      });
    this.lines.close();
  }
}
