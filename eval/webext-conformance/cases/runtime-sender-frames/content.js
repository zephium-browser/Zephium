chrome.runtime
  .sendMessage({
    probe: "frame-sender",
    source: {
      url: location.href,
      origin: location.origin,
      top: window === window.top,
    },
  })
  .then((response) => {
    if (response?.received !== true)
      throw new Error("Sender probe was not acknowledged");
  });
