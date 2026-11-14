export function isSupportedPlatform(platform) {
  return ["darwin", "win32", "linux"].includes(platform);
}

if (!isSupportedPlatform(process.platform)) {
  console.error("Zephium supports macOS, Windows, and Linux (experimental).");
  process.exit(1);
}
