import { defineConfig } from "vite";
import { frameConfig } from "./vite.frame.ts";

// Onboarding has its own build, vite.onboarding.config.ts; the dev server
// serves every page from this one.
export default defineConfig(frameConfig(["browser", "panel"]));
