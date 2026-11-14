chrome.runtime.sendMessage({ kind: 'popup' }).then((reply) => {
  document.title = 'reply:' + JSON.stringify(reply);
}, (error) => {
  document.title = 'error:' + error.message;
});
