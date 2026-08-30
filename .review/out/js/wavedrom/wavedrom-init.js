// WaveDrom renders on an explicit call, so this runs it, then paints it.
//
// Why the palette is injected here rather than written in main.scss: two of
// WaveDrom's outputs are out of reach of an external stylesheet.
//
//   1. The panel background is `<rect style="stroke:none;fill:white">` - an
//      inline style attribute, which no external rule can override.
//   2. The coloured data blocks are drawn with `<use xlink:href="#vvv-3">`
//      pointing into <defs>. Styling shadow-cloned content from outside the
//      SVG is unreliable across browsers.
//
// Injecting a <style> element INSIDE each generated SVG sidesteps both. The
// values still come from the site's CSS variables, so the diagram follows the
// theme toggle.
(function () {
  'use strict';

  if (typeof WaveDrom === 'undefined') return;
  if (!document.querySelector('script[type="WaveDrom"]')) return;

  function cssVar(name, fallback) {
    var v = getComputedStyle(document.documentElement).getPropertyValue(name);
    return (v && v.trim()) || fallback;
  }

  function tint(colour, pct, base) {
    return 'color-mix(in srgb, ' + colour + ' ' + pct + '%, ' + base + ')';
  }

  function themeCss() {
    var ink = cssVar('--ink', '#14110d');
    var surf = cssVar('--surf', '#f6f1e7');
    var accent = cssVar('--accent', '#c04a18');
    var accent2 = cssVar('--accent-2', '#dd9b1f');
    var mid = cssVar('--mid', '#5b5044');
    var mono = cssVar('--mono', 'ui-monospace, monospace');

    return [
      // the panel background, which is an inline style on a bare rect
      'rect[style]{fill:' + surf + ' !important;stroke:none !important}',
      // wave lines, grid and dividers
      '.s1,.s2,.s3,.s4{stroke:' + ink + ' !important}',
      // the white fills inside the skin
      '.s5,.s7{fill:' + surf + ' !important}',
      // solid fills: markers and arrowheads
      '.s6{fill:' + ink + ' !important}',
      // signal names and data labels
      'text{fill:' + ink + ' !important;font-family:' + mono +
        ' !important;font-size:11px !important}',
      // the six data-block colours, retinted low-saturation so the label on
      // top stays readable in both themes
      '.s8{fill:' + tint(accent2, 26, surf) + ' !important}',
      '.s9{fill:' + tint(accent, 20, surf) + ' !important}',
      '.s10{fill:' + tint(mid, 18, surf) + ' !important}',
      '.s11{fill:' + tint(accent2, 13, surf) + ' !important}',
      '.s12{fill:' + tint(accent, 34, surf) + ' !important}',
      '.s13{fill:' + tint(mid, 30, surf) + ' !important}'
    ].join('');
  }

  function paint() {
    var svgs = document.querySelectorAll('.diagram--wave svg');
    var css = themeCss();

    Array.prototype.forEach.call(svgs, function (svg) {
      var style = svg.querySelector('style[data-theme-paint]');
      if (!style) {
        style = document.createElementNS('http://www.w3.org/2000/svg', 'style');
        style.setAttribute('data-theme-paint', '');
        // Last child, so it wins on order as well as on !important.
        svg.appendChild(style);
      }
      style.textContent = css;
    });
  }

  function draw() {
    try {
      WaveDrom.ProcessAll();
      paint();
    } catch (e) {
      if (window.console) console.error('wavedrom:', e);
    }
  }

  draw();

  // The toggle writes data-theme on <html>; re-render and repaint.
  new MutationObserver(function (records) {
    for (var i = 0; i < records.length; i++) {
      if (records[i].attributeName === 'data-theme') { draw(); return; }
    }
  }).observe(document.documentElement, { attributes: true });

  // And follow the OS while no explicit choice has been made.
  var media = window.matchMedia('(prefers-color-scheme: dark)');
  if (media.addEventListener) {
    media.addEventListener('change', function () {
      if (!document.documentElement.getAttribute('data-theme')) draw();
    });
  }
})();
