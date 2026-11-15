const registration = self.registration;
const slots = [];
for (const phase of ["install", "activate"]) {
  self.addEventListener(phase, () => slots.push({phase, installing:registration.installing?.state || null, waiting:registration.waiting?.state || null, active:registration.active?.state || null}));
}
chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message === "registration") {
    reply({scopeRoot:registration.scope === chrome.runtime.getURL("/"), stable:registration === self.registration, slots, active:registration.active?.state, installing:registration.installing === null, waiting:registration.waiting === null});
    return;
  }
  if (message !== "update") return;
  Promise.race([
    Promise.resolve().then(() => registration.update()).then(() => ({settlement:"resolved"}), error => ({settlement:"rejected", name:error.name, message:error.message})),
    new Promise(resolve => setTimeout(() => resolve({settlement:"pending after 250ms"}),250)),
  ]).then(reply, error => reply({unexpected:String(error)}));
  return true;
});
