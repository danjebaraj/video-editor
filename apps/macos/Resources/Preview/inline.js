// Renders one item for the editor's inline display and reports its size, so the app can snapshot it.
(function () {
  'use strict';
  let counter = 0;
  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }
  window.Inline = {
    async render(kind, src, dark) {
      const out = document.getElementById('out');
      document.body.className = dark ? 'dark' : 'light';
      out.innerHTML = '';
      if (kind === 'math') {
        try {
          out.innerHTML = katex.renderToString(src, { displayMode: true, throwOnError: true, output: 'html' });
        } catch (e) {
          out.innerHTML = '<div class="err">' + escapeHtml(e.message || e) + '</div>';
        }
      } else {
        try {
          mermaid.initialize({ startOnLoad: false, theme: dark ? 'dark' : 'default', securityLevel: 'strict',
                               fontFamily: '-apple-system, BlinkMacSystemFont, sans-serif' });
          const { svg } = await mermaid.render('inline-mermaid-' + (++counter), src);
          out.innerHTML = svg;
        } catch (e) {
          out.innerHTML = '<div class="err">Mermaid: ' + escapeHtml((e && e.message) || e) + '</div>';
          document.querySelectorAll('[id^="dinline-mermaid-"]').forEach(n => n.remove());
        }
      }
      if (document.fonts && document.fonts.ready) await document.fonts.ready;
      await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
      const r = out.getBoundingClientRect();
      return { w: Math.ceil(r.width), h: Math.ceil(r.height) };
    }
  };
})();
