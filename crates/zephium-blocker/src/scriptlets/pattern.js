// Shared by the scriptlets: a "string or regular expression" argument as uBlock Origin's Resources Library documents it.
function zephiumPattern(raw) {
    const text = typeof raw === 'string' ? raw : '';
    if (text === '') {
        return () => true;
    }
    const literal = /^\/(.+)\/([dgimsuvy]*)$/.exec(text);
    if (literal !== null) {
        try {
            const pattern = new RegExp(literal[1], literal[2].replace(/[gy]/g, ''));
            return (value) => pattern.test(value);
        } catch (e) {
            return () => false;
        }
    }
    return (value) => value.includes(text);
}
