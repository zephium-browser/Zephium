export async function run(h) {
  for (const [scope, name] of [
    ...["passwordSavingEnabled", "autofillEnabled", "autofillAddressEnabled", "autofillCreditCardEnabled", "alternateErrorPagesEnabled"].map(name => ["services", name]),
    ...["hyperlinkAuditingEnabled", "topicsEnabled", "adMeasurementEnabled", "fledgeEnabled"].map(name => ["websites", name]),
    ["network", "networkPredictionEnabled"],
  ]) {
    const setting = chrome.privacy[scope][name];
    const result = await setting.get({});
    h.record(name, {
      keys: Object.keys(result).sort(),
      valueType: typeof result.value,
      controlValid: ["not_controllable", "controlled_by_other_extensions", "controllable_by_this_extension", "controlled_by_this_extension"].includes(result.levelOfControl),
      callbackShape: await new Promise(resolve => setting.get({}, value => resolve(Object.keys(value).sort()))),
    });
  }
}
