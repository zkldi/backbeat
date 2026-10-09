import backbeatConfig from "eslint-config-backbeat";

export default [
	{ ignores: ["node_modules/**", "src-tauri/**", "*.config.ts"] },
	...backbeatConfig.base,
];
