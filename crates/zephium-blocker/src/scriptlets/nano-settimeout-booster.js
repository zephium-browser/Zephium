// Implements nano-setTimeout-booster (alias nano-stb) from uBlock Origin's wiki, Resources Library: "nano-setTimeout-booster.js".
function zephiumSetTimeoutBooster(needle, delay, ratio) {
    zephiumBoostTimer('setTimeout', needle, delay, ratio);
}
