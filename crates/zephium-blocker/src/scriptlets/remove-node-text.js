// Implements remove-node-text from uBlock Origin's wiki, Resources Library: "remove-node-text.js".
function zephiumRemoveNodeText(nodeName = '', pattern = '', ...tokens) {
    zephiumNodeText(nodeName, pattern, () => '', tokens);
}
