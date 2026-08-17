import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 配置前端构建插件，产物由 Rust server 直接托管。
export default defineConfig({
  plugins: [react()],
});

