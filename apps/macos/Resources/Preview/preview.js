/* Markdown preview renderer. All libraries are bundled; nothing is loaded from the network. */
(function () {
  'use strict';

  const post = (msg) => {
    try { window.webkit.messageHandlers.md.postMessage(msg); } catch (e) { /* not in app */ }
  };

  const state = { media: {}, docDir: '', noteFolder: '', remoteImages: false, copyButtons: true, islandOutlines: false, wiki: {}, theme: 'light', showFrontMatter: true, renderId: 0 };

  function escapeHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function slugify(text) {
    return text.toLowerCase().trim()
      .replace(/<[^>]+>/g, '')
      .replace(/[^\p{L}\p{N}_\- ]/gu, '')
      .replace(/ /g, '-');
  }

  // ---------- markdown-it setup ----------
  const md = window.markdownit({
    html: true,
    linkify: true,
    typographer: false,
    highlight: function (code, lang) {
      if (lang && lang.toLowerCase() === 'mermaid') return '';
      if (lang && window.hljs && hljs.getLanguage(lang)) {
        try { return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value; } catch (e) { }
      }
      return '';
    }
  });
  md.use(window.markdownitFootnote);
  // Enabled so clicks reach us; a click asks the app to toggle the source line (spec section 12c).
  md.use(window.markdownitTaskLists, { enabled: true, label: false });

  // Source line numbers on block tokens, for scroll sync.
  md.core.ruler.push('source_lines', function (st) {
    for (const t of st.tokens) {
      if (t.map && (t.nesting === 1 || t.type === 'fence' || t.type === 'code_block' || t.type === 'hr' || t.type === 'html_block' || t.type === 'math_block')) {
        t.attrSet('data-line', String(t.map[0]));
      }
    }
  });

  // Islands: top-level <!-- island: name --> / <!-- /island: name --> comments. They render as hidden
  // placeholders (export turns them back into comments), or, with "Show island outlines", as a
  // labelled box around the island. Markers in code are code tokens, so they are never matched.
  const ISLAND_RX = /^[ \t]{0,3}<!--\s*(\/?)\s*island\s*:\s*(.*?)\s*-->[ \t]*$/i;
  md.core.ruler.push('islands', function (st) {
    const open = [];
    for (const t of st.tokens) {
      if (t.type !== 'html_block' || t.level !== 0) continue;
      const m = t.content.replace(/\n+$/, '').match(ISLAND_RX);
      if (!m) continue;
      t.meta = { island: m[2], end: m[1] === '/', paired: false };
      if (!t.meta.end) { open.push(t); continue; }
      const s = open.length ? open[open.length - 1] : null;
      if (s && s.meta.island === t.meta.island) { s.meta.paired = true; t.meta.paired = true; open.pop(); }
    }
  });
  const defaultHtmlBlock = md.renderer.rules.html_block;
  md.renderer.rules.html_block = function (tokens, idx, options, env, self) {
    const t = tokens[idx];
    if (t.meta && t.meta.island !== undefined) {
      const name = escapeHtml(t.meta.island);
      if (state.islandOutlines && t.meta.paired) {
        return t.meta.end ? '</div>\n' : '<div class="island-outline" data-island="' + name + '">\n';
      }
      return '<span hidden class="island-marker" data-island="' + name + '" data-island-end="' + (t.meta.end ? '1' : '0') + '"></span>\n';
    }
    return defaultHtmlBlock(tokens, idx, options, env, self);
  };

  // Heading ids (GitHub style, de-duplicated).
  md.core.ruler.push('heading_ids', function (st) {
    const seen = {};
    for (let i = 0; i < st.tokens.length; i++) {
      const t = st.tokens[i];
      if (t.type !== 'heading_open') continue;
      const inline = st.tokens[i + 1];
      const text = inline && inline.children ? inline.children.filter(c => c.type === 'text' || c.type === 'code_inline').map(c => c.content).join('') : '';
      let slug = slugify(text);
      if (seen[slug] !== undefined) { seen[slug] += 1; slug = slug + '-' + seen[slug]; } else { seen[slug] = 0; }
      t.attrSet('id', slug);
    }
  });

  // GitHub callouts: > [!NOTE] …
  const calloutIcons = {
    note: '<svg class="octicon" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"></path></svg>',
    tip: '<svg class="octicon" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M8 1.5c-2.363 0-4 1.69-4 3.75 0 .984.424 1.625.984 2.304l.214.253c.223.264.47.556.673.848.284.411.537.896.621 1.49a.75.75 0 0 1-1.484.211c-.04-.282-.163-.547-.37-.847a8.456 8.456 0 0 0-.542-.68c-.084-.1-.173-.205-.268-.32C3.201 7.75 2.5 6.766 2.5 5.25 2.5 2.31 4.863 0 8 0s5.5 2.31 5.5 5.25c0 1.516-.701 2.5-1.328 3.259-.095.115-.184.22-.268.319-.207.245-.383.453-.541.681-.208.3-.33.565-.37.847a.751.751 0 0 1-1.485-.212c.084-.593.337-1.078.621-1.489.203-.292.45-.584.673-.848.075-.088.147-.173.213-.253.561-.679.985-1.32.985-2.304 0-2.06-1.637-3.75-4-3.75ZM5.75 12h4.5a.75.75 0 0 1 0 1.5h-4.5a.75.75 0 0 1 0-1.5ZM6 15.25a.75.75 0 0 1 .75-.75h2.5a.75.75 0 0 1 0 1.5h-2.5a.75.75 0 0 1-.75-.75Z"></path></svg>',
    important: '<svg class="octicon" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H8.06l-2.573 2.573A1.458 1.458 0 0 1 3 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Zm1.75-.25a.25.25 0 0 0-.25.25v9.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h6.5a.25.25 0 0 0 .25-.25v-9.5a.25.25 0 0 0-.25-.25Zm7 2.25v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 9a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"></path></svg>',
    warning: '<svg class="octicon" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"></path></svg>',
    caution: '<svg class="octicon" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M4.47.22A.749.749 0 0 1 5 0h6c.199 0 .389.079.53.22l4.25 4.25c.141.14.22.331.22.53v6a.749.749 0 0 1-.22.53l-4.25 4.25A.749.749 0 0 1 11 16H5a.749.749 0 0 1-.53-.22L.22 11.53A.749.749 0 0 1 0 11V5c0-.199.079-.389.22-.53Zm.84 1.28L1.5 5.31v5.38l3.81 3.81h5.38l3.81-3.81V5.31L10.69 1.5ZM8 4a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 4Zm0 8a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"></path></svg>'
  };
  md.core.ruler.after('block', 'callouts', function (st) {
    const tokens = st.tokens;
    for (let i = 0; i < tokens.length; i++) {
      if (tokens[i].type !== 'blockquote_open') continue;
      const p = tokens[i + 1], inline = tokens[i + 2];
      if (!p || p.type !== 'paragraph_open' || !inline || inline.type !== 'inline') continue;
      const m = inline.content.match(/^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][ \t]*(?:\n|$)/i);
      if (!m) continue;
      const kind = m[1].toLowerCase();
      tokens[i].attrJoin('class', 'markdown-alert markdown-alert-' + kind);
      inline.content = inline.content.slice(m[0].length);
      const title = new st.Token('html_block', '', 0);
      title.content = '<p class="markdown-alert-title">' + calloutIcons[kind] + kind.charAt(0).toUpperCase() + kind.slice(1) + '</p>\n';
      tokens.splice(i + 1, 0, title);
      if (inline.content.trim() === '') {
        // Title-only first paragraph: drop the empty paragraph.
        tokens.splice(i + 2, 3);
      }
    }
  });

  // Math: $inline$ and $$block$$ (KaTeX).
  md.inline.ruler.after('escape', 'math_inline', function (st, silent) {
    const src = st.src, pos = st.pos;
    if (src.charCodeAt(pos) !== 0x24) return false;
    if (src.charCodeAt(pos + 1) === 0x24) {
      // $$...$$ inline
      const end = src.indexOf('$$', pos + 2);
      if (end < 0) return false;
      if (!silent) { const t = st.push('math_inline_block', 'math', 0); t.content = src.slice(pos + 2, end); }
      st.pos = end + 2;
      return true;
    }
    const next = src.charCodeAt(pos + 1);
    if (next === 0x20 || next === 0x09 || isNaN(next)) return false;
    let end = pos + 1;
    while ((end = src.indexOf('$', end)) !== -1) {
      if (src.charCodeAt(end - 1) === 0x5C) { end++; continue; }
      break;
    }
    if (end < 0 || end === pos + 1) return false;
    const prev = src.charCodeAt(end - 1);
    if (prev === 0x20 || prev === 0x09) return false;
    const after = src.charCodeAt(end + 1);
    if (after >= 0x30 && after <= 0x39) return false;
    if (!silent) { const t = st.push('math_inline', 'math', 0); t.content = src.slice(pos + 1, end); }
    st.pos = end + 1;
    return true;
  });
  md.block.ruler.after('blockquote', 'math_block', function (st, startLine, endLine, silent) {
    let pos = st.bMarks[startLine] + st.tShift[startLine];
    let max = st.eMarks[startLine];
    if (st.sCount[startLine] - st.blkIndent >= 4) return false;
    if (st.src.slice(pos, pos + 2) !== '$$') return false;
    let firstLine = st.src.slice(pos + 2, max);
    let content = '';
    let line = startLine;
    let found = false;
    if (firstLine.trim().endsWith('$$') && firstLine.trim().length > 2) {
      content = firstLine.trim().slice(0, -2);
      found = true;
    } else {
      content = firstLine.trim() ? firstLine + '\n' : '';
      while (++line < endLine) {
        pos = st.bMarks[line] + st.tShift[line];
        max = st.eMarks[line];
        const text = st.src.slice(pos, max);
        if (text.trim().endsWith('$$')) {
          content += text.trim().slice(0, -2);
          found = true;
          break;
        }
        content += st.src.slice(st.bMarks[line], max) + '\n';
      }
    }
    if (!found) return false;
    if (silent) return true;
    st.line = line + 1;
    const t = st.push('math_block', 'math', 0);
    t.block = true;
    t.content = content;
    t.map = [startLine, st.line];
    return true;
  }, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });

  function renderMath(src, display) {
    try {
      return katex.renderToString(src, { displayMode: display, throwOnError: false, output: 'html' });
    } catch (e) {
      return '<span class="math-error">' + escapeHtml(src) + '</span>';
    }
  }
  md.renderer.rules.math_inline = (t, i) => renderMath(t[i].content, false);
  md.renderer.rules.math_inline_block = (t, i) => renderMath(t[i].content, true);
  md.renderer.rules.math_block = (t, i) => '<div class="math-block" data-line="' + (t[i].map ? t[i].map[0] : '') + '">' + renderMath(t[i].content, true) + '</div>\n';

  const SPEC_BLOCKS = { vo: 'Voice-over', music: 'Music', cta: 'Call to action', shot: 'Shot' };
  // Fences: Mermaid diagrams and highlighted code.
  const defaultFence = md.renderer.rules.fence;
  md.renderer.rules.fence = function (tokens, idx, options, env, self) {
    const t = tokens[idx];
    const lang = (t.info || '').trim().split(/\s+/)[0].toLowerCase();
    if (lang === 'mermaid') {
      return '<pre class="mermaid" data-line="' + (t.map ? t.map[0] : '') + '">' + escapeHtml(t.content) + '</pre>\n';
    }
    // Structured spec blocks: the voice-over script, the music brief and the call to action.
    if (SPEC_BLOCKS[lang]) {
      return '<div class="spec-block spec-' + lang + '" data-line="' + (t.map ? t.map[0] : '') + '"><span class="spec-block-label">' + SPEC_BLOCKS[lang] +
        '</span><div class="spec-block-body">' + escapeHtml(t.content.replace(/\n$/, '')) + '</div></div>\n';
    }
    if (!lang || !(window.hljs && hljs.getLanguage(lang))) {
      // Auto-highlight unknown/blank languages lightly: leave as plain.
    }
    t.attrJoin('class', 'hljs-block');
    return defaultFence(tokens, idx, options, env, self);
  };

  // Images. Only relative paths that stay inside the notes folder load, through the app's
  // mdraft-asset scheme (the app checks the path again, resolving symbolic links). Remote images
  // load through mdraft-remote only when the setting allows; everything else (absolute paths,
  // file:, data:, paths that climb out of the notes folder) shows as "[image unavailable]".
  // Mirrors PreviewImagePath.relativeToNotes in MarkdownCore.
  function localImagePath(raw) {
    let s = (raw || '').trim();
    if (s.startsWith('<') && s.endsWith('>')) s = s.slice(1, -1);
    if (!s || s[0] === '/' || s[0] === '\\' || /[?#]/.test(s) || /^[a-z][a-z0-9+.\-]*:/i.test(s)) return null;
    let decoded;
    try { decoded = decodeURIComponent(s); } catch (e) { return null; }
    if (decoded.indexOf('\0') >= 0) return null;
    const parts = state.noteFolder ? state.noteFolder.split('/').filter(Boolean) : [];
    for (const p of decoded.split('/')) {
      if (!p || p === '.') continue;
      if (p === '..') { if (!parts.length) return null; parts.pop(); } else parts.push(p);
    }
    return parts.length ? parts.map(encodeURIComponent).join('/') : null;
  }
  const defaultImage = md.renderer.rules.image;
  md.renderer.rules.image = function (tokens, idx, options, env, self) {
    tokens[idx].attrSet('data-raw-src', tokens[idx].attrGet('src'));
    return defaultImage(tokens, idx, options, env, self);
  };
  const COPY_ICON = '<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4">' +
    '<rect x="5.5" y="5.5" width="8.5" height="8.5" rx="1.8"/><path d="M10.5 3.2V3.5A1.5 1.5 0 0 0 9 2H3.5A1.5 1.5 0 0 0 2 3.5V9a1.5 1.5 0 0 0 1.5 1.5h.3"/></svg>';

  // ---------- Rendering ----------
  // Exported and copied HTML keeps checkboxes read-only.
  function readOnlyTasks(html) {
    return html.replace(/<input class="task-list-item-checkbox"/g, '<input class="task-list-item-checkbox" disabled=""');
  }

  function splitFrontMatter(text) {
    const m = text.match(/^---[ \t]*\r?\n([\s\S]*?)\r?\n(---|\.\.\.)[ \t]*(\r?\n|$)/);
    if (!m) return { body: text, meta: null };
    const lines = m[0].split('\n').length - (m[3] ? 1 : 0);
    // Replace front matter with blank lines so source line numbers stay aligned.
    return { body: '\n'.repeat(lines) + text.slice(m[0].length), meta: m[1] };
  }

  // Todo markers (spec section 12c): a leading "! " becomes a red badge and "@category " a pill. Not links.
  const CATEGORY_RX = /^(!\s+)?(?:@([\p{L}][\p{L}\p{N}_\-/]*)\s+)?/u;
  function decorateTodos(root) {
    root.querySelectorAll('li.task-list-item > input.task-list-item-checkbox, li.task-list-item > p > input.task-list-item-checkbox').forEach(box => {
      let node = box.nextSibling;
      while (node && node.nodeType === 3 && node.textContent === '') node = node.nextSibling;
      if (!node || node.nodeType !== 3) return;
      const text = node.textContent.replace(/^\s+/, '');
      const m = text.match(CATEGORY_RX);
      if (!m || (!m[1] && !m[2])) return;
      const frag = document.createDocumentFragment();
      frag.appendChild(document.createTextNode(' '));
      if (m[1]) {
        const b = document.createElement('span');
        b.className = 'todo-important';
        b.textContent = '!';
        b.title = 'Important';
        frag.appendChild(b);
      }
      if (m[2]) {
        const c = document.createElement('span');
        c.className = 'todo-category';
        c.textContent = m[2];
        frag.appendChild(c);
      }
      frag.appendChild(document.createTextNode(' ' + text.slice(m[0].length)));
      node.parentNode.replaceChild(frag, node);
    });
  }

  // The Auto-tagged line's tags in the auto-tag colour (spec section 14b).
  function decorateAutoTags(root) {
    root.querySelectorAll('p').forEach(p => {
      if (!/^Auto-tagged:(\s*#[^\s#]+)*\s*$/.test(p.textContent.trim()) || p.children.length) return;
      p.classList.add('auto-tagged');
      const tags = p.textContent.trim().slice('Auto-tagged:'.length).trim().split(/\s+/).filter(Boolean);
      p.textContent = 'Auto-tagged: ';
      tags.forEach((t, i) => {
        const s = document.createElement('span');
        s.className = 'auto-tag';
        s.textContent = t;
        p.appendChild(s);
        if (i < tags.length - 1) p.appendChild(document.createTextNode(' '));
      });
    });
  }

  // A todo's " · due …" / " · every … from …" part becomes a small date chip (spec section 15a).
  const DUE_RX = / · (due \d{4}-\d{2}-\d{2}|every .+ from \d{4}-\d{2}-\d{2})\s*$/;
  function decorateDueDates(root) {
    root.querySelectorAll('li.task-list-item').forEach(li => {
      // The item's own last text node (not a nested list's).
      let node = null;
      for (const n of li.childNodes) {
        if (n.nodeType === 3) node = n;
        else if (n.nodeType === 1 && n.tagName === 'P') { for (const c of n.childNodes) if (c.nodeType === 3) node = c; }
        else if (n.nodeType === 1 && (n.tagName === 'UL' || n.tagName === 'OL')) break;
      }
      if (!node) return;
      const m = node.textContent.match(DUE_RX);
      if (!m) return;
      const chip = document.createElement('span');
      const repeating = m[1].startsWith('every');
      chip.className = 'todo-due' + (repeating ? ' repeating' : '');
      chip.textContent = (repeating ? '↻ ' : '') + m[1].replace(/^due /, '');
      chip.title = m[1];
      const before = node.textContent.slice(0, m.index);
      node.parentNode.insertBefore(document.createTextNode(before + ' '), node);
      node.parentNode.insertBefore(chip, node);
      node.parentNode.removeChild(node);
    });
  }

  // Todos that are stopped (": stop") and checked are left out when Hide stopped items is on.
  function hideStoppedTodos(root) {
    root.querySelectorAll('li.task-list-item').forEach(li => {
      const box = li.querySelector(':scope > input.task-list-item-checkbox, :scope > p > input.task-list-item-checkbox');
      if (!box || !box.hasAttribute('checked')) return;
      let own = '';
      for (const n of li.childNodes) {
        if (n.nodeType === 1 && (n.tagName === 'UL' || n.tagName === 'OL')) break;
        own += n.textContent;
      }
      if (/:\s*stop\s*$/i.test(own.trim())) li.remove();
    });
  }

  const MEDIA_ICONS = { video: '▶', audio: '♪', image: '', font: 'Aa', transcript: '❝', tape: '›_', demo_script: '⌘', other: '◻' };
  const STATUS_TEXT = { missing: 'Missing', changed: 'Changed', unknown: 'Not in this project' };

  function formatTime(s) {
    s = Math.max(0, s);
    const m = Math.floor(s / 60), r = Math.floor(s % 60);
    return m + ':' + (r < 10 ? '0' : '') + r;
  }

  // `![label](media:<id>)`: a card with a thumbnail, kind, duration and status. Hovering a video
  // scrubs through it; clicking plays the file in Quick Look.
  function mediaCard(id, label) {
    const info = state.media[id] || { kind: 'other', name: id, status: 'unknown', meta: '', frames: 0 };
    const card = document.createElement('span');
    card.className = 'media-card kind-' + info.kind + ' status-' + info.status;
    card.setAttribute('data-media', id);
    card.setAttribute('role', 'button');
    card.setAttribute('tabindex', '0');
    card.title = info.name + (STATUS_TEXT[info.status] ? ' — ' + STATUS_TEXT[info.status] : '');
    const thumb = document.createElement('span');
    thumb.className = 'media-thumb';
    const found = info.status === 'ok' || info.status === 'moved' || info.status === 'changed';
    if (found && info.frames > 0 && (info.kind === 'video' || info.kind === 'image')) {
      const img = document.createElement('img');
      img.setAttribute('src', 'vs-media://thumb/' + encodeURIComponent(id) + '/0');
      img.alt = '';
      img.addEventListener('error', () => { img.remove(); thumb.classList.add('no-picture'); }, { once: true });
      thumb.appendChild(img);
    } else {
      thumb.classList.add('no-picture');
    }
    const icon = document.createElement('span');
    icon.className = 'media-icon';
    icon.textContent = MEDIA_ICONS[info.kind] || '';
    thumb.appendChild(icon);
    if (found && info.duration && info.kind !== 'image') {
      const bar = document.createElement('span');
      bar.className = 'media-scrub';
      const time = document.createElement('span');
      time.className = 'media-time';
      time.textContent = formatTime(info.duration);
      thumb.appendChild(bar);
      thumb.appendChild(time);
      let shown = 0;
      thumb.addEventListener('mousemove', e => {
        const r = thumb.getBoundingClientRect();
        const f = Math.min(0.999, Math.max(0, (e.clientX - r.left) / Math.max(1, r.width)));
        bar.style.width = (f * 100) + '%';
        time.textContent = formatTime(f * info.duration) + ' / ' + formatTime(info.duration);
        const frame = Math.floor(f * info.frames);
        const img = thumb.querySelector('img');
        if (img && info.frames > 1 && frame !== shown) { shown = frame; img.setAttribute('src', 'vs-media://thumb/' + encodeURIComponent(id) + '/' + frame); }
      });
      thumb.addEventListener('mouseleave', () => { bar.style.width = '0'; time.textContent = formatTime(info.duration); });
    }
    card.appendChild(thumb);
    const cap = document.createElement('span');
    cap.className = 'media-caption';
    const name = document.createElement('span');
    name.className = 'media-label';
    name.textContent = label || info.name;
    cap.appendChild(name);
    const meta = document.createElement('span');
    meta.className = 'media-meta';
    meta.textContent = [info.meta, 'media:' + id].filter(Boolean).join(' · ');
    cap.appendChild(meta);
    if (STATUS_TEXT[info.status]) {
      const badge = document.createElement('span');
      badge.className = 'media-badge';
      badge.textContent = STATUS_TEXT[info.status];
      cap.appendChild(badge);
    }
    card.appendChild(cap);
    return card;
  }

  // Images: `media:<id>` becomes a media card; web images load through mdraft-remote only when
  // the setting allows; anything else (local paths, file:, data:) shows as unavailable, because
  // the spec refers to files only through the project's media list.
  function fixHtmlImages(root) {
    root.querySelectorAll('img').forEach(img => {
      const raw = (img.getAttribute('data-raw-src') || img.getAttribute('src') || '').trim();
      img.setAttribute('data-raw-src', raw);
      img.removeAttribute('srcset');
      const m = /^media:([A-Za-z0-9_-]+)$/.exec(raw);
      if (m) {
        img.replaceWith(mediaCard(m[1], img.getAttribute('alt') || ''));
        return;
      }
      if (!(state.remoteImages && /^https?:\/\//i.test(raw))) {
        const ph = document.createElement('span');
        ph.className = 'image-unavailable';
        ph.textContent = /^https?:\/\//i.test(raw) ? '[remote image: turn on Load Remote Images to show it]' : '[image unavailable: add the file to the project and use media:<id>]';
        ph.title = raw;
        img.replaceWith(ph);
        return;
      }
      img.setAttribute('src', 'mdraft-remote://fetch/' + encodeURIComponent(raw));
      img.addEventListener('error', () => {
        const ph = document.createElement('span');
        ph.className = 'broken-image';
        ph.textContent = '⚠︎ Image not found: ' + raw;
        img.replaceWith(ph);
      }, { once: true });
    });
  }

  async function runMermaid(root, id) {
    const nodes = root.querySelectorAll('pre.mermaid');
    if (!nodes.length || !window.mermaid) return;
    try {
      mermaid.initialize({ startOnLoad: false, theme: state.theme === 'dark' ? 'dark' : 'default', securityLevel: 'strict', fontFamily: '-apple-system, BlinkMacSystemFont, sans-serif' });
      await mermaid.run({ nodes: Array.from(nodes), suppressErrors: true });
    } catch (e) {
      console.error(e);
    }
    if (id === state.renderId) post({ type: 'rendered' });
  }

  function applyTheme(theme) {
    state.theme = theme;
    document.getElementById('css-light').disabled = theme === 'dark';
    document.getElementById('css-dark').disabled = theme !== 'dark';
    document.getElementById('hl-light').disabled = theme === 'dark';
    document.getElementById('hl-dark').disabled = theme !== 'dark';
    document.documentElement.style.colorScheme = theme;
    document.documentElement.style.background = theme === 'dark' ? '#0d1117' : '#ffffff';
    document.body.style.background = theme === 'dark' ? '#0d1117' : '#ffffff';
  }

  window.MD = {
    render: function (p) {
      const t0 = performance.now();
      state.media = p.media || {};
      state.noteFolder = p.noteFolder || '';
      state.remoteImages = !!p.remoteImages;
      state.copyButtons = p.copyButtons !== false;
      state.islandOutlines = !!p.islandOutlines;
      state.wiki = {};
      Object.keys(p.wiki || {}).forEach(k => { state.wiki[k.toLowerCase()] = p.wiki[k]; });
      state.showFrontMatter = !!p.showFrontMatter;
      applyTheme(p.theme || 'light');
      document.getElementById('custom-css').textContent = p.customCSS || '';
      const { body, meta } = splitFrontMatter(p.text || '');
      let html = md.render(body);
      if (meta !== null && state.showFrontMatter) {
        html = '<details class="frontmatter" data-line="0"><summary>Metadata</summary><pre>' + escapeHtml(meta) + '</pre></details>' + html;
      }
      const root = document.getElementById('content');
      state.renderId += 1;
      const id = state.renderId;
      // Treat Markdown HTML as untrusted, including when it is later exported.
      // Keep SVG/MathML for callout icons and KaTeX, and file: URLs for local images.
      root.innerHTML = DOMPurify.sanitize(html, {
        FORBID_TAGS: ['style', 'link', 'iframe', 'object', 'embed', 'form', 'base'],
        ALLOWED_URI_REGEXP: /^(?:(?:(?:f|ht)tps?|file|mailto|tel|callto|sms|cid|xmpp|matrix):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i
      });
      fixHtmlImages(root);
      if (p.hideStopped) hideStoppedTodos(root);
      decorateTodos(root);
      decorateDueDates(root);
      decorateAutoTags(root);
      if (typeof p.scrollLine === 'number') MD.scrollToLine(p.scrollLine, false);
      const hasMermaid = root.querySelector('pre.mermaid');
      if (hasMermaid) {
        runMermaid(root, id).then(() => {
          if (typeof p.scrollLine === 'number') MD.scrollToLine(p.scrollLine, false);
        });
      } else {
        post({ type: 'rendered' });
      }
      return { ms: performance.now() - t0, mermaid: !!hasMermaid };
    },

    scrollToLine: function (line, flash) {
      const els = Array.from(document.querySelectorAll('[data-line]'));
      let best = null;
      for (const el of els) {
        const l = parseInt(el.getAttribute('data-line'), 10);
        if (isNaN(l)) continue;
        if (l <= line) best = el; else break;
      }
      if (!best) { window.scrollTo(0, 0); return; }
      const top = best.getBoundingClientRect().top + window.scrollY - 12;
      window.scrollTo(0, Math.max(0, top));
      if (flash) { best.classList.remove('flash'); void best.offsetWidth; best.classList.add('flash'); }
    },

    topLine: function () {
      const els = document.querySelectorAll('[data-line]');
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.bottom > 8) return parseInt(el.getAttribute('data-line'), 10) || 0;
      }
      return 0;
    },

    scrollToAnchor: function (id) {
      const el = document.getElementById(id) || document.getElementById(slugify(id));
      if (el) { el.scrollIntoView({ block: 'start' }); el.classList.add('flash'); return true; }
      return false;
    },

    /* Renders and waits for Mermaid diagrams; resolves to the body HTML (used by export). */
    renderAsync: async function (p) {
      MD.render(p);
      const root = document.getElementById('content');
      if (root.querySelector('pre.mermaid:not([data-processed])')) await runMermaid(root, state.renderId);
      await Promise.all(Array.from(root.querySelectorAll('img')).map(img => img.complete ? null : new Promise(r => { img.onload = img.onerror = r; })));
      return readOnlyTasks(root.innerHTML);
    },

    bodyHTML: function () {
      return readOnlyTasks(document.getElementById('content').innerHTML);
    }
  };

  // ---------- Clicks ----------
  document.addEventListener('click', function (e) {
    const box = e.target.closest('input.task-list-item-checkbox');
    if (box) {
      // The app toggles the source line and re-renders; don't flip the box here.
      e.preventDefault();
      const li = box.closest('li[data-line]');
      if (li) post({ type: 'task', line: parseInt(li.getAttribute('data-line'), 10) });
      return;
    }
    const card = e.target.closest('.media-card');
    if (card) {
      e.preventDefault();
      post({ type: 'media', id: card.getAttribute('data-media') });
      return;
    }
    const img = e.target.closest('img');
    if (img && !e.target.closest('a')) {
      e.preventDefault();
      post({ type: 'image', src: img.getAttribute('src'), raw: img.getAttribute('data-raw-src') || '' });
      return;
    }
    const a = e.target.closest('a');
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute('href') || '';
    if (href.startsWith('#')) {
      const id = decodeURIComponent(href.slice(1));
      if (!MD.scrollToAnchor(id)) {
        // Footnote ids may need exact match.
        const el = document.getElementById(id);
        if (el) el.scrollIntoView({ block: 'center' });
      }
      return;
    }
    post({ type: 'link', href: href });
  }, true);

  let scrollTimer = null;
  window.addEventListener('scroll', () => {
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => post({ type: 'scroll', line: MD.topLine() }), 120);
  }, { passive: true });

  document.addEventListener('DOMContentLoaded', () => post({ type: 'ready' }));
})();
