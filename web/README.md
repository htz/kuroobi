# KUROOBI site

The KUROOBI site and Kuroobi for web, built with Astro: Japanese at `/`, English under `/en/`.
The engine is `crates/web` (kuroobi_ai.wasm) running in a worker. On a cross-origin isolated page it is the
threaded build (wasm32-wasip1-threads), searching on up to 8 threads lent as workers (`kuroobi_ai_thread.js`);
elsewhere the single-threaded one.

## Develop

```sh
npm ci
scripts/engine.sh   # builds both kuroobi_ai.wasm; puts them and model/nnue.bin in public/engine/
npm run dev
```

`model/nnue.bin` is the model the site serves, `nnue_to_web`'s output
(`cargo run -p train --release --features nnue-web --bin nnue_to_web IN.bin OUT.bin`), kept in git.
`public/engine/` is generated and stays out of git.

## Notes

One file per article and language: `src/content/notes/ja/<slug>.mdx` and `src/content/notes/en/<slug>.mdx`,
with `title`, `summary`, `date` and `tags` (`src/content.config.ts`). `<Position kifu="f5d6c3" caption="…" />`
draws a position with a link to it in Study.

## Deploy

The site is served from an S3 bucket through CloudFront; the infrastructure lives elsewhere. GitHub Actions
deploys from `main` and `web-model`, and only from there, the site and the model each only when it changed:

- `deploy-site.yml` (pages, scripts, the wasm; `web/` other than `model/`, `crates/web`, the engine's `src/`)
  builds the site and uploads it.
- `deploy-model.yml` (`model/nnue.bin`) uploads the model and points `engine/model.json` at it.

Both assume a role through GitHub's OIDC and read the repository variables `AWS_REGION`,
`AWS_DEPLOY_ROLE_ARN`, `KUROOBI_WEB_BUCKET` and `KUROOBI_WEB_DISTRIBUTION_ID`.

Every page needs `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`
for the threaded engine (`npm run dev` and `npm run preview` send them; the distribution must too). Without
them the page is not isolated and the engine runs on one thread.

The wasm and the model are named by their hash (`engine/kuroobi_ai-<hash>.wasm`,
`engine/kuroobi_ai-mt-<hash>.wasm`, `engine/nnue-<hash>.bin`)
and cached for a year. The site names its wasm at build time; the model it finds at run time in
`engine/model.json`, which is never cached and also gives the model's size, so a new model needs no new
site. Both go up brotli-compressed, since CloudFront compresses only files up to 10 MB. Pages are checked
again on every visit. The distribution must serve `dir/index.html` for `dir/` and `/404.html` for missing
paths.
