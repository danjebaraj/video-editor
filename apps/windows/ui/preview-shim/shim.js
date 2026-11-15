// WebView2 shim for the macOS preview page (apps/macos/Resources/Preview), which is used as is.
// - The page posts to `window.webkit.messageHandlers.md`; here that goes to the window that
//   hosts the preview iframe (same origin).
// - The page asks for media stills as `vs-media://thumb/<id>/<n>`; here they come from the media
//   host, which the window maps from the media id (`window.parent.__vsThumb(id, n)`).
(function () {
  'use strict';
  window.webkit = {
    messageHandlers: {
      md: {
        postMessage: function (msg) {
          try { window.parent.postMessage({ __vsPreview: true, msg: msg }, location.origin); } catch (e) { /* detached */ }
        },
      },
    },
  };
  var setAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {
    if (name === 'src' && typeof value === 'string') {
      if (value.indexOf('vs-media://thumb/') === 0) {
        var rest = value.slice('vs-media://thumb/'.length).split('/');
        var mapped = '';
        try { mapped = window.parent.__vsThumb(decodeURIComponent(rest[0]), Number(rest[1] || 0)) || ''; } catch (e) { /* no host */ }
        if (mapped) { value = mapped; } else { return; }
      } else if (value.indexOf('mdraft-remote://') === 0) {
        // Remote images aren't loaded in the Windows preview (the macOS default too).
        return;
      }
    }
    return setAttribute.call(this, name, value);
  };
  window.addEventListener('error', function (e) {
    window.webkit.messageHandlers.md.postMessage({ type: 'jserror', message: String(e.message) });
  });
})();
