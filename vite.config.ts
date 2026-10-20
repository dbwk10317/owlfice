import { defineConfig, loadEnv } from "vite-plus";

export default defineConfig(({ mode }) => ({
  lint: {
    options: { typeAware: true, typeCheck: true },
    // tsconfig noUnusedLocals를 oxlint는 경고로만 낸다. 빌드가 막히도록 에러로
    rules: { "no-unused-vars": "error" },
    ignorePatterns: ["design/**", "src-tauri/**", "dist/**"],
  },
  fmt: {
    // fixtures는 실제 세션 파일의 모양(한 줄 JSON) 그대로여야 한다
    ignorePatterns: [
      "design/**",
      "**/*.md",
      "src-tauri/**",
      "dist/**",
      "package-lock.json",
      "fixtures/**",
    ],
  },
  // Rust 에러가 Vite 출력에 가려지지 않게
  clearScreen: false,
  server: {
    // tauri.conf.json의 devUrl과 같은 포트. 비어 있지 않으면 실패한다
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
      // WSL 폴백(docs/BUILD.md)에서는 Windows 쪽 편집이 /mnt/c의 inotify로 오지 않는다
      usePolling: loadEnv(mode, ".", "").WSL_DISTRO_NAME !== undefined,
    },
  },
}));
