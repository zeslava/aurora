import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

export default defineConfig({
  // Относительная база (VITE_BASE=./) нужна для упаковки в webOS-приложение,
  // где index.html грузится по file://; для веб-деплоя остаётся '/'
  base: process.env.VITE_BASE || '/',
  plugins: [svelte()],
  // webOS 22 и новее — Chromium 87. Синтаксис ES2026 даунлевелится esbuild'ом,
  // но рантайм-методы (Math.sumPrecise, Error.isError, Array.fromAsync) на ТВ
  // отсутствуют — им нужны полифилы.
  build: {
    target: 'chrome87',
  },
  esbuild: {
    target: 'chrome87',
  },
  server: {
    host: true,
    port: 4400,
    proxy: {
      '/api': {
        target: 'http://localhost:4000',
        changeOrigin: true,
        ws: true, // Для WebSocket если нужно
        configure: (proxy, _options) => {
          proxy.on('error', (err, _req, _res) => {
            console.log('proxy error', err);
          });
          proxy.on('proxyReq', (_proxyReq, req, _res) => {
            console.log('Sending Request to the Target:', req.method, req.url);
          });
          proxy.on('proxyRes', (proxyRes, req, _res) => {
            console.log('Received Response from the Target:', proxyRes.statusCode, req.url);
          });
        },
      },
    },
  },
})
