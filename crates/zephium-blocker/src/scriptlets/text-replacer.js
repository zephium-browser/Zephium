// Shared by the replace-response scriptlets: the pattern, replacement and `includes, pattern` token uBlock Origin's Resources Library documents for "trusted-replace-fetch-response.js".
function zephiumTextReplacer(pattern, replacement, options) {
    if (typeof pattern !== 'string' || pattern === '') {
        return null;
    }
    const settings = {};
    for (let i = 0; i + 1 < options.length; i += 2) {
        settings[options[i]] = options[i + 1];
    }
    const includes = typeof settings.includes === 'string' && settings.includes !== ''
        ? zephiumPattern(settings.includes)
        : null;
    const value = typeof replacement === 'string' ? replacement : '';
    let search = pattern;
    const literal = /^\/(.+)\/([dgimsuvy]*)$/.exec(pattern);
    if (literal !== null) {
        try {
            search = new RegExp(literal[1], literal[2]);
        } catch (e) {
            return null;
        }
    }
    // A string or a regular expression replaces as String.prototype.replace does: the first match, or every match with the g flag.
    // One expression serves every response; a global or sticky one starts each from the beginning.
    const replace = pattern === '*' ? () => value : (text) => {
        if (search instanceof RegExp) {
            search.lastIndex = 0;
        }
        return text.replace(search, value);
    };
    // The edited text, or null when it would not change.
    return (text) => {
        if (includes !== null && !includes(text)) {
            return null;
        }
        const result = replace(text);
        return result === text ? null : result;
    };
}
