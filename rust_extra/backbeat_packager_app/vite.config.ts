import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
	plugins: [solid()],
	clearScreen: false,
	server: {
		port: 1422,
		strictPort: true,
		watch: { ignored: ["**/src-tauri/**"] },
	},
});
