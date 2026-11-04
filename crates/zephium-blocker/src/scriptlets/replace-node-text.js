// Implements trusted-replace-node-text from uBlock Origin's wiki, Resources Library: "trusted-replace-node-text.js".
function zephiumReplaceNodeText(nodeName = '', pattern = '', replacement = '', ...tokens) {
    const literal = /^\/(.+)\/([dgimsuvy]*)$/.exec(pattern);
    let find = pattern;
    if (literal !== null) {
        try {
            find = new RegExp(literal[1], literal[2]);
        } catch (e) {
            return;
        }
    }
    const text = typeof replacement === 'string' ? replacement : '';
    zephiumNodeText(nodeName, pattern, (before) => before.replace(find, text), tokens);
}
