// mc-calendar.js — MiracleClaw calendar overlay (month grid + event editor)
// rc57.25. Loaded by mc-toolbar.js when the 📅 button is clicked.
//
// Design decisions locked 2026-10-21 (David delegated: "do whatever you
// think is best for the program going forward"):
//
//   * TIMEZONE = machine-local, read from
//     Intl.DateTimeFormat().resolvedOptions().timeZone (America/Denver on
//     David's box). No stored preference for now. All timezone math lives
//     in TZ helpers at the bottom so promoting to set_preferences later is
//     a single-function change with no rework.
//
//   * CLICK-TO-CREATE = 1-hour slot starting at the clicked hour, plus a
//     "find me a free time" button that calls find_free_slot. 1 hour is the
//     common case; the smart search stays one click away.
//
// CRITICAL (Lesson 1417): /opt/maic/api on the host is NOT a bind mount
// into the gateway container. All server-side work was patched INSIDE the
// container and verified with `docker exec ... grep`. If you add server tools
// later, "restart + healthy" proves nothing.
//
// API shape notes (captured from live manifest, 2026-10-21):
//   * add_event returns FLAT outputs.event_id — NOT outputs.event.id
//   * update_event returns {updated, event, event_id, message}
//   * delete_event returns {deleted, event_id, message}; second call 404s
//   * PATCH requires end_at whenever you move start_at, else 422
//   * start_at comes back as +00:00 (UTC) even when tz_name is
//     America/Denver. tz_name is metadata only — render via TZ helpers.

(function () {
  'use strict';

  var MODULE_ID = 'calendar';
  var DAY_MS = 86400000;

  // ── timezone helpers (machine-local; promote to set_preferences later) ──

  function displayTz() {
    try {
      var t = Intl.DateTimeFormat().resolvedOptions().timeZone;
      if (t) return t;
    } catch (e) { /* fall through */ }
    return 'local';
  }

  // Format a Date into the {y,m,d,h,min} wall-clock parts of displayTz().
  function partsInTz(date, tz) {
    var fmt;
    try {
      fmt = new Intl.DateTimeFormat('en-CA', {
        timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit',
        hour: '2-digit', minute: '2-digit', hour12: false,
      });
    } catch (e) { return null; }
    var out = {};
    fmt.formatToParts(date).forEach(function (p) {
      if (p.type !== 'literal') out[p.type] = p.value;
    });
    // Intl renders midnight as 24 in some locales — normalize.
    if (out.hour === '24') out.hour = '00';
    return {
      y: +out.year, m: +out.month, d: +out.day,
      h: +out.hour, min: +out.minute,
    };
  }

  // Build a Date from wall-clock parts in the display timezone.
  // Two-pass offset resolution — the offset depends on the instant, which
  // depends on the offset. Standard DST-safe approach.
  function dateFromParts(p, tz) {
    var guess = Date.UTC(p.y, p.m - 1, p.d, p.h, p.min);
    var off1 = tzOffsetMs(new Date(guess), tz);
    var d1 = new Date(guess - off1);
    var off2 = tzOffsetMs(d1, tz);
    return new Date(guess - off2);
  }

  function tzOffsetMs(date, tz) {
    var p = partsInTz(date, tz);
    if (!p) return -date.getTimezoneOffset() * 60000;
    var asUtc = Date.UTC(p.y, p.m - 1, p.d, p.h, p.min);
    return asUtc - date.getTime();
  }

  // ISO 8601 with a real offset (e.g. 2026-11-03T14:00:00-06:00). MAIC's
  // add_event takes ISO strings; sending the offset preserves the intent.
  function toIsoTz(date, tz) {
    var p = partsInTz(date, tz);
    if (!p) return date.toISOString();
    var off = tzOffsetMs(date, tz) / 60000;
    var sign = off < 0 ? '-' : '+';
    var abs = Math.abs(off);
    var hh = String(Math.floor(abs / 60)).padStart(2, '0');
    var mm = String(Math.round(abs % 60)).padStart(2, '0');
    function pad(n) { return String(n).padStart(2, '0'); }
    return p.y + '-' + pad(p.m) + '-' + pad(p.d) + 'T' +
           pad(p.h) + ':' + pad(p.min) + ':00' + sign + hh + ':' + mm;
  }

  // ── tiny DOM helpers ──

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function escapeHtml(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  // ── state ──

  var state = {
    root: null,
    viewYear: null,   // anchor for the month grid
    viewMonth: null,  // 0-11
    events: [],       // EventOut[]
    selectedId: null,
    tz: displayTz(),
    invoke: null,
  };

  // ── data ──

  function runTool(tool, inputs) {
    if (typeof state.invoke !== 'function') {
      return Promise.reject(new Error('Tauri IPC unavailable'));
    }
    return state.invoke('mc_run_module', {
      moduleId: MODULE_ID,
      tool: tool,
      inputs: inputs || {},
      surface: 'openclaw_chat',
    });
  }

  function outputsOf(resp) {
    return (resp && resp.outputs && typeof resp.outputs === 'object') ? resp.outputs : {};
  }

  // Read the event list defensively — the array may arrive under a few
  // different keys depending on the runner revision in production.
  function extractEvents(resp) {
    var o = outputsOf(resp);
    var list = o.events || o.items || o.results;
    if (Array.isArray(list)) return list;
    if (Array.isArray(o)) return o;
    if (Array.isArray(resp)) return resp;
    return [];
  }

  function loadMonth() {
    // Fetch a window wider than the visible grid: month grids include
    // leading/trailing days from adjacent months, so pad by a week each side.
    var tz = state.tz;
    var anchor = dateFromParts({ y: state.viewYear, m: state.viewMonth + 1, d: 1, h: 0, min: 0 }, tz);
    var from = new Date(anchor.getTime() - 7 * DAY_MS);
    var to = new Date(anchor.getTime() + 42 * DAY_MS);
    // Lesson 1421: the calendar runners use window_start/window_end, NOT
    // start_at/end_at. Sending start_at here yields
    //   HTTP 422 {"detail":"window_start is required."}
    // Verified against the live manifest + runners in the running container.
    return runTool('list_events', {
      window_start: toIsoTz(from, tz),
      window_end: toIsoTz(to, tz),
    }).then(function (resp) {
      state.events = extractEvents(resp);
      render();
    });
  }

  // ── rendering ──

  function monthMatrix(year, month) {
    // Monday-first grid, 6 rows of 7. Pad with adjacent-month days.
    var tz = state.tz;
    var first = dateFromParts({ y: year, m: month + 1, d: 1, h: 0, min: 0 }, tz);
    var firstParts = partsInTz(first, tz);
    var jsDow = first.getUTCDay(); // placeholder; use parts instead
    var p = partsInTz(first, tz);
    // day-of-week for the 1st, computed in display tz via a UTC-known date
    var dow = new Date(Date.UTC(p.y, p.m - 1, p.d)).getUTCDay();
    var mondayFirst = (dow + 6) % 7;
    var cells = [];
    var start = new Date(first.getTime() - mondayFirst * DAY_MS);
    for (var i = 0; i < 42; i++) {
      var cur = new Date(start.getTime() + i * DAY_MS);
      var cp = partsInTz(cur, tz);
      cells.push({
        date: cur,
        y: cp.y, m: cp.m, d: cp.d,
        inMonth: cp.m === month + 1,
      });
    }
    return cells;
  }

  function eventsOnDay(y, m, d) {
    var tz = state.tz;
    var out = [];
    state.events.forEach(function (ev) {
      if (ev.all_day) {
        var s = new Date(ev.start_at), e = new Date(ev.end_at);
        // all-day: compare using UTC calendar days (stored as midnight UTC)
        var sd = s.toISOString().slice(0, 10);
        var ed = e.toISOString().slice(0, 10);
        var key = y + '-' + String(m).padStart(2, '0') + '-' + String(d).padStart(2, '0');
        if (key >= sd && key <= ed) out.push(ev);
        return;
      }
      var p = partsInTz(new Date(ev.start_at), tz);
      if (p && p.y === y && p.m === m && p.d === d) out.push(ev);
    });
    out.sort(function (a, b) {
      if (a.all_day !== b.all_day) return a.all_day ? -1 : 1;
      return String(a.start_at).localeCompare(String(b.start_at));
    });
    return out;
  }

  function fmtTime(ev, tz) {
    if (ev.all_day) return 'all day';
    var p = partsInTz(new Date(ev.start_at), tz);
    if (!p) return '';
    var h = p.h, mm = p.min;
    var ampm = h >= 12 ? 'PM' : 'AM';
    var h12 = h % 12; if (h12 === 0) h12 = 12;
    return h12 + ':' + String(mm).padStart(2, '0') + ' ' + ampm;
  }

  function render() {
    if (!state.root) return;
    var body = state.root.querySelector('.mc-cal-body');
    if (!body) return;
    body.innerHTML = '';

    var monthName = new Date(Date.UTC(state.viewYear, state.viewMonth, 1))
      .toLocaleString('en-US', { month: 'long', timeZone: 'UTC' });
    state.root.querySelector('.mc-cal-title').textContent =
      monthName + ' ' + state.viewYear;

    var grid = el('div', 'mc-cal-grid');
    ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'].forEach(function (d) {
      grid.appendChild(el('div', 'mc-cal-dow', d));
    });

    monthMatrix(state.viewYear, state.viewMonth).forEach(function (cell) {
      var cellEl = el('div', 'mc-cal-cell' + (cell.inMonth ? '' : ' is-outside'));
      var head = el('div', 'mc-cal-daynum', String(cell.d));
      cellEl.appendChild(head);

      var evs = eventsOnDay(cell.y, cell.m, cell.d);
      var listEl = el('div', 'mc-cal-events');
      evs.slice(0, 3).forEach(function (ev) {
        var chip = el('button', 'mc-cal-chip');
        if (ev.all_day) chip.classList.add('is-allday');
        if (ev.category_color) {
          chip.style.setProperty('--chip', ev.category_color);
        }
        chip.title = (ev.title || '(untitled)') +
          (ev.location ? ' @ ' + ev.location : '');
        chip.textContent = (ev.all_day ? '' : fmtTime(ev, state.tz) + ' ') +
          (ev.title || '(untitled)');
        chip.addEventListener('click', function (e) {
          e.stopPropagation();
          state.selectedId = ev.id;
          openEditor(ev);
        });
        listEl.appendChild(chip);
      });
      if (evs.length > 3) {
        listEl.appendChild(el('div', 'mc-cal-more', '+' + (evs.length - 3) + ' more'));
      }
      cellEl.appendChild(listEl);

      // click-to-create: 1-hour slot at the clicked day (9am default)
      cellEl.addEventListener('click', function () {
        openCreate(cell);
      });
      grid.appendChild(cellEl);
    });

    body.appendChild(grid);
  }

  // ── editor (create + edit share one form) ──

  function field(labelText, inputEl) {
    var wrap = el('label', 'mc-cal-field');
    wrap.appendChild(el('span', 'mc-cal-label', labelText));
    wrap.appendChild(inputEl);
    return wrap;
  }

  function openCreate(cell) {
    var tz = state.tz;
    // Default 9:00 AM on the clicked day — a sane business-hours anchor.
    var start = dateFromParts(
      { y: cell.y, m: cell.m, d: cell.d, h: 9, min: 0 }, tz);
    openForm({
      mode: 'create',
      start: start,
      end: new Date(start.getTime() + 3600000),
    });
  }

  function openEditor(ev) {
    var start = new Date(ev.start_at);
    var end = new Date(ev.end_at);
    if (isNaN(start.getTime())) start = new Date();
    if (isNaN(end.getTime())) end = new Date(start.getTime() + 3600000);
    openForm({ mode: 'edit', event: ev, start: start, end: end });
  }

  function openForm(ctx) {
    closeEditor();
    var tz = state.tz;
    var ed = el('div', 'mc-cal-editor');
    var panel = el('div', 'mc-cal-editor-panel');

    panel.appendChild(el('div', 'mc-cal-editor-title',
      ctx.mode === 'edit' ? 'Edit event' : 'New event'));

    var titleInput = document.createElement('input');
    titleInput.type = 'text';
    titleInput.className = 'mc-cal-input';
    titleInput.placeholder = 'Title';
    if (ctx.mode === 'edit') titleInput.value = ctx.event.title || '';

    // Real date+time inputs — no ISO typing. `<input type=date>` returns
    // YYYY-MM-DD and `<input type=time>` returns HH:MM in the user's own
    // local wall clock, which is exactly displayTz() on a single-user box.
    var startDate = document.createElement('input');
    startDate.type = 'date'; startDate.className = 'mc-cal-input';
    var startTime = document.createElement('input');
    startTime.type = 'time'; startTime.className = 'mc-cal-input';
    var endDate = document.createElement('input');
    endDate.type = 'date'; endDate.className = 'mc-cal-input';
    var endTime = document.createElement('input');
    endTime.type = 'time'; endTime.className = 'mc-cal-input';

    var sp = partsInTz(ctx.start, tz);
    var ep = partsInTz(ctx.end, tz);
    function p2(v) { return String(v).padStart(2, '0'); }
    startDate.value = sp.y + '-' + p2(sp.m) + '-' + p2(sp.d);
    startTime.value = p2(sp.h) + ':' + p2(sp.min);
    endDate.value = ep.y + '-' + p2(ep.m) + '-' + p2(ep.d);
    endTime.value = p2(ep.h) + ':' + p2(ep.min);

    var startRow = el('div', 'mc-cal-row');
    startRow.appendChild(field('Starts', startDate));
    startRow.appendChild(field('', startTime));
    var endRow = el('div', 'mc-cal-row');
    endRow.appendChild(field('Ends', endDate));
    endRow.appendChild(field('', endTime));

    var allDay = document.createElement('input');
    allDay.type = 'checkbox';
    allDay.className = 'mc-cal-check';
    if (ctx.mode === 'edit' && ctx.event.all_day) allDay.checked = true;

    var showAs = document.createElement('select');
    showAs.className = 'mc-cal-input';
    ['busy', 'tentative', 'free'].forEach(function (v) {
      var o = document.createElement('option');
      o.value = v; o.textContent = v;
      if (ctx.mode === 'edit' && ctx.event.show_as === v) o.selected = true;
      showAs.appendChild(o);
    });

    var locInput = document.createElement('input');
    locInput.type = 'text'; locInput.className = 'mc-cal-input';
    locInput.placeholder = 'Location';
    if (ctx.mode === 'edit' && ctx.event.location) locInput.value = ctx.event.location;

    var descInput = document.createElement('textarea');
    descInput.className = 'mc-cal-input mc-cal-textarea';
    descInput.placeholder = 'Notes';
    if (ctx.mode === 'edit' && ctx.event.description) {
      descInput.value = ctx.event.description;
    }

    var rruleInput = document.createElement('input');
    rruleInput.type = 'text'; rruleInput.className = 'mc-cal-input';
    rruleInput.placeholder = 'Repeat (RRULE) — blank for once';
    if (ctx.mode === 'edit' && ctx.event.rrule) rruleInput.value = ctx.event.rrule;

    panel.appendChild(field('Title', titleInput));
    panel.appendChild(startRow);
    panel.appendChild(endRow);

    var optsRow = el('div', 'mc-cal-row');
    var adWrap = el('label', 'mc-cal-field mc-cal-field-inline');
    adWrap.appendChild(allDay);
    adWrap.appendChild(el('span', 'mc-cal-label', 'All day'));
    optsRow.appendChild(adWrap);
    optsRow.appendChild(field('Show as', showAs));
    panel.appendChild(optsRow);

    panel.appendChild(field('Location', locInput));
    panel.appendChild(field('Notes', descInput));
    // Lesson 1421: update_event does NOT accept rrule (its runner only
    // forwards title/start_at/end_at/description/location/status/show_as/
    // all_day/reminder_minutes/category_id). Sending it is a no-op at best
    // and pollutes the payload, so only offer Repeat when creating.
    if (ctx.mode === 'create') {
      panel.appendChild(field('Repeat', rruleInput));
    }

    var status = el('div', 'mc-cal-status');

    var actions = el('div', 'mc-cal-actions');
    var freeBtn = el('button', 'mc-cal-btn mc-cal-btn-ghost', 'Find me a free time');
    freeBtn.type = 'button';
    freeBtn.title = 'Ask the calendar for an open slot near this time';
    var cancelBtn = el('button', 'mc-cal-btn mc-cal-btn-ghost', 'Cancel');
    cancelBtn.type = 'button';
    cancelBtn.addEventListener('click', closeEditor);

    var saveBtn = el('button', 'mc-cal-btn mc-cal-btn-primary',
      ctx.mode === 'edit' ? 'Save changes' : 'Create event');
    saveBtn.type = 'button';

    if (ctx.mode === 'edit') {
      var delBtn = el('button', 'mc-cal-btn mc-cal-btn-danger', 'Delete');
      delBtn.type = 'button';
      delBtn.addEventListener('click', function () {
        if (!window.confirm('Delete “' + (ctx.event.title || 'this event') + '”?')) return;
        status.textContent = 'Deleting…';
        runTool('delete_event', { event_id: ctx.event.id })
          .then(function () { closeEditor(); return loadMonth(); })
          .catch(function (e) {
            status.textContent = 'Delete failed: ' + errText(e);
            status.className = 'mc-cal-status is-error';
          });
      });
      actions.appendChild(delBtn);
    }

    actions.appendChild(freeBtn);
    actions.appendChild(cancelBtn);
    actions.appendChild(saveBtn);
    panel.appendChild(actions);
    panel.appendChild(status);

    freeBtn.addEventListener('click', function () {
      status.className = 'mc-cal-status';
      status.textContent = 'Looking for an open time…';
      freeBtn.disabled = true;
      // Lesson 1421: find_free_slot takes window_start/window_end and
      // returns {slots:[{start_at,end_at},...]}, not a bare start_at.
      // Search the 3 days after the requested start, business hours.
      var baseIso = startDate.value + 'T' + (startTime.value || '09:00') + ':00';
      var base = new Date(baseIso);
      return runTool('find_free_slot', {
        window_start: toIsoTz(base, tz),
        window_end: toIsoTz(new Date(base.getTime() + 3 * DAY_MS), tz),
        duration_minutes: 60,
        work_start_hour: 9,
        work_end_hour: 17,
      }).then(function (resp) {
        var o = outputsOf(resp);
        var slots = o.slots || [];
        if (!slots.length) throw new Error('no open slot in the next 3 days');
        var pick = slots[0];
        var d = new Date(pick.start_at);
        var dp = partsInTz(d, tz);
        startDate.value = dp.y + '-' + p2(dp.m) + '-' + p2(dp.d);
        startTime.value = p2(dp.h) + ':' + p2(dp.min);
        var e2 = new Date(pick.end_at || d.getTime() + 3600000);
        var ep2 = partsInTz(e2, tz);
        endDate.value = ep2.y + '-' + p2(ep2.m) + '-' + p2(ep2.d);
        endTime.value = p2(ep2.h) + ':' + p2(ep2.min);
        status.textContent = slots.length > 1
          ? ('Picked ' + fmtTime({start_at: pick.start_at, all_day:false}, tz) +
             ' — ' + (slots.length - 1) + ' other open time(s) in range.')
          : 'Filled in the next open hour. Adjust if you like.';
      }).catch(function (e) {
        status.className = 'mc-cal-status is-error';
        status.textContent = 'No free slot found: ' + errText(e);
      }).then(function () { freeBtn.disabled = false; });
    });

    function readInputs() {
      // Lesson 1421: update_event ignores rrule (runner doesn't forward it).
      // Only attach it on create so the edit payload stays honest.
      var allowRrule = ctx.mode === 'create';
      var sd = startDate.value, st = startTime.value || '00:00';
      var ed2 = endDate.value, et = endTime.value || '00:00';
      if (!sd || !ed2) throw new Error('start and end dates are required');
      var s = dateFromParts(parseParts(sd, st), tz);
      var e3 = dateFromParts(parseParts(ed2, et), tz);
      if (e3.getTime() <= s.getTime()) throw new Error('end must be after start');
      var out = {
        title: titleInput.value.trim() || '(untitled)',
        start_at: toIsoTz(s, tz),
        end_at: toIsoTz(e3, tz),
        all_day: allDay.checked,
        show_as: showAs.value,
        location: locInput.value.trim(),
        description: descInput.value.trim(),
      };
      if (allowRrule && rruleInput.value.trim()) {
        out.rrule = rruleInput.value.trim();
      }
      return out;
    }

    saveBtn.addEventListener('click', function () {
      var payload;
      try { payload = readInputs(); }
      catch (e) {
        status.className = 'mc-cal-status is-error';
        status.textContent = e.message;
        return;
      }
      status.className = 'mc-cal-status';
      status.textContent = 'Saving…';
      saveBtn.disabled = true;
      var p;
      if (ctx.mode === 'edit') {
        payload.event_id = ctx.event.id;
        p = runTool('update_event', payload);
      } else {
        payload.tz_name = state.tz;
        p = runTool('add_event', payload);
      }
      p.then(function () {
        closeEditor();
        return loadMonth();
      }).catch(function (e) {
        status.className = 'mc-cal-status is-error';
        status.textContent = 'Save failed: ' + errText(e);
        saveBtn.disabled = false;
      });
    });

    ed.appendChild(panel);
    ed.addEventListener('click', function (e) {
      if (e.target === ed) closeEditor();
    });
    state.root.appendChild(ed);
    state.root._editor = ed;
    setTimeout(function () { titleInput.focus(); }, 0);
  }

  function parseParts(dateStr, timeStr) {
    var d = dateStr.split('-').map(Number);
    var t = (timeStr || '00:00').split(':').map(Number);
    return { y: d[0], m: d[1], d: d[2], h: t[0] || 0, min: t[1] || 0 };
  }

  function errText(e) {
    if (!e) return 'unknown error';
    return e.message || String(e);
  }

  function closeEditor() {
    if (state.root && state.root._editor) {
      state.root.removeChild(state.root._editor);
      state.root._editor = null;
    }
  }

  // ── shell ──

  function open(opts) {
    if (state.invoke) { close(); }
    state.invoke = (opts && opts.invoke) || null;

    var root = el('div', 'mc-cal-root');
    root.setAttribute('role', 'dialog');
    root.setAttribute('aria-label', 'Calendar');

    var bar = el('div', 'mc-cal-bar');
    var prev = el('button', 'mc-cal-nav', '‹');
    prev.type = 'button';
    prev.title = 'Previous month';
    var title = el('div', 'mc-cal-title', '');
    var next = el('button', 'mc-cal-nav', '›');
    next.type = 'button';
    next.title = 'Next month';
    var today = el('button', 'mc-cal-btn mc-cal-btn-ghost', 'Today');
    today.type = 'button';
    var closeBtn = el('button', 'mc-cal-nav mc-cal-close', '✕');
    closeBtn.type = 'button';
    closeBtn.title = 'Close calendar';

    bar.appendChild(prev);
    bar.appendChild(title);
    bar.appendChild(next);
    bar.appendChild(today);
    bar.appendChild(closeBtn);
    root.appendChild(bar);

    var tzNote = el('div', 'mc-cal-tz', 'Times shown in ' + state.tz);
    root.appendChild(tzNote);

    var body = el('div', 'mc-cal-body');
    root.appendChild(body);

    function shift(delta) {
      var m = state.viewMonth + delta;
      var y = state.viewYear;
      if (m < 0) { m = 11; y--; }
      if (m > 11) { m = 0; y++; }
      state.viewMonth = m;
      state.viewYear = y;
      render();
    }
    prev.addEventListener('click', function () { shift(-1); });
    next.addEventListener('click', function () { shift(1); });
    closeBtn.addEventListener('click', function () { close(); });
    today.addEventListener('click', function () {
      var p = partsInTz(new Date(), state.tz);
      state.viewYear = p.y; state.viewMonth = p.m - 1;
      render();
    });
    root.addEventListener('click', function (e) {
      if (e.target === root) close();
    });

    state.root = root;
    var now = partsInTz(new Date(), state.tz);
    state.viewYear = now.y;
    state.viewMonth = now.m - 1;

    document.body.appendChild(root);
    render();

    loadMonth().catch(function (e) {
      var body2 = state.root && state.root.querySelector('.mc-cal-body');
      if (body2) body2.innerHTML = '';
      if (body2) {
        body2.appendChild(el('div', 'mc-cal-error',
          'Could not load events: ' + errText(e)));
      }
    });
  }

  function close() {
    closeEditor();
    if (state.root && state.root.parentNode) {
      state.root.parentNode.removeChild(state.root);
    }
    state.root = null;
  }

  window.McCalendar = { open: open, close: close, toIsoTz: toIsoTz };
})();