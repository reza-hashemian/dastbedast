(function () {
  'use strict';

  // ------------------------------------------------------------------ bridge to the core
  // Inside the desktop app commands go through Tauri; in a browser they go to the local daemon.
  var T = window.__TAURI__;
  var TOKEN = location.hash.slice(1);

  function call(cmd, args) {
    args = args || {};
    if (T) return T.core.invoke('call', { cmd: cmd, args: args });
    return fetch('/api/call', {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-dbd-token': TOKEN },
      body: JSON.stringify({ cmd: cmd, args: args })
    }).then(function (r) { return r.json(); }).then(function (j) {
      if (!j.ok) throw new Error(j.error);
      return j.data;
    });
  }

  function listen(handler) {
    if (T) { T.event.listen('core', function (e) { handler(e.payload); }); return; }
    var es = new EventSource('/api/events?t=' + encodeURIComponent(TOKEN));
    es.onmessage = function (e) { handler(JSON.parse(e.data)); };
    // After a reconnect the page may have missed events.
    es.onopen = function () { handler({ type: 'changed' }); };
  }

  // ------------------------------------------------------------------ languages
  // Every text of the UI: [Persian, English]. {0}, {1} are filled in by t().
  var L = {
    brand: ['دست‌به‌دست', 'DastBeDast'],
    other_lang: ['English', 'فارسی'],
    settings_title: ['تنظیمات این دستگاه', 'Settings of this device'],
    networks: ['شبکه‌ها', 'Networks'],
    newconn: ['اتصال جدید', 'New connection'],
    newconn_btn: ['+ اتصال جدید', '+ New connection'],
    devices: ['دستگاه‌ها', 'Devices'],
    transfers: ['انتقال‌ها', 'Transfers'],
    clear_done: ['پاک‌کردن تمام‌شده‌ها', 'Clear finished'],
    accept_here: ['پذیرش فایل روی این دستگاه', 'Accepting files on this device'],
    accept_mode: ['حالت پذیرش', 'Accept mode'],
    mode_ask: ['با تأیید', 'Ask first'],
    mode_auto: ['بدون تأیید', 'Without asking'],
    mode_default: ['پیش‌فرض', 'Default'],
    inbox: ['صندوق دریافت', 'Inbox'],
    folder_colon: ['پوشه:', 'Folder:'],
    close: ['بستن', 'Close'],
    nearby: ['دستگاه‌های نزدیک', 'Nearby devices'],
    scan: ['جست‌وجوی شبکه', 'Scan the network'],
    scanning: ['در حال جست‌وجو…', 'Scanning…'],
    scan_hint: ['برای شبکه‌هایی که کشف خودکار را می‌بندند.', 'For networks that block automatic discovery.'],
    scan_found: ['{0} دستگاه پیدا شد', '{0} device(s) found'],
    scan_none: ['دستگاهی پیدا نشد', 'No device found'],
    by_addr: ['با آدرس', 'By address'],
    addr_ph: ['192.168.1.20 یا home.example.com', '192.168.1.20 or home.example.com'],
    address: ['آدرس', 'Address'],
    port: ['پورت', 'Port'],
    connect: ['وصل شو', 'Connect'],
    static_check: ['این آدرس ثابت است؛ از هر شبکه‌ای به آن وصل شو', 'This address is static; connect to it from any network'],
    guest: ['مهمان مرورگر', 'Browser guest'],
    paths_hint: ['در حالت مرورگر پنجرهٔ انتخاب فایل در دسترس نیست. مسیر کامل هر فایل یا پوشه را در یک خط بنویس.',
      'The file picker is not available in browser mode. Write the full path of each file or folder on its own line.'],
    paths_files: ['مسیر فایل‌ها', 'File paths'],
    paths_folder: ['مسیر پوشه', 'Folder path'],
    ok: ['تأیید', 'OK'],
    sure: ['مطمئنی؟', 'Sure?'],
    saved: ['ذخیره شد', 'Saved'],
    save: ['ذخیره', 'Save'],
    file: ['فایل', 'File'],
    folder: ['پوشه', 'Folder'],
    n_files: ['{0} فایل', '{0} files'],
    n_devices: ['{0} دستگاه', '{0} device(s)'],
    units: [['بایت', 'کیلوبایت', 'مگابایت', 'گیگابایت', 'ترابایت'], ['B', 'KB', 'MB', 'GB', 'TB']],
    per_sec: ['{0} بر ثانیه', '{0}/s'],
    just_now: ['همین الان', 'just now'],
    min_ago: ['{0} دقیقه پیش', '{0} min ago'],
    hour_ago: ['{0} ساعت پیش', '{0} h ago'],
    day_ago: ['{0} روز پیش', '{0} d ago'],
    sec_left: ['{0} ثانیه مانده', '{0} s left'],
    min_left: ['{0} دقیقه مانده', '{0} min left'],
    hour_left: ['{0} ساعت مانده', '{0} h left'],
    of: ['{0} از {1}', '{0} of {1}'],
    kind_linux: ['لینوکس', 'Linux'], kind_windows: ['ویندوز', 'Windows'], kind_android: ['اندروید', 'Android'], kind_macos: ['مک', 'macOS'],

    no_net: ['به هیچ شبکه‌ای وصل نیستی', 'Not connected to any network'],
    net_known: ['{0} · خودکار شناخته شد', '{0} · recognised'],
    net_unknown: ['شبکهٔ ناشناس', 'Unknown network'],
    net_save_it: ['ذخیره‌اش کن', 'Save it'],
    here_now: ['الان اینجایی', 'you are here'],
    nets_empty: ['هنوز شبکه‌ای ذخیره نشده. با اولین جفت‌سازی، شبکهٔ فعلی خودکار ذخیره می‌شود.', 'No saved network yet. The current network is saved automatically with your first pairing.'],
    devs_empty: ['هنوز دستگاهی جفت نشده. روی هر دو دستگاه «اتصال جدید» را بزن.', 'No paired device yet. Press “New connection” on both devices.'],
    connected: ['وصل', 'Connected'],
    unreachable: ['در دسترس نیست', 'Not reachable'],
    not_here: ['در این شبکه نیست', 'Not on this network'],
    send_to: ['فرستادن فایل به این دستگاه', 'Send files to this device'],
    settings_of: ['تنظیمات {0}', 'Settings of {0}'],

    to_peer: ['به «{0}»', 'to “{0}”'],
    from_peer: ['از «{0}»', 'from “{0}”'],
    st_connecting: ['در حال اتصال', 'connecting'],
    st_asking: ['منتظر تأیید گیرنده', 'waiting for the receiver to accept'],
    st_waiting: ['قطع شد؛ با برگشتن دستگاه خودکار ادامه پیدا می‌کند', 'interrupted; continues by itself when the device is back'],
    transfers_empty: ['برای فرستادن، روی یک دستگاهِ وصل بزن.', 'To send, press a connected device.'],
    cancel: ['لغو', 'Cancel'],
    sent: ['فرستاده شد', 'Sent'], arrived: ['رسید', 'Received'], rejected: ['رد شد', 'Declined'], cancelled: ['لغو شد', 'Cancelled'], failed: ['ناموفق', 'Failed'],

    hint_auto: ['فایل دستگاه‌های جفت‌شده بدون پرسش وارد صندوق می‌شود.', 'Files from paired devices go into the inbox without asking.'],
    hint_auto_max: ['فایل دستگاه‌های جفت‌شده تا {0} بدون پرسش وارد صندوق می‌شود.', 'Files from paired devices up to {0} go into the inbox without asking.'],
    hint_ask: ['قبل از دریافت هر فایل از تو پرسیده می‌شود.', 'You are asked before every file is received.'],
    hint_net: ['در این شبکه: {0}.', 'On this network: {0}.'],
    offer_one: ['<b>{0}</b> می‌خواهد «{1}» را بفرستد ({2}).', '<b>{0}</b> wants to send “{1}” ({2}).'],
    accept: ['قبول', 'Accept'], decline: ['رد', 'Decline'],
    missing: ['فایل دیگر سر جایش نیست', 'The file is no longer there'],
    from: ['از {0}', 'from {0}'],
    kept: ['نگه داشته شد', 'kept'],
    keep: ['نگه دار', 'Keep'], open: ['باز کن', 'Open'], del: ['حذف', 'Delete'], unlist: ['حذف از فهرست', 'Remove from list'], show_folder: ['نمایش پوشه', 'Show folder'],
    inbox_empty: ['صندوق خالی است. فایل‌های رسیده اینجا می‌نشینند تا تصمیم بگیری.', 'The inbox is empty. Received files wait here until you decide.'],

    board: ['میز مشترک', 'Shared board'],
    board_ph: ['متن یا لینک برای همهٔ دستگاه‌ها…', 'Text or a link for all your devices…'],
    put: ['بگذار', 'Put'],
    board_empty: ['میز خالی است. هر چه اینجا بگذاری روی همهٔ دستگاه‌هایت دیده می‌شود.', 'The board is empty. Whatever you put here shows up on all your devices.'],
    this_device: ['این دستگاه', 'this device'],
    b_own: ['از همین‌جا در دسترس بقیه است', 'available to the others from here'],
    b_here: ['روی این دستگاه هست', 'on this device'],
    b_fetching: ['در حال دریافت…', 'fetching…'],
    b_sources: ['آماده روی {0}', 'available from {0}'],
    b_nosource: ['فعلاً هیچ دستگاهِ وصلی آن را ندارد', 'no connected device has it right now'],
    get: ['بگیر', 'Get'], copy: ['کپی', 'Copy'], copied: ['کپی شد', 'Copied'], take_off: ['بردار', 'Take off'],
    board_got: ['«{0}» از میز مشترک گرفته شد', '“{0}” was fetched from the shared board'],
    drop_hint: ['فایل را روی یکی از دستگاه‌ها یا روی میز مشترک رها کن', 'Drop the file on a device or on the shared board'],

    pair_with: ['جفت‌سازی با «{0}»', 'Pairing with “{0}”'],
    pair_hint: ['این کد را با کدی که روی دستگاه دیگر می‌بینی مقایسه کن. فقط اگر یکی بودند تأیید کن.', 'Compare this code with the one on the other device. Confirm only if they are the same.'],
    pair_wait: ['منتظر تأیید روی دستگاه دیگر…', 'Waiting for confirmation on the other device…'],
    pair_no: ['یکی نیست، رد کن', 'Not the same, reject'],
    pair_yes: ['یکی است، جفت کن', 'Same, pair'],
    pair_btn: ['جفت کن', 'Pair'],
    new_status: ['تا وقتی این پنجره باز است، دستگاه‌های دیگر می‌توانند به این دستگاه درخواست جفت‌شدن بفرستند.', 'While this window is open, other devices can ask to pair with this one.'],
    nearby_empty: ['دستگاهی دیده نمی‌شود. روی دستگاه دیگر هم «اتصال جدید» را باز کن.', 'No device in sight. Open “New connection” on the other device too.'],
    static_hint: ['آدرس این دستگاه: {0}. برای اتصال از بیرون، پورت TCP {1} را روی روتر به این دستگاه فوروارد کن و روی دستگاه دیگر، آی‌پی ثابتِ اینترنتت را با تیک بالا وارد کن.',
      'This device’s address: {0}. To connect from outside, forward TCP port {1} on your router to this device, and on the other device enter your static internet IP with the box above ticked.'],
    guest_on: ['روشن کن', 'Turn on'], guest_off: ['خاموش کن', 'Turn off'],
    guest_hint: ['دستگاهی که برنامه را ندارد با یک لینک در مرورگر فایل می‌فرستد و می‌گیرد.', 'A device without the app sends and receives files through a link in its browser.'],
    guest_warn: ['هر کس این لینک را روی همین شبکه داشته باشد می‌تواند به صندوق تو فایل بفرستد.', 'Anyone on this network who has this link can send files to your inbox.'],
    guest_shared: ['برای مهمان گذاشته‌ای: {0}', 'Put out for the guest: {0}'],
    guest_share: ['گذاشتن فایل برای مهمان', 'Put out a file for the guest'],
    enter_addr: ['آدرس را وارد کن', 'Enter the address'],

    net_deleted: ['شبکهٔ حذف‌شده', 'a removed network'],
    connected_from: ['وصل از {0}', 'connected from {0}'],
    ident: ['شناسه', 'ID'],
    accept_from_peer: ['پذیرش فایل از این دستگاه', 'Accepting files from this device'],
    addresses: ['آدرس‌ها', 'Addresses'],
    in_net: ['در {0}', 'on {0}'],
    static_any: ['ثابت؛ از هر شبکه', 'static; from any network'],
    no_addrs: ['آدرسی ذخیره نشده. این دستگاه فقط وقتی دیده می‌شود که خودش به تو وصل شود.', 'No saved address. This device is seen only when it connects to you.'],
    addr_or_domain: ['آدرس یا دامنه', 'Address or domain'],
    add: ['افزودن', 'Add'],
    static_try: ['آدرس ثابت است؛ از هر شبکه‌ای امتحانش کن', 'The address is static; try it from any network'],
    unpair: ['حذف این اتصال', 'Remove this connection'],
    net_save_title: ['ذخیرهٔ این شبکه', 'Save this network'],
    network: ['شبکه', 'Network'],
    name: ['اسم', 'Name'],
    net_name_ph: ['مثلاً خانه', 'e.g. Home'],
    net_save_hint: ['دفعهٔ بعد که به این شبکه وصل شوی خودکار شناخته می‌شود و دستگاه‌هایت پیدا می‌شوند.', 'Next time you join this network it is recognised and your devices are found.'],
    accept_in_net: ['پذیرش فایل در این شبکه', 'Accepting files on this network'],
    forget: ['فراموش کن', 'Forget'],
    set_name: ['اسم دستگاه', 'Device name'],
    set_inbox: ['صندوق دریافت', 'Inbox folder'],
    set_keep: ['پوشهٔ «نگه دار»', '“Keep” folder'],
    set_board: ['پوشهٔ فایل‌های میز مشترک', 'Folder for shared-board files'],
    set_max: ['سقف پذیرش بدون تأیید، به گیگابایت (۰ یعنی بدون سقف)', 'Largest transfer accepted without asking, in GB (0 = no limit)'],
    set_port: ['پورت (بعد از اجرای دوبارهٔ برنامه اعمال می‌شود)', 'Port (applies after the app restarts)'],
    set_always: ['این دستگاه همیشه روشن است', 'This device is always on'],
    set_always_hint: ['هر چه روی میز مشترک گذاشته شود خودکار اینجا هم ذخیره می‌شود، تا وقتی دستگاهِ صاحبش خاموش است بقیه از اینجا بگیرند.',
      'Everything put on the shared board is also stored here automatically, so the others can get it from here while its owner is off.'],
    cur_addr: ['آدرس فعلی:', 'Current address:'],
    no_core: ['به هستهٔ برنامه وصل نشد: {0}', 'Could not reach the app core: {0}'],
    ev_received: ['«{0}» از {1} رسید', '“{0}” arrived from {1}'],
    ev_incoming: ['{0} می‌خواهد فایل بفرستد', '{0} wants to send files']
  };
  var lang = 'fa', langApplied = false;
  function t(key) {
    var s = (L[key] || [key, key])[lang === 'en' ? 1 : 0], a = arguments;
    return typeof s !== 'string' ? s : s.replace(/\{(\d)\}/g, function (_, i) { return a[+i + 1]; });
  }
  function applyLang() {
    var root = document.documentElement;
    root.lang = lang; root.dir = lang === 'en' ? 'ltr' : 'rtl';
    document.title = t('brand');
    [['data-t', 'textContent'], ['data-t-ph', 'placeholder'], ['data-t-title', 'title'], ['data-t-aria', 'aria-label']].forEach(function (m) {
      Array.prototype.forEach.call(document.querySelectorAll('[' + m[0] + ']'), function (el) {
        var v = t(el.getAttribute(m[0]));
        if (m[1] === 'textContent') el.textContent = v; else el.setAttribute(m[1], v);
      });
    });
    $('lang').textContent = t('other_lang');
    try { if (T && T.window) T.window.getCurrentWindow().setTitle(t('brand')); } catch (e) { /* the page title is enough */ }
  }

  // ------------------------------------------------------------------ helpers
  var S = null;
  function $(id) { return document.getElementById(id); }
  function esc(s) { return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]; }); }
  function num(n) { return lang === 'en' ? String(n) : String(n).replace(/\d/g, function (d) { return '۰۱۲۳۴۵۶۷۸۹'[d]; }); }
  // Joins the parts of a status line. Each part is isolated, so a Latin name inside Persian text cannot reorder its neighbours.
  function dots(parts) { return parts.filter(Boolean).map(function (p) { return '<bdi>' + p + '</bdi>'; }).join(' · '); }
  function ltr(s) { return '<bdi class="ltr">' + esc(s) + '</bdi>'; }
  function size(n) {
    var u = t('units'), i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return num(i ? n.toFixed(n < 10 ? 1 : 0) : n) + ' ' + u[i];
  }
  function ago(ts) {
    var s = Math.max(0, Date.now() / 1000 - ts);
    if (s < 60) return t('just_now');
    if (s < 3600) return t('min_ago', num(Math.floor(s / 60)));
    if (s < 86400) return t('hour_ago', num(Math.floor(s / 3600)));
    return t('day_ago', num(Math.floor(s / 86400)));
  }
  function eta(x) {
    if (!x.speed || x.done >= x.total) return '';
    var s = (x.total - x.done) / x.speed;
    if (s < 60) return t('sec_left', num(Math.ceil(s)));
    if (s < 3600) return t('min_left', num(Math.ceil(s / 60)));
    return t('hour_left', num((s / 3600).toFixed(1)));
  }
  function toast(msg, kind) {
    var el = document.createElement('div');
    el.className = 'toast' + (kind === 'err' ? ' err' : '');
    el.textContent = msg;
    $('toasts').appendChild(el);
    setTimeout(function () { el.remove(); }, kind === 'err' ? 6000 : 3500);
  }
  function run(promise) { return Promise.resolve(promise).catch(function (e) { toast(e && e.message ? e.message : String(e), 'err'); }); }
  function mode(m) { return t('mode_' + m); }
  function kind(k) { return L['kind_' + k] ? t('kind_' + k) : esc(k); }
  function sep() { return lang === 'en' ? ', ' : '، '; }
  function isLink(s) { return /^https?:\/\/\S+$/.test(s); }
  function xBtn() { return '<button class="x" value="close" aria-label="' + esc(t('close')) + '">×</button>'; }
  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) return navigator.clipboard.writeText(text).catch(function () { legacyCopy(text); });
    legacyCopy(text);
    return Promise.resolve();
  }
  function legacyCopy(text) {
    var ta = document.createElement('textarea');
    ta.value = text; ta.style.position = 'fixed'; ta.style.opacity = '0';
    document.body.appendChild(ta); ta.select();
    try { document.execCommand('copy'); } finally { ta.remove(); }
  }

  var ICON = {
    laptop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="5" width="16" height="11" rx="1.5"/><path d="M2 19h20"/></svg>',
    phone: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="7" y="3" width="10" height="18" rx="2"/><path d="M11 18h2"/></svg>',
    desktop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="1.5"/><path d="M9 20h6M12 16v4"/></svg>'
  };
  function icon(k) { return ICON[k === 'android' ? 'phone' : k === 'windows' ? 'desktop' : 'laptop']; }

  // ------------------------------------------------------------------ rendering
  function renderBar() {
    var n = S.network, chip = $('netchip');
    if (!n) {
      chip.className = 'netchip unknown';
      chip.innerHTML = '<span class="dot"></span>' + esc(t('no_net'));
    } else if (n.saved) {
      chip.className = 'netchip';
      chip.innerHTML = '<span class="dot on"></span>' + esc(t('net_known', n.name));
    } else {
      chip.className = 'netchip unknown';
      chip.innerHTML = '<span class="dot warn"></span>' + esc(t('net_unknown')) + ' <button class="link" type="button" data-act="save-net">' + esc(t('net_save_it')) + '</button>';
    }
    $('me').textContent = S.me.name;
  }

  function renderNets() {
    $('nets').innerHTML = S.networks.length ? S.networks.map(function (n) {
      return '<button type="button" class="net' + (n.current ? ' current' : '') + '" data-act="net" data-id="' + esc(n.id) + '"><b>' + esc(n.name) + '</b><span>' +
        esc(t('n_devices', num(n.devices))) + (n.current ? ' · ' + esc(t('here_now')) : '') + '</span></button>';
    }).join('') : '<p class="hint">' + esc(t('nets_empty')) + '</p>';
  }

  function renderDevs() {
    if (!S.peers.length) {
      $('devs').innerHTML = '<p class="empty" style="grid-column:1/-1">' + esc(t('devs_empty')) + '</p>';
      return;
    }
    $('devs').innerHTML = S.peers.map(function (p) {
      var st = p.online ? esc(t('connected')) + ' · ' + ltr((p.addr || '').replace(/:\d+$/, '')) : esc(t(p.here ? 'unreachable' : 'not_here'));
      var dis = p.online ? '' : ' disabled';
      var id = esc(p.id);
      return '<div class="dev' + (p.online ? '' : ' away') + '" data-peer="' + id + '">' +
        '<button type="button" class="dev-main" data-act="send-files" data-id="' + id + '"' + dis + ' title="' + esc(t('send_to')) + '">' + icon(p.kind) +
        '<b>' + esc(p.name) + '</b><span class="st"><span class="dot' + (p.online ? ' on' : '') + '"></span><span>' + st + '</span></span></button>' +
        '<div class="dev-acts"><button type="button" data-act="send-files" data-id="' + id + '"' + dis + '>' + esc(t('file')) + '</button>' +
        '<button type="button" data-act="send-folder" data-id="' + id + '"' + dis + '>' + esc(t('folder')) + '</button>' +
        '<button type="button" class="more" data-act="peer" data-id="' + id + '" aria-label="' + esc(t('settings_of', p.name)) + '">⋯</button></div></div>';
    }).join('');
  }

  function renderBoard() {
    $('board').innerHTML = S.board.length ? S.board.map(function (b) {
      var id = esc(b.id), who = [esc(t('from', b.mine ? t('this_device') : b.from)), esc(ago(b.at))];
      var off = '<button type="button" class="btn del" data-act="board-remove" data-confirm="1" data-id="' + id + '">' + esc(t('take_off')) + '</button>';
      if (b.kind === 'text') {
        var link = isLink(b.text);
        return '<div class="row"><span class="nm txt' + (link ? ' link' : '') + '" dir="auto">' + esc(b.text) + '</span><span class="meta">' + dots(who) + '</span><span class="act">' +
          '<button type="button" class="btn go" data-act="board-copy" data-id="' + id + '">' + esc(t('copy')) + '</button>' +
          (link ? '<button type="button" class="btn" data-act="board-link" data-id="' + id + '">' + esc(t('open')) + '</button>' : '') + off + '</span></div>';
      }
      var status = b.state === 'fetching' ? t('b_fetching')
        : b.state === 'away' ? (b.sources.length ? t('b_sources', b.sources.join(sep())) : t('b_nosource'))
        : b.own ? t('b_own') : b.kept ? t('kept') : t('b_here');
      var act = b.state === 'away' ? '<button type="button" class="btn go" data-act="board-get" data-id="' + id + '"' + (b.sources.length ? '' : ' disabled') + '>' + esc(t('get')) + '</button>'
        : b.state === 'fetching' ? ''
        : '<button type="button" class="btn" data-act="board-open" data-id="' + id + '">' + esc(t('open')) + '</button>' +
          (b.own || b.kept ? '<button type="button" class="btn" data-act="board-open" data-folder="1" data-id="' + id + '">' + esc(t('show_folder')) + '</button>'
            : '<button type="button" class="btn go" data-act="board-keep" data-id="' + id + '">' + esc(t('keep')) + '</button>');
      return '<div class="row"><span class="nm">' + esc(b.name) + '</span><span class="meta">' + dots([size(b.size), b.files > 1 && esc(t('n_files', num(b.files)))].concat(who, esc(status))) + '</span><span class="act">' + act + off + '</span></div>';
    }).join('') : '<p class="hint">' + esc(t('board_empty')) + '</p>';
  }

  var RUNNING = { connecting: 1, asking: 1, active: 1, waiting: 1 };
  function transferMeta(x) {
    var who = esc(t(x.outgoing ? 'to_peer' : 'from_peer', x.peer));
    var files = x.count > 1 && esc(t('n_files', num(x.count)));
    var part = esc(t('of', size(x.done), size(x.total)));
    switch (x.state) {
      case 'connecting': return dots([who, files, esc(t('st_connecting'))]);
      case 'asking': return dots([who, files, size(x.total), esc(t('st_asking'))]);
      case 'active': return dots([who, part, x.speed && esc(t('per_sec', size(x.speed))), x.speed && esc(eta(x))]);
      case 'waiting': return dots([who, part, esc(t('st_waiting'))]);
      case 'done': return dots([who, files, size(x.total), esc(ago(x.at))]);
      default: return dots([who, files, esc(x.error)]);
    }
  }
  function renderTransfers() {
    $('clear-transfers').hidden = !S.transfers.some(function (x) { return !RUNNING[x.state]; });
    if (!S.transfers.length) {
      $('transfers').innerHTML = '<p class="hint">' + esc(t('transfers_empty')) + '</p>';
      return;
    }
    $('transfers').innerHTML = S.transfers.map(function (x) {
      var pct = x.total ? Math.min(100, x.done / x.total * 100) : (x.state === 'done' ? 100 : 0);
      var act = RUNNING[x.state] ? '<button type="button" class="btn" data-act="cancel" data-id="' + esc(x.id) + '">' + esc(t('cancel')) + '</button>'
        : x.state === 'done' ? '<span class="pill ok">' + esc(t(x.outgoing ? 'sent' : 'arrived')) + '</span>'
        : x.state === 'rejected' ? '<span class="pill wait">' + esc(t('rejected')) + '</span>'
        : x.state === 'cancelled' ? '<span class="pill">' + esc(t('cancelled')) + '</span>'
        : '<span class="pill bad">' + esc(t('failed')) + '</span>';
      return '<div class="row"><span class="nm">' + esc(x.name) + '</span><span class="meta' + (x.state === 'failed' ? ' bad' : '') + '" id="tm-' + esc(x.id) + '">' + transferMeta(x) +
        '</span><span class="act">' + act + '</span>' +
        (x.state === 'active' || x.state === 'waiting' ? '<span class="prog"><i id="tp-' + esc(x.id) + '" style="width:' + pct.toFixed(1) + '%"></i></span>' : '') + '</div>';
    }).join('');
  }

  function renderInbox() {
    var s = S.settings;
    $('mode-ask').setAttribute('aria-pressed', s.accept_mode === 'ask');
    $('mode-auto').setAttribute('aria-pressed', s.accept_mode === 'auto');
    var hint = s.accept_mode !== 'auto' ? t('hint_ask') : s.auto_max ? t('hint_auto_max', size(s.auto_max)) : t('hint_auto');
    if (S.network && S.network.mode) hint += ' ' + t('hint_net', mode(S.network.mode));
    $('mode-hint').textContent = hint;
    $('inbox-dir').textContent = s.inbox_dir;

    $('offers').innerHTML = S.offers.map(function (o) {
      var what = (o.count > 1 ? t('n_files', num(o.count)) + sep() : '') + size(o.total);
      return '<div class="ask"><span>' + t('offer_one', esc(o.peer), esc(o.name), esc(what)) + '</span>' +
        '<span class="acts"><button type="button" class="btn go" data-act="offer" data-ok="1" data-id="' + esc(o.id) + '">' + esc(t('accept')) + '</button>' +
        '<button type="button" class="btn" data-act="offer" data-id="' + esc(o.id) + '">' + esc(t('decline')) + '</button></span></div>';
    }).join('');

    function btn(cls, act, id, label, extra) { return '<button type="button" class="btn' + cls + '" data-act="' + act + '" data-id="' + id + '"' + (extra || '') + '>' + esc(t(label)) + '</button>'; }
    $('inbox').innerHTML = S.inbox.length ? S.inbox.map(function (i) {
      var id = esc(i.id), kept = i.state === 'kept';
      var meta = i.missing ? esc(t('missing')) : dots([size(i.size), i.files > 1 && esc(t('n_files', num(i.files))), esc(t('from', i.from)), esc(kept ? t('kept') : ago(i.at))]);
      var act = i.missing ? btn('', 'inbox-del', id, 'unlist')
        : kept ? btn('', 'inbox-open', id, 'show_folder', ' data-folder="1"') + btn('', 'inbox-del', id, 'unlist')
        : btn(' go', 'inbox-keep', id, 'keep') + btn('', 'inbox-open', id, 'open') + btn(' del', 'inbox-del', id, 'del', ' data-confirm="1"');
      return '<div class="row"><span class="nm">' + esc(i.name) + '</span><span class="meta">' + meta + '</span><span class="act">' + act + '</span></div>';
    }).join('') : '<p class="hint">' + esc(t('inbox_empty')) + '</p>';
  }

  function renderPair() {
    var dlg = $('dlg-pair'), p = S.pairs[0];
    if (!p) { if (dlg.open) dlg.close(); return; }
    var code = p.sas.slice(0, 3) + ' ' + p.sas.slice(3);
    $('pair-body').innerHTML = '<h2>' + esc(t('pair_with', p.name)) + '</h2>' +
      '<p class="hint">' + esc(t('pair_hint')) + '</p>' +
      '<div class="code">' + esc(code) + '</div>' +
      (p.waiting ? '<p class="center hint">' + esc(t('pair_wait')) + '</p>'
        : '<div class="line end"><button type="button" class="btn big" data-act="pair" data-id="' + esc(p.id) + '">' + esc(t('pair_no')) + '</button>' +
          '<button type="button" class="btn big go" data-act="pair" data-ok="1" data-id="' + esc(p.id) + '">' + esc(t('pair_yes')) + '</button></div>');
    if (!dlg.open) dlg.showModal();
  }

  function renderNew() {
    if (!$('dlg-new').open) return;
    $('new-status').textContent = t('new_status');
    $('nearby').innerHTML = S.nearby.length ? S.nearby.map(function (n) {
      return '<div class="row"><span class="nm">' + esc(n.name) + ' · ' + kind(n.kind) + '</span><span class="meta">' + ltr(n.addr) + '</span>' +
        '<span class="act"><button type="button" class="btn go" data-act="pair-nearby" data-id="' + esc(n.id) + '">' + esc(t('pair_btn')) + '</button></span></div>';
    }).join('') : '<p class="hint">' + esc(t('nearby_empty')) + '</p>';
    $('static-hint').innerHTML = t('static_hint', ltr((S.me.ip || '?') + ':' + S.me.port), ltr(S.me.port));
    var g = S.guest;
    $('guest').innerHTML = !g
      ? '<div class="line"><button type="button" class="btn" data-act="guest-start">' + esc(t('guest_on')) + '</button><span class="hint">' + esc(t('guest_hint')) + '</span></div>'
      : '<div class="dlg" style="padding:0"><div class="qr">' + g.qr + '</div><p class="center">' + ltr(g.url) + '</p>' +
        '<p class="hint center">' + esc(t('guest_warn')) + '</p>' +
        (g.shared.length ? '<p class="hint">' + esc(t('guest_shared', g.shared.join(sep()))) + '</p>' : '') +
        '<div class="line end"><button type="button" class="btn" data-act="guest-share">' + esc(t('guest_share')) + '</button><button type="button" class="btn del" data-act="guest-stop">' + esc(t('guest_off')) + '</button></div></div>';
  }

  var peerOpen = null;
  function modeSeg(act, id, cur) {
    return '<div class="seg" role="group">' + ['default', 'ask', 'auto'].map(function (m) {
      return '<button type="button" data-act="' + act + '" data-id="' + esc(id) + '" data-mode="' + m + '" aria-pressed="' + ((cur || 'default') === m) + '">' + esc(mode(m)) + '</button>';
    }).join('') + '</div>';
  }
  function renderPeer(force) {
    var dlg = $('dlg-peer');
    if (!dlg.open && !force) return;
    var p = S.peers.filter(function (x) { return x.id === peerOpen; })[0];
    if (!p) { if (dlg.open) dlg.close(); return; }
    // Do not redraw under the user's cursor while they type an address.
    if (!force && dlg.contains(document.activeElement) && document.activeElement.tagName === 'INPUT') return;
    var netName = function (id) { var n = S.networks.filter(function (x) { return x.id === id; })[0]; return n ? n.name : t('net_deleted'); };
    $('peer-body').innerHTML = '<div class="dlg-head"><h2>' + esc(p.name) + '</h2>' + xBtn() + '</div>' +
      '<p class="hint">' + kind(p.kind) + ' · ' + (p.online ? t('connected_from', ltr(p.addr)) : esc(t('unreachable'))) + ' · ' + esc(t('ident')) + ' ' + ltr(p.id.slice(0, 12)) + '</p>' +
      '<h3>' + esc(t('accept_from_peer')) + '</h3>' + modeSeg('peer-mode', p.id, p.mode) +
      '<h3>' + esc(t('addresses')) + '</h3><div class="rows">' + (p.addrs.length ? p.addrs.map(function (a) {
        return '<div class="row"><span class="nm">' + ltr(a.host + ':' + a.port) + '</span><span class="meta">' + esc(a.net ? t('in_net', netName(a.net)) : t('static_any')) + '</span>' +
          '<span class="act"><button type="button" class="btn" data-act="addr-del" data-id="' + esc(p.id) + '" data-host="' + esc(a.host) + '" data-port="' + esc(a.port) + '">' + esc(t('del')) + '</button></span></div>';
      }).join('') : '<p class="hint">' + esc(t('no_addrs')) + '</p>') + '</div>' +
      '<div class="line"><input id="pa-host" class="ltr" placeholder="' + esc(t('addr_or_domain')) + '" aria-label="' + esc(t('address')) + '" autocomplete="off"><input id="pa-port" class="ltr port" inputmode="numeric" value="47800" aria-label="' + esc(t('port')) + '">' +
      '<button type="button" class="btn" data-act="addr-add" data-id="' + esc(p.id) + '">' + esc(t('add')) + '</button></div>' +
      '<label class="check"><input type="checkbox" id="pa-anywhere" checked> ' + esc(t('static_try')) + '</label>' +
      '<div class="line end"><button type="button" class="btn del" data-act="unpair" data-confirm="1" data-id="' + esc(p.id) + '">' + esc(t('unpair')) + '</button></div>';
    if (!dlg.open) dlg.showModal();
  }

  function openNet(id, saving) {
    var n = saving ? { id: '', name: (S.network && S.network.suggest) || '', mode: null } : S.networks.filter(function (x) { return x.id === id; })[0];
    if (!n) return;
    $('net-body').innerHTML = '<div class="dlg-head"><h2>' + esc(t(saving ? 'net_save_title' : 'network')) + '</h2>' + xBtn() + '</div>' +
      '<label class="field">' + esc(t('name')) + '<input id="net-name" value="' + esc(n.name) + '" placeholder="' + esc(t('net_name_ph')) + '"></label>' +
      (saving ? '<p class="hint">' + esc(t('net_save_hint')) + '</p>'
        : '<h3>' + esc(t('accept_in_net')) + '</h3>' + modeSeg('net-mode', n.id, n.mode)) +
      '<div class="line end">' + (saving ? '' : '<button type="button" class="btn del" data-act="net-forget" data-confirm="1" data-id="' + esc(n.id) + '">' + esc(t('forget')) + '</button>') +
      '<button type="button" class="btn go" data-act="net-save" data-id="' + esc(n.id) + '">' + esc(t('save')) + '</button></div>';
    $('dlg-net').showModal();
    $('net-name').focus();
  }

  function openSettings() {
    var s = S.settings;
    function field(label, id, value, cls, extra) { return '<label class="field">' + esc(t(label)) + '<input id="' + id + '"' + (cls ? ' class="' + cls + '"' : '') + (extra || '') + ' value="' + esc(value) + '"></label>'; }
    $('settings-body').innerHTML = '<div class="dlg-head"><h2>' + esc(t('settings_title')) + '</h2>' + xBtn() + '</div>' +
      field('set_name', 'set-name', S.me.name) +
      field('set_inbox', 'set-inbox', s.inbox_dir, 'ltr') +
      field('set_keep', 'set-keep', s.keep_dir, 'ltr') +
      field('set_board', 'set-board', s.board_dir, 'ltr') +
      field('set_max', 'set-max', +(s.auto_max / 1073741824).toFixed(2), 'ltr', ' inputmode="decimal"') +
      field('set_port', 'set-port', S.me.config_port, 'ltr', ' inputmode="numeric"') +
      '<label class="check"><input type="checkbox" id="set-always"' + (s.always_on ? ' checked' : '') + '> ' + esc(t('set_always')) + '</label>' +
      '<p class="hint">' + esc(t('set_always_hint')) + '</p>' +
      '<p class="hint">' + esc(t('cur_addr')) + ' ' + ltr((S.me.ip || '?') + ':' + S.me.port) + ' · ' + esc(t('ident')) + ' ' + ltr(S.me.id.slice(0, 12)) + '</p>' +
      '<div class="line end"><button type="button" class="btn go" data-act="settings-save">' + esc(t('save')) + '</button></div>';
    $('dlg-settings').showModal();
  }

  function render() {
    var want = S.settings.lang === 'en' ? 'en' : 'fa';
    if (want !== lang || !langApplied) { lang = want; langApplied = true; applyLang(); }
    renderBar(); renderNets(); renderDevs(); renderBoard(); renderTransfers(); renderInbox(); renderPair(); renderNew(); renderPeer(false);
  }
  function reload() { return call('snapshot').then(function (s) { S = s; render(); }); }

  // ------------------------------------------------------------------ events from the core
  var pending = null;
  function refresh() {
    if (pending) return;
    pending = setTimeout(function () {
      pending = null;
      reload().catch(function () {});
    }, 40);
  }
  function onEvent(ev) {
    if (ev.type === 'changed') return refresh();
    if (ev.type === 'toast') return toast(ev.msg, ev.kind === 'warn' ? 'err' : '');
    if (ev.type === 'received') return toast(t('ev_received', ev.name, ev.from));
    if (ev.type === 'incoming') return toast(t('ev_incoming', ev.from));
    if (ev.type === 'board_got') return toast(t('board_got', ev.name));
    if (ev.type === 'progress' && S) {
      var x = S.transfers.filter(function (y) { return y.id === ev.id; })[0];
      if (!x) return;
      x.done = ev.done; x.speed = ev.speed;
      var bar = $('tp-' + ev.id), meta = $('tm-' + ev.id);
      if (bar) bar.style.width = (x.total ? Math.min(100, x.done / x.total * 100) : 0).toFixed(1) + '%';
      if (meta && x.state === 'active') meta.innerHTML = transferMeta(x);
    }
  }

  // ------------------------------------------------------------------ choosing files
  var pathsResolve = null;
  function pick(folder) {
    if (T) {
      return T.dialog.open({ multiple: !folder, directory: !!folder }).then(function (r) { return !r ? [] : Array.isArray(r) ? r : [r]; });
    }
    return new Promise(function (resolve) {
      pathsResolve = resolve;
      $('paths-title').textContent = t(folder ? 'paths_folder' : 'paths_files');
      $('paths-text').value = '';
      $('dlg-paths').showModal();
      $('paths-text').focus();
    });
  }
  $('dlg-paths').addEventListener('close', function () { if (pathsResolve) { pathsResolve([]); pathsResolve = null; } });
  function send(peer, folder) {
    return pick(folder).then(function (paths) { if (paths.length) return call('send', { peer: peer, paths: paths }); });
  }
  function boardPut(folder) {
    return pick(folder).then(function (paths) { if (paths.length) return call('board_put', { paths: paths }); });
  }
  function boardItem(id) { return S.board.filter(function (b) { return b.id === id; })[0] || {}; }

  // ------------------------------------------------------------------ actions
  var ACTS = {
    'lang': function () { return call('set_lang', { lang: lang === 'en' ? 'fa' : 'en' }).then(reload); },
    'newconn': function () { $('dlg-new').showModal(); renderNew(); return call('pairing_open', { secs: 300 }); },
    'settings': openSettings,
    'save-net': function () { openNet('', true); },
    'net': function (d) { openNet(d.id, false); },
    'net-save': function (d) {
      var name = $('net-name').value;
      return call(d.id ? 'rename_network' : 'save_network', { id: d.id, name: name }).then(function () { $('dlg-net').close(); });
    },
    'net-mode': function (d) { return call('set_network_mode', { id: d.id, mode: d.mode }).then(function () { $('dlg-net').close(); }); },
    'net-forget': function (d) { return call('forget_network', { id: d.id }).then(function () { $('dlg-net').close(); }); },
    'accept': function (d) { return call('set_accept', { mode: d.mode }); },
    'send-files': function (d) { return send(d.id, false); },
    'send-folder': function (d) { return send(d.id, true); },
    'cancel': function (d) { return call('cancel', { id: d.id }); },
    'clear-transfers': function () { return call('clear_transfers'); },
    'offer': function (d) { return call('offer_decide', { id: d.id, ok: !!d.ok }); },
    'inbox-keep': function (d) { return call('inbox_keep', { id: d.id }); },
    'inbox-open': function (d) { return call('inbox_open', { id: d.id, folder: !!d.folder }); },
    'inbox-del': function (d) { return call('inbox_delete', { id: d.id }); },
    'board-text': function () {
      var el = $('board-text'), text = el.value.trim();
      if (!text) { el.focus(); return; }
      return call('board_text', { text: text }).then(function () { el.value = ''; });
    },
    'board-files': function () { return boardPut(false); },
    'board-folder': function () { return boardPut(true); },
    'board-get': function (d) { return call('board_get', { id: d.id }); },
    'board-keep': function (d) { return call('board_keep', { id: d.id }); },
    'board-open': function (d) { return call('board_open', { id: d.id, folder: !!d.folder }); },
    'board-remove': function (d) { return call('board_remove', { id: d.id }); },
    'board-copy': function (d) { return copyText(boardItem(d.id).text || '').then(function () { toast(t('copied')); }); },
    'board-link': function (d) { return call('open_url', { url: boardItem(d.id).text || '' }); },
    'pair': function (d) { return call('pair_decide', { id: d.id, ok: !!d.ok }); },
    'pair-nearby': function (d) { return call('pair_nearby', { id: d.id }); },
    'pair-addr': function () {
      var host = $('addr-host').value.trim();
      if (!host) { toast(t('enter_addr'), 'err'); return; }
      return call('pair_addr', { host: host, port: +$('addr-port').value, anywhere: $('addr-anywhere').checked });
    },
    'scan': function () {
      var b = $('btn-scan'); b.disabled = true; b.textContent = t('scanning');
      return call('scan').then(function (r) { toast(r.found ? t('scan_found', num(r.found)) : t('scan_none')); })
        .finally(function () { b.disabled = false; b.textContent = t('scan'); });
    },
    'guest-start': function () { return call('guest_start'); },
    'guest-stop': function () { return call('guest_stop'); },
    'guest-share': function () { return pick(false).then(function (p) { if (p.length) return call('guest_share', { paths: p }); }); },
    'peer': function (d) { peerOpen = d.id; renderPeer(true); },
    'peer-mode': function (d) { return call('set_peer_mode', { id: d.id, mode: d.mode }).then(reload).then(function () { renderPeer(true); }); },
    'addr-add': function (d) {
      var host = $('pa-host').value.trim();
      if (!host) { toast(t('enter_addr'), 'err'); return; }
      return call('add_peer_addr', { id: d.id, host: host, port: +$('pa-port').value, anywhere: $('pa-anywhere').checked })
        .then(reload).then(function () { renderPeer(true); });
    },
    'addr-del': function (d) {
      return call('remove_peer_addr', { id: d.id, host: d.host, port: +d.port }).then(reload).then(function () { renderPeer(true); });
    },
    'unpair': function (d) { $('dlg-peer').close(); return call('unpair', { id: d.id }); },
    'paths-ok': function () {
      var list = $('paths-text').value.split('\n').map(function (s) { return s.trim(); }).filter(Boolean);
      var done = pathsResolve; pathsResolve = null;
      $('dlg-paths').close();
      if (done) done(list);
    },
    'settings-save': function () {
      var max = Math.max(0, parseFloat($('set-max').value) || 0);
      var jobs = [
        call('set_name', { name: $('set-name').value }),
        call('set_dirs', { inbox_dir: $('set-inbox').value.trim(), keep_dir: $('set-keep').value.trim(), board_dir: $('set-board').value.trim() }),
        call('set_accept', { mode: S.settings.accept_mode, auto_max: Math.round(max * 1073741824) }),
        call('set_always_on', { on: $('set-always').checked })
      ];
      if (+$('set-port').value !== S.me.config_port) jobs.push(call('set_port', { port: +$('set-port').value }));
      return Promise.all(jobs).then(function () { $('dlg-settings').close(); toast(t('saved')); });
    }
  };

  document.addEventListener('click', function (e) {
    var el = e.target.closest('[data-act]');
    if (!el || el.disabled) return;
    var fn = ACTS[el.dataset.act];
    if (!fn) return;
    // Destructive actions need a second click.
    if (el.dataset.confirm && !el.dataset.armed) {
      el.dataset.armed = '1';
      var old = el.textContent;
      el.textContent = t('sure');
      setTimeout(function () { if (el.isConnected) { delete el.dataset.armed; el.textContent = old; } }, 3000);
      return;
    }
    run(fn(el.dataset));
  });
  $('board-text').addEventListener('keydown', function (e) { if (e.key === 'Enter') { e.preventDefault(); run(ACTS['board-text']()); } });

  $('dlg-new').addEventListener('close', function () { run(call('pairing_close')); });
  // The pairing code must be answered, not dismissed with Escape.
  $('dlg-pair').addEventListener('cancel', function (e) { e.preventDefault(); });
  // Keep this device open for pairing for as long as the dialog stays open.
  setInterval(function () { if ($('dlg-new').open) call('pairing_open', { secs: 300 }).catch(function () {}); }, 120000);
  // Relative times ("5 minutes ago") drift; redraw the lists now and then.
  setInterval(function () { if (S) { renderBoard(); renderTransfers(); renderInbox(); } }, 30000);

  // Files dropped from the file manager onto a device card or onto the shared board (desktop app only).
  if (T) {
    T.event.listen('tauri://drag-drop', function (e) {
      var p = e.payload, pos = p.position || { x: 0, y: 0 }, r = window.devicePixelRatio || 1;
      var el = document.elementFromPoint(pos.x / r, pos.y / r);
      var card = el && el.closest('[data-peer]');
      if (card) return run(call('send', { peer: card.dataset.peer, paths: p.paths }));
      if (el && el.closest('#board-box')) return run(call('board_put', { paths: p.paths }));
      toast(t('drop_hint'), 'err');
    });
  }

  listen(onEvent);
  reload().catch(function (e) {
    document.querySelector('main').innerHTML = '<p class="empty" style="margin:24px">' + esc(t('no_core', e.message)) + '</p>';
  });
})();
