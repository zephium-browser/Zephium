// Linux, macOS, and Windows are supported.
if (!["darwin", "win32", "linux"].includes(process.platform)) {
  console.error(
    `Zephium is not supported on ${process.platform}.`,
  );
  process.exit(1);
}
