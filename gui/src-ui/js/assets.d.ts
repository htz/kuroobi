// Vite `?raw` imports (file contents as a string). The logo SVG loads
// this way — one source of truth prevents copy drift.
declare module '*.svg?raw' {
  const content: string;
  export default content;
}
// Locale data (@rollup/plugin-yaml turns each file into a module).
declare module '*.yaml' {
  const data: Record<string, unknown>;
  export default data;
}
