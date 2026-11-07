// Shared by the timer boosters: the needle, delay and boost ratio uBlock Origin's Resources Library documents for "nano-setTimeout-booster.js".
function zephiumBoostTimer(name, needle, delay, ratio) {
    const callback = zephiumPattern(needle === undefined ? '' : String(needle));
    const wanted = delay === undefined || delay === '' ? 1000 : delay === '*' ? '*' : Number(delay);
    if (wanted !== '*' && !Number.isInteger(wanted)) {
        return;
    }
    let boost = ratio === undefined || ratio === '' ? 0.05 : Number(ratio);
    if (!Number.isFinite(boost) || boost <= 0) {
        return;
    }
    boost = Math.min(Math.max(boost, 0.02), 50);
    self[name] = new Proxy(self[name], {
        apply(target, thisArg, args) {
            const [handler, time] = args;
            if ((wanted === '*' || Number(time) === wanted) && callback(String(handler))) {
                args[1] = Number(time) * boost;
            }
            return Reflect.apply(target, thisArg, args);
        },
    });
}
