// Shared by the value scriptlets: the fixed values uBlock Origin's Resources Library documents for set-constant.
function zephiumConstant(raw) {
    const text = typeof raw === 'string' ? raw.trim() : '';
    switch (text) {
        case 'undefined': return { value: undefined };
        case 'false': return { value: false };
        case 'true': return { value: true };
        case 'null': return { value: null };
        case "''": case 'emptyStr': return { value: '' };
        case '[]': case 'emptyArr': return { value: [] };
        case '{}': case 'emptyObj': return { value: {} };
        case 'noopFunc': return { value: function () {} };
        case 'trueFunc': return { value: function () { return true; } };
        case 'falseFunc': return { value: function () { return false; } };
        case 'throwFunc': return { value: function () { throw new TypeError(); } };
        default:
            if (/^\d+$/.test(text) && Number(text) <= 32767) {
                return { value: Number(text) };
            }
            return null;
    }
}
