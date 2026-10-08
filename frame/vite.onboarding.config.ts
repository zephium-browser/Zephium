import { defineConfig } from "vite";
import { frameConfig } from "./vite.frame.ts";

export default defineConfig(frameConfig(["onboarding"]));
