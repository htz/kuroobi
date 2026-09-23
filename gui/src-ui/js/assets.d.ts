declare module '*.svg?raw' {
  const content: string;
  export default content;
}
declare module '*.yaml' {
  const data: Record<string, unknown>;
  export default data;
}
