// Vite の `?raw` インポート (ファイルの中身を文字列で受け取る)。
// ロゴの SVG はこれで読む — アセットを唯一の出所にしておけば、
// 画面用に写した複製とファイルが食い違う事故が起きない。
declare module '*.svg?raw' {
  const content: string;
  export default content;
}
