// Builds the window content into dist/. The spec preview is the macOS app's preview page and
// scripts, used unchanged (ui/spec.md §23.3): they are copied into dist/preview/ with a Windows
// index.html whose CSP allows the media host, plus shim.js, which forwards the page's messages
// to the window and maps its `vs-media://thumb/…` image URLs onto the media host.
import { defineConfig, type Plugin } from 'vite';
import { cpSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const macPreview = resolve(here, '../../macos/Resources/Preview');

function preview(): Plugin {
  return {
    name: 'mac-preview',
    apply: 'build',
    closeBundle() {
      const out = resolve(here, 'dist/preview');
      mkdirSync(out, { recursive: true });
      cpSync(macPreview, out, { recursive: true });
      cpSync(resolve(here, 'preview-shim/shim.js'), resolve(out, 'shim.js'));
      cpSync(resolve(here, 'preview-shim/windows.css'), resolve(out, 'windows.css'));
      let html = readFileSync(resolve(macPreview, 'index.html'), 'utf8');
      html = html
        .replace(/img-src [^;]*;/, "img-src https://media.video-studio.invalid data:;")
        .replace('<script src="preview.js"></script>', '<script src="shim.js"></script>\n<script src="preview.js"></script>')
        .replace('<link rel="stylesheet" href="preview.css">', '<link rel="stylesheet" href="preview.css">\n<link rel="stylesheet" href="windows.css">');
      writeFileSync(resolve(out, 'index.html'), html);
    },
  };
}

export default defineConfig({
  root: here,
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    chunkSizeWarningLimit: 4000,
    // No eval and no remote code: the CSP forbids both.
    modulePreload: { polyfill: false },
  },
  plugins: [preview()],
});
