export async function run(h) {
  for (const name of ["passwordSavingEnabled", "autofillEnabled", "autofillAddressEnabled", "autofillCreditCardEnabled"]) {
    const setting = chrome.privacy.services[name];
    const result = await setting.get({});
    h.record(name, {
      keys: Object.keys(result).sort(),
      valueType: typeof result.value,
      controlValid: ["not_controllable", "controlled_by_other_extensions", "controllable_by_this_extension", "controlled_by_this_extension"].includes(result.levelOfControl),
      callbackShape: await new Promise(resolve => setting.get({}, value => resolve(Object.keys(value).sort()))),
    });
  }
}
