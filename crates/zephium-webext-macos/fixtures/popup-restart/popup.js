globalThis.popupRestart = { status: "waiting" };

const settle = (result) => {
  globalThis.popupRestart = result;
  document.querySelector("#status").textContent = result.status;
};

chrome.runtime.sendMessage("popup-restart-probe").then(
  (reply) =>
    settle(
      reply?.status === "ready" && typeof reply.boot === "string"
        ? reply
        : { status: "missing-worker-response" },
    ),
  (error) => settle({ status: "rejected", error: String(error) }),
);
