import { expect, test } from "vitest";
import { graphicsDetails, graphicsStatus, graphicsWarnings } from "../lib/graphics";
import { environmentOverride, gpuReport, steppedDown } from "./graphics-fixtures";

test("Automatic on a driver without known problems runs the full GPU path", () => {
  expect(graphicsStatus(gpuReport())).toEqual({
    kind: "current",
    title: "Running in GPU mode",
    detail: "No problems are known for your graphics driver, so Automatic uses full acceleration.",
  });
});

test("Automatic says when policy steps away from the default path", () => {
  const status = graphicsStatus(
    gpuReport({
      detection: { driver_class: "virtio_virgl" },
      selection: { path: "disable_dmabuf", reason: "virtio/virgl: DMA-BUF icon flicker" },
    }),
  );
  expect(status.title).toBe("Running in Safe mode");
  expect(status.detail).toContain("known display problem");
});

test("a software rasterizer is named as such, not as GPU acceleration", () => {
  const report = gpuReport({ detection: { driver_class: "software" } });
  report.detection.gl!.software = true;
  expect(graphicsStatus(report).detail).toBe(
    "Your system has no hardware graphics acceleration, so the processor draws pages.",
  );
  expect(graphicsWarnings(report).map((warning) => warning.text)).toContain(
    "Pages are drawn in software: no hardware graphics acceleration is in use.",
  );
});

test("a saved mode says the person chose it", () => {
  const report = gpuReport({
    preference: "compatibility",
    running_preference: "compatibility",
    selection: { path: "force_shm", source: "preference" },
  });
  expect(graphicsStatus(report)).toMatchObject({
    title: "Running in Compatibility mode",
    detail: "You chose this mode.",
  });
});

test("an environment override names only the renderer variables, with their values", () => {
  const status = graphicsStatus(gpuReport({ ...environmentOverride(), preference: "gpu" }));
  expect(status).toEqual({
    kind: "environment",
    title: "Running in Safe mode, set by an environment variable",
    detail: expect.stringContaining("takes priority over the choice above"),
    variables: ["WEBKIT_DISABLE_DMABUF_RENDERER=1"],
  });
});

test("a pending restart names the saved mode and the one still running", () => {
  const status = graphicsStatus(gpuReport({ preference: "safe", restart_required: true }));
  expect(status).toEqual({
    kind: "restart",
    title: "Restart Zephium to use Safe",
    detail: "Until then, Zephium keeps running in GPU mode.",
  });
});

test("a saved mode that matches the running path asks for no restart", () => {
  const status = graphicsStatus(gpuReport({ preference: "gpu", restart_required: true }));
  expect(status.kind).toBe("current");
  expect(status.detail).toBe(
    "Saved. Zephium already runs this way, so there’s no need to restart.",
  );
});

test("a step-down after a failed start offers to keep the simpler mode", () => {
  const status = graphicsStatus(gpuReport(steppedDown()));
  expect(status).toMatchObject({
    kind: "recovered",
    title: "Running in Compatibility mode after a startup problem",
    keep: "compatibility",
  });
  expect(status.detail).toContain("tries the faster mode again");
  // Keeping it saves the mode that already runs, so nothing waits for a restart.
  const kept = gpuReport({ ...steppedDown(), preference: "compatibility", restart_required: true });
  expect(graphicsStatus(kept).kind).toBe("current");
  // Choosing a faster mode instead does.
  const faster = gpuReport({ ...steppedDown(), preference: "gpu", restart_required: true });
  expect(graphicsStatus(faster).kind).toBe("restart");
});

test("details read as people name their hardware, with the native reason kept verbatim", () => {
  const rows = Object.fromEntries(graphicsDetails(gpuReport()).map((row) => [row.id, row]));
  expect(rows.driver!.value).toBe("AMD (Mesa RadeonSI)");
  expect(rows.gpu!.value).toContain("AMD Radeon RX 6600");
  expect(rows.opengl!.value).toBe("OpenGL ES 3.2 Mesa 25.2.4");
  expect(rows["driver-version"]!.value).toBe("Mesa 25.2.4");
  expect(rows.session!.value).toBe("Wayland · Mutter");
  expect(rows.engine!.value).toBe("WebKitGTK 2.54.2 · GTK 3.24.52");
  expect(rows.selected!.value).toBe("GPU, chosen by Automatic");
  expect(rows.selected!.note).toBe("no evidenced renderer exception; retain WebKit defaults");
});

test("missing GL facts fall back to the kernel device and say what is unknown", () => {
  const report = gpuReport({
    detection: { gl: null, session: "x11", compositor: null, probe_outcome: "unavailable" },
  });
  const rows = Object.fromEntries(graphicsDetails(report).map((row) => [row.id, row.value]));
  expect(rows.gpu).toBe("amdgpu");
  expect(rows.opengl).toBe("Not reported");
  expect(rows["driver-version"]).toBe("Not reported");
  expect(rows.session).toBe("X11");
  expect(graphicsWarnings(report)).toEqual([
    {
      text: "The OpenGL check at startup didn’t complete, so some details are missing.",
      native: false,
    },
  ]);
});

test("NVIDIA's proprietary version outranks Mesa, and native warnings stay untranslated", () => {
  const report = gpuReport({
    detection: { driver_class: "nvidia_proprietary" },
    state_warnings: ["renderer state could not be read"],
  });
  report.detection.gl!.nvidia_version = "580.95.05";
  report.detection.gl!.mesa_version = null;
  const rows = Object.fromEntries(graphicsDetails(report).map((row) => [row.id, row.value]));
  expect(rows.driver).toBe("NVIDIA (proprietary driver)");
  expect(rows["driver-version"]).toBe("NVIDIA 580.95.05");
  expect(graphicsWarnings(report)).toEqual([
    { text: "renderer state could not be read", native: true },
  ]);
});
