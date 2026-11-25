// The red dot of the remote's laser pointer. The phone sends movements; the dot starts in the middle each time.
(function () {
  'use strict';
  var dot = document.getElementById('dot'), x = innerWidth / 2, y = innerHeight / 2;
  window.__TAURI__.event.listen('laser', function (e) {
    var p = e.payload;
    if (!p.on) { x = innerWidth / 2; y = innerHeight / 2; }
    else {
      x = Math.max(0, Math.min(innerWidth, x + p.x));
      y = Math.max(0, Math.min(innerHeight, y + p.y));
    }
    dot.style.left = x + 'px';
    dot.style.top = y + 'px';
  });
})();
