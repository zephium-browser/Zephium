// Shared by the response scriptlets: propsToMatch and stackToMatch as uBlock Origin's Resources Library documents them for "json-prune-fetch-response.js".
function zephiumRequestMatcher(options) {
    const settings = {};
    for (let i = 0; i + 1 < options.length; i += 2) {
        settings[options[i]] = options[i + 1];
    }
    const props = (typeof settings.propsToMatch === 'string' ? settings.propsToMatch.split(/\s+/) : [])
        .filter((token) => token !== '')
        .map((token) => {
            const named = /^([a-zA-Z]+):(.*)$/.exec(token);
            return named === null ? ['url', zephiumPattern(token)] : [named[1], zephiumPattern(named[2])];
        });
    const stack = typeof settings.stackToMatch === 'string' && settings.stackToMatch !== ''
        ? zephiumPattern(settings.stackToMatch)
        : null;
    // `details` holds the request's url and the options it was made with.
    return (details) => props.every(([name, test]) => details[name] !== undefined && test(String(details[name])))
        && (stack === null || stack(String(new Error().stack)));
}
