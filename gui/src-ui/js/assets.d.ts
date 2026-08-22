// Vite `?raw` imports (file contents as a string). The logo SVG loads
// this way — one source of truth prevents copy drift.
declare module '*.svg?raw' {
  const content: string;
  export default content;
}
