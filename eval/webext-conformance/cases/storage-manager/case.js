export async function run(h) {
  const storage = navigator.storage;
  h.record("native interface", {estimate: typeof storage?.estimate, persist: typeof storage?.persist, persisted: typeof storage?.persisted});
  const before = await storage.estimate();
  h.record("estimate", {quotaFinite: Number.isFinite(before.quota) && before.quota > 0, usageFinite: Number.isFinite(before.usage) && before.usage >= 0});
  const persisted = await storage.persisted();
  const accepted = await storage.persist();
  h.record("persistence", {beforeType: typeof persisted, acceptedType: typeof accepted, settled: await storage.persisted() === accepted});
}
