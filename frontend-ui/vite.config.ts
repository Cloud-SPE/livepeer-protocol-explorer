import { defineConfig } from 'vite';

// Dev-only proxy so the SPA can run on http://localhost:5173 and reach the
// Rust livepeer-api at http://127.0.0.1:8080 same-origin-style. In production
// the FE bundle and the API are served by the same axum process, so no
// proxy is needed there. `/config.json` is included so dev consumes the
// same env-driven runtime config as prod.
const BACKEND = 'http://127.0.0.1:8080';
const BACKEND_PREFIXES = [
  '/health',
  '/metrics',
  '/docs',
  '/openapi.json',
  '/config.json',
  '/backfills',
  '/events',
  '/valuations',
  '/aggregations',
  '/delegators',
  '/governance',
  '/network',
  '/prices',
  '/payouts',
  '/rewards',
  '/rounds',
  '/tickets',
  '/reports',
  '/stake',
  '/transcoders',
  '/orchestrators',
  '/gateways',
];

export default defineConfig({
  server: {
    port: 5173,
    strictPort: false,
    proxy: Object.fromEntries(BACKEND_PREFIXES.map((p) => [p, BACKEND])),
  },
  build: {
    target: 'es2022',
    sourcemap: true,
    outDir: 'dist',
    rollupOptions: {
      output: {
        manualChunks: {
          echarts: ['echarts'],
          openai: ['openai'],
        },
      },
    },
    // Don't <link rel="modulepreload"> the heavy chunks — they should only
    // fetch when a user actually navigates to a view that needs them.
    modulePreload: {
      resolveDependencies: (_filename, deps) =>
        deps.filter((d) => !/\b(echarts|openai)-/.test(d)),
    },
  },
  resolve: {
    dedupe: ['lit', 'rxjs'],
  },
});
