import { defineConfig } from 'vite';

// Dev-only proxy so the SPA can run on http://localhost:5173 and reach the
// Rust livepeer-api at http://127.0.0.1:8080 without CORS. In production the
// `baseApiUrl` from `public/config.json` does the routing and CORS is the
// backend's problem.
const BACKEND = 'http://127.0.0.1:8080';
const BACKEND_PREFIXES = [
  '/health',
  '/metrics',
  '/docs',
  '/openapi.json',
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
