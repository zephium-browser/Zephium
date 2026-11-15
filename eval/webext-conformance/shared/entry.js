import { createHarness } from "./harness.js";
import { run } from "./case.js";
import config from "./config.js";
const h = createHarness(config);
let status = "complete";
let error;
try {
  await run(h);
} catch (cause) {
  status = "error";
  error = { message: cause.message, stack: cause.stack };
}
await fetch(`${config.collector}/complete`, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({
    status,
    observations: h.observations,
    ...(error ? { error } : {}),
  }),
});
