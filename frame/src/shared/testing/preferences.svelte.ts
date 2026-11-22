/** Reactive state for preference editors; native persistence is tested separately. */
export class PreferenceFixture {
  values = $state<Record<string, string>>({});
  pending = $state(false);
  failed = $state(false);

  value = (key: string) => this.values[key] ?? "";
  saving = () => this.pending;
  saveFailed = () => this.failed;
  enabled = () => false;

  async set(key: string, value: string) {
    this.pending = true;
    try {
      await Promise.resolve();
      if (!this.failed) this.values[key] = value;
    } finally {
      this.pending = false;
    }
  }

  async resetNewTab() {
    this.values = {};
    this.failed = false;
    return true;
  }

  reset() {
    this.values = {};
    this.pending = false;
    this.failed = false;
  }
}
