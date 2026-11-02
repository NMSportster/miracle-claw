// MiracleClaw chat-toolbar (Lesson 978, rc56.8)
//
// Adds a top-right floating toolbar with four icon-link buttons to the
// OpenClaw chat UI when hosted inside MiracleClaw's Tauri webview
// (http://127.0.0.1:28789/):
//
//   📎 Attach    -- drop files, copy paths to clipboard
//   🔑 Secrets   -- list local tool names + secret names (no values)
//   🎙 Voice     -- invoke mc_voice_native_capture (whisper.cpp sidecar, Lesson 1001)
//   🧩 Modules   -- dropdown of installed modules, "Run" calls
//                  mc_run_module and sends the result into the chat input
//
// This file is *deliberately* self-contained -- the openclaw dist has its
// own JS world and can't import from MC's source tree (`src/`). We
// duplicate the minimum we need: a Tauri-invoke helper, an event bus,
// a toast helper, and the modules menu. State that we *do* share with
// MC's `modules-runtime.js` (the `installed` Map) is not shared
// directly -- both sides read from the same Tauri command
// `mc_list_installed_modules` so they agree.
//
// Detection (Lesson 511 -- IPC-independent):
//   We gate on `window.location.host === '127.0.0.1:28789'`. No Tauri
//   globals required for the gate; the gate just tells us "we're on the
//   OpenClaw chat page hosted by MC". Actual Tauri calls inside the
//   button handlers will fail loudly if MC's webview isn't wired --
//   which is the right behavior, since the user wouldn't be seeing
//   this UI in that case anyway.
//
// Click strategy:
//   We attach CAPTURE-phase click listeners on our own buttons. We do
//   NOT touch the openclaw app's own button handlers -- we live in our
//   own floating panel and never touch the React tree.
//
// Idempotency:
//   We check for `#mc-oc-toolbar` before mounting. The patcher is also
//   idempotent (it's a WRITE patch, content-compared).
//
//   All UI elements carry the `.mc-oc-` prefix so we can't accidentally
//   select something inside the openclaw host page.

(function () {
  'use strict';

  // ---------------------------------------------------------------------------
  // Gate
  // ---------------------------------------------------------------------------

  function shouldMount() {
    if (!window.location || window.location.host !== '127.0.0.1:28789') {
      return false;
    }
    if (document.getElementById('mc-oc-toolbar')) {
      return false; // already mounted
    }
    return true;
  }

  // ---------------------------------------------------------------------------
  // Tauri invoke -- supports the three surfaces Tauri 2.x exposes
  // (mirrors mc-voice-native.js + mc-back-button.js).
  // ---------------------------------------------------------------------------

  function tauriInvoke(cmd, args) {
    args = args || {};
    var w = window;
    var internals = w.__TAURI_INTERNALS__;
    if (internals && typeof internals.invoke === 'function') {
      return internals.invoke(cmd, args);
    }
    var t2 = w.__TAURI__ && w.__TAURI__.core;
    if (t2 && typeof t2.invoke === 'function') {
      return t2.invoke(cmd, args);
    }
    var t1 = w.__TAURI__;
    if (t1 && typeof t1.invoke === 'function') {
      return t1.invoke(cmd, args);
    }
    return Promise.reject(new Error(
      'Tauri IPC not available -- mc-toolbar must run inside MC Tauri webview'
    ));
  }

  // ---------------------------------------------------------------------------
  // escapeHtml — local helper so the toolbar doesn't depend on any
  // global that may or may not exist in the host page (Lesson 1132).
  // The chat toolbar used to reference a bare `escapeHtml(v)` in the
  // enum option loop without defining it, which silently worked only
  // because the enum values were safe ASCII strings. Help text and
  // placeholder text come from the module's form_schema and can
  // contain anything, so we escape explicitly here.
  // ---------------------------------------------------------------------------

  function escapeHtml(s) {
    if (s === null || s === undefined) return '';
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  // ---------------------------------------------------------------------------
  // Toast
  // ---------------------------------------------------------------------------

  var toastTimer = null;
  function showToast(msg, kind) {
    kind = kind || 'info';
    var t = document.getElementById('mc-oc-toast');
    if (!t) {
      t = document.createElement('div');
      t.id = 'mc-oc-toast';
      t.className = 'mc-oc-toast';
      t.setAttribute('role', 'status');
      t.setAttribute('aria-live', 'polite');
      t.setAttribute('hidden', '');
      document.body.appendChild(t);
    }
    t.className = 'mc-oc-toast' + (kind === 'error' ? ' is-error' : kind === 'success' ? ' is-success' : '');
    t.textContent = String(msg);
    t.removeAttribute('hidden');
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      t.setAttribute('hidden', '');
    }, 3500);
  }

  // ---------------------------------------------------------------------------
  // Dropdown open/close (Lesson 968 -- sibling of trigger, viewport-aware
  // positioning from Lesson 976)
  // ---------------------------------------------------------------------------

  var openDropdown = null;

  function closeDropdown() {
    if (openDropdown) {
      openDropdown.setAttribute('hidden', '');
      var triggerId = openDropdown.getAttribute('data-trigger');
      if (triggerId) {
        var trig = document.getElementById(triggerId);
        if (trig) trig.setAttribute('aria-expanded', 'false');
      }
      openDropdown = null;
    }
  }

  function openDropdownFor(trigger, dropdown) {
    if (openDropdown && openDropdown !== dropdown) closeDropdown();
    dropdown.removeAttribute('hidden');
    trigger.setAttribute('aria-expanded', 'true');

    // Position AFTER attach so we can measure width.
    var rect = trigger.getBoundingClientRect();
    var menuRect = dropdown.getBoundingClientRect();
    var menuWidth = menuRect.width || 280;
    var menuHeight = menuRect.height || 240;
    var margin = 8;
    var top, left;
    // Default: below the trigger, right-aligned to its right edge
    left = rect.right - menuWidth;
    if (left < margin) left = margin;
    if (left + menuWidth > window.innerWidth - margin) {
      left = window.innerWidth - menuWidth - margin;
    }
    top = rect.bottom + 6;
    // Flip above if not enough room below (200px minimum visible).
    if (top + menuHeight > window.innerHeight - margin && top - menuHeight - rect.height - 6 > margin) {
      top = top - menuHeight - rect.height - 6;
    }
    dropdown.style.left = Math.max(margin, left) + 'px';
    dropdown.style.top = Math.max(margin, top) + 'px';
    openDropdown = dropdown;
  }

  function makeButton(id, glyph, title, ariaLabel, extraClass) {
    var btn = document.createElement('button');
    btn.type = 'button';
    btn.id = id;
    btn.className = 'mc-oc-icon-link' + (extraClass ? ' ' + extraClass : '');
    btn.setAttribute('title', title);
    btn.setAttribute('aria-label', ariaLabel || title);
    btn.setAttribute('aria-haspopup', 'true');
    btn.setAttribute('aria-expanded', 'false');
    var span = document.createElement('span');
    span.className = 'mc-oc-icon-link-glyph';
    span.textContent = glyph;
    btn.appendChild(span);
    return btn;
  }

  // ---------------------------------------------------------------------------
  // Modules: list (Lesson 975 -- query mc_list_installed_modules) + run
  // ---------------------------------------------------------------------------

  // Module ID aliases (Lesson 1136, 2026-10-08, David). MAIC tracks
  // modules under hyphenated product names (e.g. `email-campaigns-pro`,
  // `voice-memo`) while the bundled installer registry on disk uses
  // shorter generic names (`email`, `voice`). They're the same
  // underlying product, but a naive merge by exact id counted both
  // and David saw 18 "modules installed" in the Terminal toolbar
  // badge on 2026-10-08 when the truth is 8.
  //
  // Direction: MAIC always wins on overlap because it has live
  // install state + the rich client_manifest (tools, form_schema,
  // description, category). The local registry is a fallback / source
  // for modules that exist on disk but haven't been claimed by MAIC
  // (e.g. standalone sidecar installs that pre-date MAIC enrollment).
  //
  // When adding a new MAIC module, also add its local-bundle id here
  // (or its MAIC id) so the dropdown and badge count it once.
  var MODULE_ID_ALIASES = {
    // MAIC id             -> canonical (local-bundle id)
    'email-campaigns-pro': 'email',
    'calendar-companion':  'calendar',
    'voice-memo':          'voice',
    'brand-asset-kit':     'brand',
    'ad-performance':      'ads',
    'data-analyzer':       'data',
    'marketing-suite':     'marketing',
  };
  function canonicalId(id) {
    if (!id) return id;
    if (Object.prototype.hasOwnProperty.call(MODULE_ID_ALIASES, id)) {
      return MODULE_ID_ALIASES[id];
    }
    // Reverse: if `id` is the local side of an alias, return the
    // MAIC id (so remote-wins deduplication prefers the MAIC record).
    for (var maic in MODULE_ID_ALIASES) {
      if (MODULE_ID_ALIASES[maic] === id) return maic;
    }
    return id;
  }

  // >>> BEGIN GENERATED: MODULE-HELP >>>
  // GENERATED by scripts/sync-toolbar-module-help.mjs — DO NOT EDIT BY HAND.
  // Source of truth: src/data/module-help.js + src/data/maic-module-help.js
  // Run `node scripts/sync-toolbar-module-help.mjs` after editing those.
  var TOOLBAR_MODULE_HELP = {
    "ad-performance": {
      "whoFor": "Anyone running paid ads who wants a quick read on what is working and what is burning money.",
      "whyUseIt": "Give it a date range and it summarizes clicks, conversions, and ROI so you know where to move your next dollar.",
      "whatItDoes": "Analyzes ad campaign metrics and flags underperformers. Google Ads sync is not wired up yet, so the run returns a filled-in analysis template you can paste your numbers into.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button in the top-right of the chat window.",
            "Click Ad Performance, then press Run.",
            "Fill in Date range (for example: last 30 days). Add a Campaign ID if you have one.",
            "Press Run again to see the result, then Send to Chat to drop it into your conversation."
          ]
        }
      ],
      "examples": [
        "Date range: last 30 days — summarize clicks, conversions, and ROI.",
        "Date range: last 7 days, Campaign ID: brake-special-q3 — which ads are underperforming?"
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "The result looks like an empty template with no real numbers in it.",
          "fix": "That is expected right now. Google Ads sync is still coming, so the module hands you a correctly-structured analysis table for you to paste your metrics into. The layout, the fields, and the underperformer logic are the real thing."
        },
        {
          "problem": "I get an error saying Date range is required.",
          "fix": "Date range is the one required field. Enter something like 'last 30 days' or '2026-10-01 to 2026-10-31'. Campaign ID is optional."
        }
      ]
    },
    "brand-asset-kit": {
      "whoFor": "Small business owners and marketers who need a bio that actually sounds like their brand.",
      "whyUseIt": "Pick a platform, add a few keywords that match your voice, and get a bio written to fit that platform's style — punchy for Twitter, professional for LinkedIn.",
      "whatItDoes": "Generates on-brand social bios tuned to the platform you choose.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Brand Asset Kit.",
            "Press Run.",
            "Choose a Platform from the drop-down (twitter, linkedin, instagram, or website).",
            "Optionally add Keywords — comma separated, for example: tire repair, San Francisco Rd, fast.",
            "Press Run, then Send to Chat."
          ]
        }
      ],
      "examples": [
        "Platform: twitter, Keywords: auto repair, Albuquerque, honest estimates.",
        "Platform: linkedin, Keywords: family shop, second generation, since 1994."
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "The bio is too generic.",
          "fix": "Add specific keywords — your actual services, your town, what makes you different. The more concrete the keywords, the more the bio sounds like you instead of a template."
        },
        {
          "problem": "I get an error saying Platform is required.",
          "fix": "Platform is the one required field. Pick one from the drop-down. Keywords are optional but make a big difference."
        }
      ]
    },
    "calendar": {
      "whoFor": "Anyone with a packed schedule — booking jobs, running back-to-back appointments, or coordinating across devices.",
      "whyUseIt": "Your calendar lives in your Miracle Claw account, so it shows up on every device you sign in on. Recurring events, categories, reminders, free-slot finding, and .ics import are all built in.",
      "whatItDoes": "Add, edit, move, and cancel events; mark them Busy, Tentative, or Free; search by any word in a title, note, or location; find open slots; import .ics files; and extract appointments out of pasted messages.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Miracle Claw Calendar.",
            "With 15 tools you'll get a tool picker — pick the one you want.",
            "Search Events asks for a word to look for and how many results to show.",
            "Add Event asks for a title, a start, and an end. Times are ISO 8601, for example 2026-11-03T14:00:00.",
            "Extract Appointment takes a pasted message and reads the date, time, location, and title back to you.",
            "Free Slot finds openings that don't clash with anything you've saved."
          ]
        }
      ],
      "examples": [
        "Search Events: q = 'dentist', limit = 10",
        "Add Event: title = 'Brake job — Smith', start_at = 2026-11-03T14:00:00, end_at = 2026-11-03T15:00:00, location = 'San Francisco Rd NE'",
        "Extract Appointment: text = 'Dentist Tuesday Dec 5 at 2pm, Dr Pham on Paseo del Norte'",
        "Free Slot: window_start = 2026-11-03T09:00:00, window_end = 2026-11-03T17:00:00, duration_minutes = 30",
        "Import ICS: paste the whole .ics text from BEGIN:VCALENDAR to END:VCALENDAR"
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "Free Slot and Busy Range don't see my Google Calendar or Outlook events.",
          "fix": "Honest limit: those two tools only know about events saved inside Miracle Claw. They don't read external calendars, so check Google or Outlook for conflicts first. Import is a one-time copy, not a live two-way sync."
        },
        {
          "problem": "Extract Appointment read the time wrong.",
          "fix": "It parses the message you paste and uses your timezone to work out the time. If the message says 'tomorrow' or 'next Friday', set Reference Date (YYYY-MM-DD) so it resolves against the right day. It saves nothing until you confirm — read it back and fix anything it got wrong."
        },
        {
          "problem": "An event isn't blocking my free-slot search.",
          "fix": "Check the Show As setting. Anything marked Free is skipped by the search, so a long lunch marked 'free' still leaves your afternoon open."
        },
        {
          "problem": "I deleted a category and now my events lost their color.",
          "fix": "That's by design. Deleting a category removes the color label but leaves every event exactly where it was."
        }
      ]
    },
    "calendar-companion": {
      "whoFor": "People who need a time that works for everyone and would rather not read six calendars themselves.",
      "whyUseIt": "Tell it how long the meeting is and who's attending, and it suggests a time that works for everyone.",
      "whatItDoes": "Smart scheduling with conflict detection. This one is in private beta — Google Calendar sync is on the way.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Calendar Companion.",
            "Press Run.",
            "Enter Duration in minutes (between 15 and 480).",
            "Optionally add Attendees as comma-separated email addresses.",
            "Press Run."
          ]
        }
      ],
      "examples": [
        "Duration: 30 minutes, Attendees: shop@example.com, david@example.com",
        "Duration: 60 minutes — find a slot for a brake-system review."
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "I get 'This module is in private beta. Join the waitlist at https://miracleclaw.com/waitlist/calendar-companion'.",
          "fix": "That's the expected response for this one — Calendar Companion hasn't shipped its server-side runner yet. It's not a mistake on your end. Use the main Miracle Claw Calendar module's Free Slot tool in the meantime; it does the same job against the events saved in your account."
        }
      ]
    },
    "crm": {
      "whoFor": "Sales teams, service shops, agencies — anyone who keeps customer data in HubSpot, Pipedrive, or Notion and wants MAIC to push enriched contacts and lead status into those systems automatically.",
      "whyUseIt": "MAIC pulls contact info and lead status out of your chats, emails, voice notes, and transcripts — then pushes it straight into your CRM as structured records. No more manual data entry, no more copy-paste between tools.",
      "whatItDoes": "Push enriched contacts and lead status to HubSpot, Pipedrive, Notion, and other CRMs. MAIC structures the data, CRM Sync delivers it via API.",
      "windowsUI": [
        {
          "surface": "Settings → Modules → CRM Sync",
          "steps": [
            "Click the CRM Sync card to open its settings.",
            "Pick a target (HubSpot, Pipedrive, or Notion) and click Connect.",
            "Authorize in the browser window that opens. Your API key is stored locally.",
            "Repeat for any other CRMs you want to push to. All three can be active at once."
          ]
        },
        {
          "surface": "MAIC Chat",
          "steps": [
            "After chatting with a lead or customer, type \"/crm push this contact to HubSpot\" or \"/crm sync recent\".",
            "MAIC previews the structured record before pushing — you confirm.",
            "CRM Sync delivers it. You get a confirmation toast with the CRM record ID."
          ]
        }
      ],
      "examples": [
        "After a 20-minute sales call with a fleet manager, type \"/crm push to HubSpot\" — MAIC extracts name, company, fleet size, pain points, and next steps, and pushes them as a HubSpot contact + deal.",
        "End of week: type \"/crm sync recent\" — MAIC pulls the last 7 days of contacts mentioned in chat and pushes the ones it can structure cleanly.",
        "\"Add this lead to my Notion CRM database — name is Sarah, company is Acme Logistics, they're a fleet of 12 trucks\" — quick capture.",
        "\"Show me what got pushed to Pipedrive this week\" — audit trail."
      ],
      "terminal": [
        {
          "cmd": "/crm check",
          "desc": "Show CRM Sync status: which targets are configured, last sync timestamps, API key health."
        },
        {
          "cmd": "/crm list_targets",
          "desc": "List the connected CRMs (HubSpot / Pipedrive / Notion) and their connection status."
        },
        {
          "cmd": "/crm connect <target>",
          "desc": "Connect a new CRM target. target = hubspot | pipedrive | notion. Opens browser for OAuth."
        },
        {
          "cmd": "/crm preview <contact_json>",
          "desc": "Dry-run: show what would be pushed without actually pushing. Example: /crm preview '{\"name\":\"Sarah Chen\",\"company\":\"Acme Logistics\"}'"
        },
        {
          "cmd": "/crm push <contact_json>",
          "desc": "Push a structured contact to all connected CRMs. Example: /crm push '{\"name\":\"Sarah\",\"email\":\"s@acme.com\",\"stage\":\"qualified\"}'"
        },
        {
          "cmd": "/crm sync_recent",
          "desc": "Pull recent enriched contacts from MAIC session memory and push the ones that match contact patterns."
        },
        {
          "cmd": "/crm audit",
          "desc": "Show the last 20 push attempts with status (success/failure), target, record ID, and timestamp."
        }
      ],
      "chat": [
        {
          "cmd": "/crm push",
          "desc": "Push the current conversation's extracted contact + lead status to all connected CRMs. MAIC previews first."
        },
        {
          "cmd": "/crm sync_recent",
          "desc": "Pull the last 7 days of contacts from MAIC session memory and push."
        },
        {
          "cmd": "/crm audit",
          "desc": "Show the last 20 push attempts."
        }
      ],
      "troubleshooting": [
        {
          "problem": "OAuth window closed before I finished — connection didn't save.",
          "fix": "Re-open Settings → Modules → CRM Sync → click Connect for the target again. OAuth windows have a 5-minute timeout."
        },
        {
          "problem": "Push fails with 401 / 403.",
          "fix": "API key expired or was revoked. Open Settings → Modules → CRM Sync → Disconnect → Connect again. For HubSpot, also check that your connected app still has the scopes 'crm.objects.contacts.write' and 'crm.objects.deals.write'."
        },
        {
          "problem": "MAIC can't extract a contact from the conversation.",
          "fix": "CRM Sync needs at least a name + email or name + company. Make sure the contact info is mentioned in chat (not just in a linked file). For voice transcripts, use the Voice module first so the transcript is in chat."
        },
        {
          "problem": "Notion push creates the contact but doesn't link it to my CRM database.",
          "fix": "Notion pushes create standalone database entries. To link them to a parent CRM database, open Settings → Modules → CRM Sync → Notion → set the 'Parent Database ID' from your Notion database URL."
        },
        {
          "problem": "HubSpot rate limit hit (429 errors).",
          "fix": "HubSpot allows 100 requests per 10 seconds per app. CRM Sync batches pushes in groups of 10, so you won't hit this unless you're pushing thousands at once. Wait 10 seconds and try again."
        }
      ]
    },
    "data-analyzer": {
      "whoFor": "Anyone staring at a spreadsheet and wanting the story in it without writing the formulas.",
      "whyUseIt": "Point it at a public CSV and it summarizes the data, pulls out the most interesting rows, and flags anomalies. Files up to 5 MB.",
      "whatItDoes": "CSV and spreadsheet analysis with plain-English summaries.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Data Analyzer.",
            "Press Run.",
            "Paste a public CSV URL into CSV URL.",
            "Optionally fill in Focus — e.g. 'highest revenue months' or 'rows with missing values'.",
            "Press Run, then Send to Chat."
          ]
        }
      ],
      "examples": [
        "CSV URL: https://example.com/sales.csv, Focus: which months did we lose money?",
        "CSV URL: https://example.com/inventory.csv — flag anything that looks wrong."
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "I get an error saying CSV URL is required.",
          "fix": "CSV URL is the one required field. It must be a public http:// or https:// address — a link to a file your browser can fetch. Local paths like C:\\data.csv and private-network addresses are blocked on purpose."
        },
        {
          "problem": "The summary missed my columns.",
          "fix": "Use the Focus field to say what matters — 'revenue by month', 'duplicate records', 'rows over 10,000 miles'. The summarizer uses it to decide what's worth calling out."
        }
      ]
    },
    "email": {
      "whoFor": "Anyone who writes a lot of business email — sales outreach, customer replies, follow-ups, internal announcements.",
      "whyUseIt": "Tell MAIC who you're emailing and why — get a complete draft tailored to the recipient. Pick a tone (formal, friendly, direct) and edit before sending.",
      "whatItDoes": "Draft cold outreach, follow-ups, and reply emails. Pick a tone (formal / friendly / direct) and the module generates a complete email tailored to the recipient.",
      "windowsUI": [
        {
          "surface": "MAIC Chat",
          "steps": [
            "Click ✉️ in the chat input toolbar (or type /email).",
            "Pick a template (cold outreach / follow-up / reply), paste the recipient context, choose a tone.",
            "Generated email lands in the chat input. Edit or send."
          ]
        }
      ],
      "examples": [
        "\"Draft a cold outreach email to a fleet manager introducing our brake service\" — pick the 'formal' tone.",
        "\"Write a friendly follow-up to a customer who got a quote last Tuesday and hasn't responded.\"",
        "\"Draft a reply to this support thread — the customer's printer is offline and they tried restarting.\"",
        "\"Write an internal email announcing the new shop hours next week.\""
      ],
      "terminal": [
        {
          "cmd": "/email draft",
          "desc": "Open the draft wizard: pick template, enter recipient + context, choose tone."
        },
        {
          "cmd": "/email check",
          "desc": "Show module status: whether MAIC is reachable, supported tones."
        }
      ],
      "chat": [
        {
          "cmd": "/email draft",
          "desc": "Same as Terminal but the draft drops into the chat input."
        },
        {
          "cmd": "/email reply <thread>",
          "desc": "Paste the email thread you're replying to and the tone. Returns a draft reply."
        }
      ],
      "troubleshooting": [
        {
          "problem": "Module not installed.",
          "fix": "If the Email Writer card says \"Coming Soon\" instead of \"Install\", the module hasn't shipped in your current version of MC. Check for an update in Settings → Modules."
        },
        {
          "problem": "Draft is too generic.",
          "fix": "The more context you give (recipient name, company, what you're offering, why you're reaching out), the better the draft. Start with: \"I'm reaching out to [name] at [company] about [topic].\""
        }
      ]
    },
    "email-campaigns-pro": {
      "whoFor": "Shop owners and small-business marketers who send the same campaign every few weeks and want it drafted, not written from scratch.",
      "whyUseIt": "Describe the goal in a sentence or two and you get a 3-email drip with A/B subject lines for each step.",
      "whatItDoes": "Sequenced email drips with A/B subject lines.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Email Campaigns Pro.",
            "Press Run.",
            "Describe the goal — for example: 'promote a brake special'.",
            "Optionally fill in Audience — for example: 'customers who haven't visited in 6 months'.",
            "Press Run, then Send to Chat."
          ]
        }
      ],
      "examples": [
        "Goal: win back inactive customers, Audience: shop customers last seen 6+ months ago",
        "Goal: promote a brake special, Audience: local drivers on San Francisco Rd",
        "Goal: follow up after a service — remind customers about their 6-month checkup."
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "The emails sound generic.",
          "fix": "Be specific in the Goal. 'Promote a brake special' gets a generic draft; 'promote our $99 brake pad special with free installation, mention we do same-day' gets a specific one. The Audience field helps even more."
        },
        {
          "problem": "I get an error saying Goal is required.",
          "fix": "Goal is the one required field. One or two sentences is plenty — Audience is optional."
        }
      ]
    },
    "firecrawl": {
      "whoFor": "Anyone who needs the actual text of a web page instead of a wall of navigation and ads.",
      "whyUseIt": "Paste a URL and it pulls clean text and metadata. Use Crawl Site to grab a whole site. Runs locally on your machine — nothing is sent to our servers.",
      "whatItDoes": "Pulls clean content from any public web page, or crawls a site from a starting URL.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Firecrawl.",
            "Pick a tool: Fetch URL (one page) or Crawl Site (many pages).",
            "Fetch URL asks for a URL. Crawl Site asks for a Start URL and optionally Max pages (1–100, default 10).",
            "Press Run.",
            "This module runs locally, so the result tells you to use the Your Tools tile in the launchpad to actually invoke it."
          ]
        }
      ],
      "examples": [
        "Fetch URL: https://example.com/pricing",
        "Crawl Site: start_url = https://example.com, max_pages = 25"
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "The result says 'Runs locally on this machine' instead of showing the page text.",
          "fix": "That's expected — Firecrawl is one of the modules that runs on your machine rather than on our servers. Press 'Open in launchpad' and use the Your Tools tile there to invoke it. That path has the sidecar configured."
        },
        {
          "problem": "I get an error about an internal host or a private network address.",
          "fix": "Firecrawl refuses to fetch localhost, .local, .internal, and private-network addresses on purpose — a module shouldn't be able to probe the server's own network. Use a public http:// or https:// address."
        }
      ]
    },
    "leadgen": {
      "whoFor": "Sales teams, local services, B2B prospecting — anyone who needs a list of potential customers in a specific area + industry.",
      "whyUseIt": "Search for businesses by location + industry and get name, address, phone, website, and a few enrichment signals — all in one query.",
      "whatItDoes": "Find business leads by location + industry. Returns name, address, phone, website, and a few enrichment signals.",
      "windowsUI": [
        {
          "surface": "Settings → Modules",
          "steps": [
            "Set your leadgen API key in Settings → Modules.",
            "Click \"Run\" next to the card."
          ]
        }
      ],
      "examples": [
        "\"Find me all the auto repair shops in Albuquerque with under 10 Google reviews\" — quick prospecting for partnership outreach.",
        "\"Get me a list of dentists in Rio Rancho, NM\" — for a new client referral program.",
        "\"Search for HVAC contractors in 87109 zip code\" — local competitor map."
      ],
      "terminal": [
        {
          "cmd": "/leadgen search <industry> in <location>",
          "desc": "Example: /leadgen search auto repair in Albuquerque, NM"
        },
        {
          "cmd": "/leadgen check",
          "desc": "Show API key status."
        }
      ],
      "chat": [
        {
          "cmd": "@leadgen search <industry> in <location>",
          "desc": "Same but routes through MAIC chat."
        }
      ],
      "troubleshooting": [
        {
          "problem": "No results returned.",
          "fix": "Check the industry spelling. Try a broader location (state vs city)."
        }
      ]
    },
    "marketing-suite": {
      "whoFor": "Anyone who has one brief in their head and needs it to become an email, a social post, and ad copy at once.",
      "whyUseIt": "Write a one-paragraph brief and get matching emails, social posts, and ad copy. Pick a tone to set the voice.",
      "whatItDoes": "Email campaigns, social posts, and ad copy from a single brief.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Marketing Suite.",
            "Pick a tool: Draft Email or Social Post.",
            "Draft Email asks for To audience and Key points, and optionally a Tone (friendly, professional, urgent, playful).",
            "Social Post asks for a Platform (twitter, linkedin, facebook, instagram) and a Topic.",
            "Press Run, then Send to Chat."
          ]
        }
      ],
      "examples": [
        "Draft Email: to_audience = 'shop customers', key_points = 'brake pad special, free install, same-day', tone = 'friendly'",
        "Social Post: platform = instagram, topic = 'your car's 5 telltale signs of brake trouble'",
        "Draft Email: to_audience = 'past service customers', key_points = '6-month checkup reminder', tone = 'professional'"
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "The draft doesn't mention my specifics.",
          "fix": "Key points is where the specifics live — list the actual offers, prices, and service names. To audience sets who it's talking to. Be concrete; the draft mirrors what you give it."
        },
        {
          "problem": "I get an error about to_audience or key_points.",
          "fix": "Both are required for Draft Email. For Social Post, Platform and Topic are both required."
        }
      ]
    },
    "ocr": {
      "whoFor": "Anyone who reads images more than once — receipts, whiteboards, screenshots, scanned docs, photos of signs.",
      "whyUseIt": "Drop an image into chat and get the text back. No retyping, no squinting. The image goes to your MAIC vision tool, never to a third-party OCR service.",
      "whatItDoes": "Extracts all visible text from an image using MAIC vision (moondream local + gemma4:31b cloud fallback). Preserves layout as Markdown: headings, lists, paragraphs. Supports PNG, JPG, WebP, GIF, BMP up to 8 MB.",
      "windowsUI": [
        {
          "surface": "MAIC Chat",
          "steps": [
            "Drag an image file into the chat input.",
            "MC detects the format and routes to OCR.",
            "MAIC returns the extracted text with layout preserved.",
            "Click \"Insert into chat\" to use the text as context for a follow-up question."
          ]
        },
        {
          "surface": "Files page",
          "steps": [
            "Browse to an image file.",
            "Click the row → \"Extract text\" to OCR it.",
            "Text drops into the chat for follow-up questions."
          ]
        }
      ],
      "examples": [
        "Photograph a whiteboard at the end of a meeting → drop the image into chat → get the bullet points as markdown.",
        "Snap a receipt → OCR it → ask MAIC to total the items into a clean expense line.",
        "Screenshot of an error dialog → OCR → paste into MAIC chat with \"How do I fix this?\"",
        "Scan a paper contract → OCR → ask MAIC to summarize the obligations.",
        "Photograph a parking sign → OCR → translate to English (combine with Translate module)."
      ],
      "terminal": [
        {
          "cmd": "/ocr check",
          "desc": "Show OCR module status: which formats are supported, whether MAIC vision is reachable."
        },
        {
          "cmd": "/ocr extract <path>",
          "desc": "Extract text from an image file. Output goes to stdout. Example: /ocr extract ~/Desktop/receipt.png"
        }
      ],
      "chat": [
        {
          "cmd": "/ocr extract <path>",
          "desc": "Same as Terminal command but output drops into chat input."
        }
      ],
      "troubleshooting": [
        {
          "problem": "Image format not recognized.",
          "fix": "Supported: PNG, JPG, JPEG, WebP, GIF, BMP. TIFF, HEIC, RAW are not supported in v0.1.0. Convert first or use v0.2 (planned)."
        },
        {
          "problem": "Image too large.",
          "fix": "Max 8 MB. Compress with your image viewer or use a smaller format (JPG instead of PNG for photos)."
        },
        {
          "problem": "OCR result is messy or missing text.",
          "fix": "v0.1.0 uses MAIC vision, which is good at captions and decent at printed text but struggles with cursive handwriting or very small fonts. v0.2 ships local Tesseract for higher accuracy."
        },
        {
          "problem": "Module returns 'unrecognized image format' on a real image.",
          "fix": "The file extension might be wrong (e.g. PNG saved as .jpg). OCR sniffs magic bytes, not the extension — but if the bytes don't match any known format, it returns this error."
        }
      ]
    },
    "pdf": {
      "whoFor": "Anyone who reads a lot of PDFs or Word docs — contracts, manuals, invoices, research papers, reports.",
      "whyUseIt": "Drop a PDF or Word file into chat and MAIC reads it for you. Ask questions, get a summary, or extract specific info — no copy-paste, no scrolling.",
      "whatItDoes": "Drop a PDF or DOCX into MAIC chat and get clean text, a summary, or ask any question about the document. Files stay local until MAIC sees the extracted text.",
      "windowsUI": [
        {
          "surface": "MAIC Chat",
          "steps": [
            "Drag a PDF or DOCX file into the chat input.",
            "MC extracts the text and prompts you: Extract / Summarize / Ask.",
            "Pick an action or write your own question."
          ]
        },
        {
          "surface": "Files page",
          "steps": [
            "Browse to a PDF/DOCX.",
            "Click the row → \"Send to chat\" to drop the extracted text into MAIC chat."
          ]
        }
      ],
      "examples": [
        "Drop a 50-page manual into chat and ask \"What's the warranty section say?\" — instead of Ctrl+F-ing through it.",
        "Summarize a contract before you sign it.",
        "Pull all the line items out of an invoice PDF into a clean list.",
        "Compare two PDFs by extracting both and asking MAIC about the differences.",
        "Ask \"What does this error code mean in the troubleshooting guide?\" with the PDF as context."
      ],
      "terminal": [
        {
          "cmd": "/pdf check",
          "desc": "Show PDF module status: which formats are supported, whether MAIC is reachable."
        },
        {
          "cmd": "/pdf extract <path>",
          "desc": "Extract text from a file. Output goes to stdout. Example: /pdf extract ~/Documents/manual.pdf"
        },
        {
          "cmd": "/pdf summarize <path>",
          "desc": "Extract text and ask MAIC for a summary. Returns the summary as a single string."
        },
        {
          "cmd": "/pdf ask <path> <question>",
          "desc": "Extract text and ask MAIC a question about it. Example: /pdf ask contract.pdf What is the termination clause?"
        }
      ],
      "chat": [
        {
          "cmd": "/pdf extract <path>",
          "desc": "Same as Terminal command but output drops into chat input."
        },
        {
          "cmd": "/pdf summarize <path>",
          "desc": "Same."
        },
        {
          "cmd": "/pdf ask <path> <question>",
          "desc": "Same."
        }
      ],
      "troubleshooting": [
        {
          "problem": "PDF returns empty text.",
          "fix": "v0.1.0 only handles text-based PDFs. Scanned or image-only PDFs require OCR, which ships in v0.2. If you have a text PDF that's empty, the file may be corrupted — try opening it in another viewer."
        },
        {
          "problem": "DOCX extraction is missing tables or images.",
          "fix": "v0.1.0 extracts paragraphs only — tables and images are flattened or skipped. Use v0.2 (ships with pdfium-render) for layout-preserving extraction."
        },
        {
          "problem": "PDF is too large.",
          "fix": "Max context window is 32,000 characters. For larger files, the sidecar truncates with a notice. Split the PDF or ask a more specific question."
        },
        {
          "problem": "Supported formats.",
          "fix": "v0.1.0: PDF, DOCX, TXT, MD, RTF, HTML. v0.2 adds: scanned PDF (OCR), PPTX (planned)."
        }
      ]
    },
    "translate": {
      "whoFor": "Anyone working across languages — customer emails in Spanish, French supplier docs, bilingual support tickets.",
      "whyUseIt": "Paste any text and pick a language — get a translation in seconds. Uses your MAIC account, so it knows business terms and your style preferences.",
      "whatItDoes": "Translate text between 100+ languages using your local MAIC account. Drop a snippet into MAIC chat or use the standalone translate action.",
      "windowsUI": [
        {
          "surface": "MAIC Chat",
          "steps": [
            "Click 🌍 in the chat input toolbar (or type /translate).",
            "Paste text, pick the source language (or 'auto') and the target language, click Translate.",
            "Translation lands in the chat input. Edit or send."
          ]
        }
      ],
      "examples": [
        "Translate a Spanish-language customer email so you can reply in English.",
        "Translate your English product description into Portuguese for a Brazilian listing.",
        "Translate a French supplier's invoice so you can verify line items.",
        "\"Translate this contract clause into plain English\" — for legal review."
      ],
      "terminal": [
        {
          "cmd": "/translate text",
          "desc": "Open the translate dialog. Enter text, source language (or 'auto'), and target language."
        },
        {
          "cmd": "/translate check",
          "desc": "Show whether MAIC is reachable and the list of supported language codes."
        }
      ],
      "chat": [
        {
          "cmd": "/translate <text> from <src> to <tgt>",
          "desc": "Translate text inline. Example: /translate Hello world from en to es"
        },
        {
          "cmd": "@translate",
          "desc": "Open the translate dialog in chat."
        }
      ],
      "troubleshooting": [
        {
          "problem": "Translate returns \"MAIC not reachable\".",
          "fix": "MAIC backend is down or you're logged out. Check your account in Settings → Account. The translate sidecar routes through MAIC at https://maicserver.com/v1/chat/completions."
        },
        {
          "problem": "Translation looks wrong or stale.",
          "fix": "Translation uses MAIC's chat completion API and is deterministic. If a translation seems off, the model is doing its best — for technical content, try a smaller chunk or specify 'from en' explicitly to skip auto-detect."
        },
        {
          "problem": "Language code not recognized.",
          "fix": "Supported codes: es en fr de pt it nl ru zh ja ko ar hi tr pl sv da no fi el cs ro hu he th vi id ms tl uk. Anything else falls back to 'auto'."
        }
      ]
    },
    "tts": {
      "whoFor": "Anyone who reads more by listening than by looking — multitaskers, accessibility users, long technical responses, hands-free chat.",
      "whyUseIt": "MAIC's responses can be long. Instead of reading them off the screen, click a button and have MAIC speak the answer out loud. Pick a voice you like, set the speed that works for you, and chat without looking at the screen.",
      "whatItDoes": "Read MAIC's responses aloud in chat. Multiple voices, configurable speed. Bundled eSpeak NG runs offline — no audio leaves your machine.",
      "windowsUI": [
        {
          "surface": "MAIC Chat",
          "steps": [
            "After installing TTS, you'll see a 🔊 button next to each AI response.",
            "Click 🔊 to have MAIC read that response aloud.",
            "Click again to stop. Adjust voice + speed in Settings → Modules → Text-to-Speech."
          ]
        },
        {
          "surface": "Settings → Modules → Text-to-Speech",
          "steps": [
            "Pick a voice from the dropdown (en-us, en+f3 for slightly warmer, en+m4 for male, etc.).",
            "Drag the speed slider (default 175 words per minute; lower for slower, higher for faster).",
            "Click \"Test voice\" to hear a sample before saving."
          ]
        }
      ],
      "examples": [
        "Long technical explanation from MAIC? Click 🔊 and listen while you look at the related code.",
        "Hands full working on a car — keep listening to MAIC's diagnostic walkthrough instead of reading.",
        "Want to learn a new topic while walking? Read the docs with MAIC, then have MAIC explain them out loud.",
        "Accessibility: if reading long responses on screen is tiring, listen instead."
      ],
      "terminal": [
        {
          "cmd": "/tts check",
          "desc": "Show TTS status: whether eSpeak NG is bundled, current voice + speed."
        },
        {
          "cmd": "/tts speak <text>",
          "desc": "Speak a snippet of text out loud. Example: /tts speak \"Hello, this is a voice test.\""
        },
        {
          "cmd": "/tts save <text> <path.wav>",
          "desc": "Render text to a WAV file instead of playing it. Example: /tts save \"This is the spoken version\" ~/Desktop/greeting.wav"
        },
        {
          "cmd": "/tts set_voice <name>",
          "desc": "Switch voice. Names follow eSpeak conventions: en-us, en+f3 (slightly warmer), en+m4 (male), etc. Run /tts check for the full list."
        },
        {
          "cmd": "/tts set_speed <wpm>",
          "desc": "Set speaking speed in words per minute. Default 175. Try 130 for slower, 220 for faster."
        }
      ],
      "chat": [
        {
          "cmd": "/tts",
          "desc": "Toggle the per-response 🔊 button on/off. When on, every MAIC response gets a speak button."
        }
      ],
      "troubleshooting": [
        {
          "problem": "TTS audio sounds robotic.",
          "fix": "That's eSpeak NG — it's a small offline engine with intentionally synthesized voices. For more natural voices, swap to MAIC cloud TTS (v0.2) which uses neural voices."
        },
        {
          "problem": "No audio plays after clicking 🔊.",
          "fix": "Check your system volume and that the right output device is selected in Windows Sound settings. Run /tts check to confirm eSpeak NG is bundled correctly."
        },
        {
          "problem": "Voice is too fast / too slow.",
          "fix": "Open Settings → Modules → Text-to-Speech and drag the speed slider. Or run /tts set_speed 130 (slower) or /tts set_speed 220 (faster)."
        },
        {
          "problem": "Voice doesn't pronounce technical words right (API names, code, etc.).",
          "fix": "eSpeak NG does its best with technical text. For code or weird acronyms, paste the text with hyphens or spelled-out syllables. v0.2 (MAIC cloud TTS) handles technical terms much better."
        },
        {
          "problem": "🔊 button isn't appearing next to MAIC responses.",
          "fix": "Open Settings → Modules → Text-to-Speech and confirm the module is enabled. Toggle the per-response button with /tts in chat."
        }
      ]
    },
    "voice": {
      "whoFor": "Anyone who talks faster than they type, or who can't easily use a keyboard (hands full, on the move).",
      "whyUseIt": "Talk into your mic and MAIC turns your speech into text — no typing required. Everything runs locally on your machine, so no audio ever leaves your computer.",
      "whatItDoes": "Push-to-talk microphone that transcribes your speech locally with whisper.cpp and drops the text into any chat surface. No audio ever leaves your machine.",
      "windowsUI": [
        {
          "surface": "Dashboard",
          "steps": [
            "Click the 🎙 button (bottom-right of the dashboard).",
            "Speak normally — your words appear as text in a pop-up card.",
            "Edit the text if needed, then click \"Send to OpenClaw chat\" to send it as a message."
          ]
        },
        {
          "surface": "Terminal",
          "steps": [
            "Click the 🎙 Voice button in the terminal toolbar.",
            "Speak into your microphone.",
            "Transcript appears in the terminal input row. Press Enter to send."
          ]
        },
        {
          "surface": "MAIC Chat",
          "steps": [
            "Click the 🎙 button in the chat input toolbar.",
            "Speak — your words are transcribed and dropped into the chat input.",
            "Press Enter or click send."
          ]
        }
      ],
      "examples": [
        "Talk through your ideas instead of writing them out — just hit the mic and explain what you're thinking.",
        "Dictate a long email draft while looking at your notes.",
        "Use voice instead of typing when your hands are full (working on a car, eating lunch, etc.)."
      ],
      "terminal": [
        {
          "cmd": "/voice transcribe",
          "desc": "Capture up to 30 seconds of audio (stops automatically when you stop talking), return the Whisper transcript."
        },
        {
          "cmd": "/voice check",
          "desc": "Show whether the model is loaded, your mic device, audio host, and the model file location."
        },
        {
          "cmd": "/voice download_model",
          "desc": "Download the ggml-tiny.en.bin Whisper model (~75MB) from milagrocloud.com with SHA256 verification. Safe to run multiple times."
        },
        {
          "cmd": "/voice cancel",
          "desc": "Stop the current recording (if any) and return immediately."
        }
      ],
      "chat": [
        {
          "cmd": "@voice transcribe",
          "desc": "Same as /voice transcribe, but routed through MAIC chat. The transcript lands in your next message."
        }
      ],
      "troubleshooting": [
        {
          "problem": "I click the mic button and the pop-up says \"No transcript captured\".",
          "fix": "Three things can cause this, and the pop-up's Diagnostics section will tell you which one. (1) Microphone is silent or muted — check Windows Sound settings → make sure the right input device is selected and not muted. (2) Voice Activity Detector (VAD) didn't hear speech as speech — try speaking louder, getting closer to the mic, or disabling VAD in Settings → Modules → Voice. (3) Whisper heard audio but couldn't transcribe — try speaking more clearly, or re-install Voice v0.1.8 to make sure you have the latest fixes."
        },
        {
          "problem": "Pop-up says \"whisper model missing\".",
          "fix": "The Whisper model hasn't been downloaded yet. Click the card's \"Download model\" button (~75MB download), or run /voice download_model in Terminal."
        },
        {
          "problem": "Transcription is slow (45-120 seconds for short utterances).",
          "fix": "This is normal for CPU-only Whisper — your computer's processor is doing the speech recognition by itself. On most modern computers expect ~10-30× realtime (so a 10-second clip takes 0.3-1 seconds). Voice uses the tiny.en model by default for ~4× faster transcription. Switch to base.en for higher accuracy if you can spare the latency."
        },
        {
          "problem": "The 🎙 button is greyed out.",
          "fix": "Voice module isn't installed. Open Settings → Modules → Catalog → Install Voice."
        },
        {
          "problem": "Transcription captures audio but the words come out wrong.",
          "fix": "Whisper tiny.en is optimized for English and works best in quiet environments. Try moving away from background noise, or installing the larger base.en model for better accuracy (slower but more accurate)."
        }
      ]
    },
    "voice-memo": {
      "whoFor": "Anyone with their hands busy — on a lift, under a car, halfway through a part — who still needs to note something down.",
      "whyUseIt": "Record a voice memo from the chat input mic icon and it transcribes it locally and pulls out the action items. Audio never leaves your device.",
      "whatItDoes": "Transcribes audio memos and extracts action items, using the local whisper.cpp sidecar.",
      "windowsUI": [
        {
          "surface": "Chat toolbar → Modules",
          "steps": [
            "Click the 🧩 Modules button, then click Voice Memo.",
            "Press Run.",
            "Paste a public URL to an audio file into Audio file URL (.m4a, .mp3, or .wav).",
            "Press Run.",
            "This module runs locally — press 'Open in launchpad' to reach the Your Tools tile that drives the whisper.cpp sidecar."
          ]
        }
      ],
      "examples": [
        "Audio file URL: https://example.com/voice-note.m4a",
        "Record straight from the chat input mic icon, then transcribe the recording."
      ],
      "terminal": [],
      "chat": [],
      "troubleshooting": [
        {
          "problem": "The result says 'Runs locally on this machine' instead of a transcript.",
          "fix": "Expected — Voice Memo transcribes through the local whisper.cpp sidecar, not a server. Press 'Open in launchpad' and use the Your Tools tile there. Audio stays on your machine either way."
        },
        {
          "problem": "My audio URL won't work.",
          "fix": "It needs to be a public http:// or https:// address pointing directly at the audio file, not a share page. Common formats: .m4a, .mp3, .wav."
        }
      ]
    },
    "youtube": {
      "whoFor": "Anyone who watches tutorials, interviews, lectures, podcasts-on-YouTube — and wants the content as text they can search, quote, or summarize.",
      "whyUseIt": "Paste a YouTube URL and get the full transcript, a chapter breakdown, or an AI-generated summary. Saves the transcript locally so you can search it later.",
      "whatItDoes": "Paste a YouTube URL and get the full transcript, a chapter breakdown, and an AI-generated summary. Saves the transcript locally so you can search it later.",
      "windowsUI": [
        {
          "surface": "MAIC Chat",
          "steps": [
            "Paste a YouTube URL into the chat input.",
            "MC detects the URL and prompts: Transcript / Summary / Chapters.",
            "Pick an action or ask a question about the video."
          ]
        }
      ],
      "examples": [
        "Paste a 2-hour podcast URL and ask \"What did they say about hiring?\" — instead of watching the whole thing.",
        "Get a chapter breakdown of a long tutorial so you can jump to the section you need.",
        "Summarize a keynote speech into 5 bullet points.",
        "Pull a timestamped transcript of a video you want to quote in a blog post."
      ],
      "terminal": [
        {
          "cmd": "/youtube transcript <url>",
          "desc": "Fetch the transcript (with timestamps) for a YouTube video."
        },
        {
          "cmd": "/youtube summary <url>",
          "desc": "Fetch the transcript and ask MAIC for a summary."
        },
        {
          "cmd": "/youtube chapters <url>",
          "desc": "Return the chapter breakdown with timestamps."
        },
        {
          "cmd": "/youtube check",
          "desc": "Show module status: yt-dlp available, last fetch status."
        }
      ],
      "chat": [
        {
          "cmd": "/youtube transcript <url>",
          "desc": "Same as Terminal — output drops into chat input."
        },
        {
          "cmd": "/youtube summary <url>",
          "desc": "Same."
        }
      ],
      "troubleshooting": [
        {
          "problem": "Transcript unavailable for this video.",
          "fix": "Some videos have transcripts disabled by the uploader. If the video has auto-generated captions, the sidecar falls back to those. Otherwise the sidecar can route the video's audio through Whisper (slow)."
        },
        {
          "problem": "Module not installed.",
          "fix": "If the YouTube Transcript card says \"Coming Soon\" instead of \"Install\", the module hasn't shipped in your current version of MC. Check for an update in Settings → Modules."
        }
      ]
    }
  };
  // <<< END GENERATED: MODULE-HELP <<<

  var _modulesCountInflight = false;

  async function fetchInstalledModulesMerged() {
    // Lesson 975: read remote (MAIC) install list. The Tauri command
    // is the same one the rest of MC uses. If it fails, fall back to
    // the local registry (mc_module_list) so the dropdown never goes
    // blank when MAIC is down (Lesson 1109, 2026-10-02, David —
    // market "it used to work, fix the marketplaces so this can't
    // happen again"). The local list covers modules discovered by
    // the bundled installer; the remote list covers installs done
    // via MAIC's /v1/modules/install. Merge by id, preferring the
    // remote version (it has the live version string + client_manifest).
    var remote = [];
    var remoteOk = false;
    try {
      var resp = await tauriInvoke('mc_list_installed_modules');
      remote = Array.isArray(resp) ? resp
        : Array.isArray(resp && resp.modules) ? resp.modules
        : Array.isArray(resp && resp.installed) ? resp.installed
        : [];
      remoteOk = true;
    } catch (e) {
      console.warn('[mc-toolbar] mc_list_installed_modules failed:', e);
    }

    var local = [];
    try {
      var localResp = await tauriInvoke('mc_module_list');
      local = Array.isArray(localResp) ? localResp
        : Array.isArray(localResp && localResp.modules) ? localResp.modules
        : [];
    } catch (e) {
      // mc_module_list reads from the in-memory registry (no network)
      // — failure here is genuinely unexpected.
      console.warn('[mc-toolbar] mc_module_list failed:', e);
    }

    if (remote.length === 0 && local.length > 0) {
      // rc57.13 (2026-10-07, David): the previous version of this
      // block ONLY fell back to local when !remoteOk, which left
      // users with bundled/standalone modules but no MAIC
      // `module_installs` row seeing an empty Modules dropdown
      // ("No modules installed."). David's account (user 26317)
      // hits this: MAIC `/v1/modules/installed` returns [] because
      // his 8 modules are registered but never installed via MAIC.
      // The bundled registry (mc_module_list) IS the source of truth
      // for users without a MAIC install record — fall back to it
      // whenever remote is empty, regardless of whether MAIC returned
      // 200 or threw. Tag each with `_offline` so the meta line
      // shows "(offline copy)" for diagnostic clarity.
      return local.map(function (m) {
        var n = normalizeLocalModule(m);
        n._offline = true;
        return n;
      });
    }

    if (remote.length === 0) {
      return [];
    }

    // Both sources responded: merge by canonical id, MAIC-first.
    // Lesson 1136 (2026-10-08, David): `email-campaigns-pro` (MAIC)
    // and `email` (local) are the same module; without the canonical
    // id map, both showed up in the dropdown. Now we collapse them
    // and prefer the MAIC record (live version + client_manifest).
    var byCanonical = new Map();
    local.forEach(function (m) {
      var n = normalizeLocalModule(m);
      byCanonical.set(canonicalId(n.id), n);
    });
    var seenCanonical = new Set();
    var merged = [];
    remote.forEach(function (m) {
      var n = normalizeRemoteModule(m);
      var canon = canonicalId(n.id);
      var l = byCanonical.get(canon);
      if (l && !n.version) n.version = l.version;
      merged.push(n);
      seenCanonical.add(canon);
    });
    // Local-only modules (no MAIC counterpart) get appended with the
    // (offline copy) tag so the user can tell them apart from MAIC-
    // managed modules. These are real installs on disk; hiding them
    // would lose access to tools the user explicitly installed.
    local.forEach(function (m) {
      var n = normalizeLocalModule(m);
      var canon = canonicalId(n.id);
      if (seenCanonical.has(canon)) return;
      n._offline = true;
      merged.push(n);
    });
    return merged;
  }

  function normalizeLocalModule(m) {
    // mc_module_list returns the runtime registry shape. Best-effort
    // mapping; the toolbar uses {id, name, version, icon, tools}.
    // rc57.13: include `description` (top-level + client_manifest fallback)
    // so the dropdown card can show the same one-line description the
    // Terminal launchpad card does, even for bundled/local-only modules
    // where MAIC has no record.
    var cm = m && m.client_manifest ? m.client_manifest : {};
    var desc = m.description || cm.description || '';
    var n = {
      id: m.id || m.module_id || '(unknown)',
      name: m.name || m.title || m.module_id || m.id || '(unknown)',
      version: m.version || '',
      hooks: m.hooks || m.ui_hooks || {},
      icon: m.icon || '🧩',
      tools: m.tools || m._raw_tools || [],
      _raw: m,
    };
    if (desc) n.description = desc;
    return n;
  }

  function normalizeRemoteModule(m) {
    var cm = m && m.client_manifest ? m.client_manifest : {};
    var n = {
      id: m.module_id || m.id || cm.module_id || cm.id || '(unknown)',
      name: cm.name || m.name || m.module_id || m.id || '(unknown)',
      version: cm.version || m.version || '',
      hooks: cm.ui_hooks || m.hooks || {},
      icon: m.icon || cm.icon || '🧩',
      tools: cm.tools || m.tools || cm._raw_tools || [],
      _raw: m,
    };
    // Lesson 1144 (2026-11-02, David): the remote normaliser DROPPED
    // `description` while the local one kept it, so every MAIC-managed
    // module's dropdown card showed no description while bundled/local
    // modules did. Keep the top-level description from the API row, then
    // fall back to the client_manifest copy.
    var desc = m.description || cm.description || '';
    if (desc) n.description = desc;
    // Keep the category so the info modal can show it.
    var category = m.category || cm.category || '';
    if (category) n.category = category;
    return n;
  }

  async function refreshModulesCount() {
    if (_modulesCountInflight) return;
    _modulesCountInflight = true;
    try {
      var list = await fetchInstalledModulesMerged();
      var badge = document.getElementById('mc-oc-modules-count');
      if (!badge) return;
      if (list.length === 0) {
        badge.setAttribute('hidden', '');
      } else {
        badge.textContent = String(list.length);
        badge.removeAttribute('hidden');
      }
    } catch (e) {
      var badge2 = document.getElementById('mc-oc-modules-count');
      if (badge2) badge2.setAttribute('hidden', '');
    } finally {
      _modulesCountInflight = false;
    }
  }

  async function openModulesMenu() {
    var dropdown = document.getElementById('mc-oc-modules-menu');
    if (!dropdown) return;
    if (!dropdown.hasAttribute('hidden')) {
      closeDropdown();
      return;
    }
    var body = dropdown.querySelector('.mc-oc-dropdown-body');
    body.innerHTML = '<div class="mc-oc-dropdown-empty">Loading...</div>';
    openDropdownFor(document.getElementById('mc-oc-modules-btn'), dropdown);

    var list = await fetchInstalledModulesMerged();
    if (list.length === 0) {
      body.innerHTML = '<div class="mc-oc-dropdown-empty">No modules installed.<br>Install from the launchpad marketplace.</div>';
      return;
    }
    body.innerHTML = '';
    list.forEach(function (m) {
      var item = document.createElement('button');
      item.type = 'button';
      item.className = 'mc-oc-dropdown-item';
      item.setAttribute('data-module-id', m.id);
      var iconEl = document.createElement('span');
      iconEl.className = 'mc-oc-dropdown-item-icon';
      iconEl.textContent = m.icon;
      var bodyEl = document.createElement('span');
      bodyEl.className = 'mc-oc-dropdown-item-body';
      var nameEl = document.createElement('div');
      nameEl.className = 'mc-oc-dropdown-item-name';
      nameEl.textContent = m.name;
      // rc57.13 (2026-10-07, David): show the module description under
      // the name so the dropdown matches the Terminal launchpad card
      // ("Marketing Suite · Email campaigns, social posts, and ad copy
      // from a single brief") instead of looking like a generic icon +
      // Run button row. Description comes from the MAIC row's top-level
      // `description` column; falls back to client_manifest.description.
      // For local-only modules (no MAIC record), the normalized form
      // also copies the description into `m.description`. Capped at
      // 100 chars + ellipsis to keep the dropdown scannable.
      var rawModule = m._raw || {};
      var rawCm = rawModule.client_manifest || {};
      var moduleDesc = (typeof m.description === 'string' ? m.description : '')
        || (typeof rawModule.description === 'string' ? rawModule.description : '')
        || (typeof rawCm.description === 'string' ? rawCm.description : '')
        || '';
      if (moduleDesc) {
        var descEl = document.createElement('div');
        descEl.className = 'mc-oc-dropdown-item-desc';
        descEl.textContent = moduleDesc.slice(0, 100) + (moduleDesc.length > 100 ? '…' : '');
        bodyEl.appendChild(nameEl);
        bodyEl.appendChild(descEl);
      } else {
        bodyEl.appendChild(nameEl);
      }
      var metaEl = document.createElement('div');
      metaEl.className = 'mc-oc-dropdown-item-meta';
      var toolCount = m.tools.length;
      // rc57.13: when the module has 2+ tools, list them by name in the
      // meta line so users can see what they'd run without picking
      // through a tool picker first. Same cap+ellipsis logic as the
      // description so the line stays scannable.
      var toolLabel;
      if (toolCount === 0) {
        toolLabel = 'no tools -- open in launchpad';
      } else if (toolCount === 1) {
        toolLabel = '1 tool: ' + (m.tools[0].name || '?');
      } else {
        var names = m.tools.map(function (t) { return t.name || t.id || '?'; });
        var joined = names.join(', ');
        toolLabel = toolCount + ' tools: ' + (joined.length > 80 ? joined.slice(0, 77) + '…' : joined);
      }
      metaEl.textContent = 'v' + (m.version || '?') + ' · ' + toolLabel + (m._offline ? ' (offline copy)' : '');
      bodyEl.appendChild(metaEl);
      var runBtn = document.createElement('span');
      runBtn.className = 'mc-oc-dropdown-item-run';
      runBtn.textContent = toolCount > 0 ? 'Run →' : 'Info →';
      runBtn.setAttribute('role', 'button');
      item.appendChild(iconEl);
      item.appendChild(bodyEl);
      item.appendChild(runBtn);
      // Lesson 1144 (2026-11-02, David): the chat toolbar had NO
      // module-level Help affordance at all. The Modules page card has a
      // "❔ Help" button, but a user working in the chat window had no
      // way to read the module's instructions, examples, or
      // troubleshooting notes without leaving the conversation.
      //
      // The help button is a SEPARATE control from the card button: the
      // card's own click handler runs the module, so the help button
      // must stopPropagation or every Help click would launch a run.
      var helpBtn = document.createElement('span');
      helpBtn.className = 'mc-oc-dropdown-item-help';
      helpBtn.textContent = '❔';
      helpBtn.setAttribute('role', 'button');
      helpBtn.setAttribute('tabindex', '0');
      helpBtn.setAttribute('aria-label', 'Help for ' + m.name);
      helpBtn.title = 'How to use ' + m.name;
      var openHelp = function (ev) {
        if (ev) {
          ev.preventDefault();
          ev.stopPropagation();
        }
        closeDropdown();
        openModuleHelpModal(m);
      };
      helpBtn.addEventListener('click', openHelp);
      helpBtn.addEventListener('keydown', function (ev) {
        if (ev.key === 'Enter' || ev.key === ' ') openHelp(ev);
      });
      item.appendChild(helpBtn);
      item.addEventListener('click', function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        closeDropdown();
        if (m.tools.length === 0) {
          openModuleInfoModal(m);
        } else {
          openModuleRunModal(m);
        }
      });
      body.appendChild(item);
    });
  }

  // ---------------------------------------------------------------------
  // Humanize a module-run failure (Lesson 1144, 2026-11-02, David).
  //
  // The toolbar used to surface the raw ureq error verbatim:
  //
  //   Run failed: /v1/modules/calendar/run POST: HTTP 422:
  //   {"detail":"q is required."}
  //
  // That's a URL, a method, a status, and a JSON blob -- none of which
  // tells the user what to DO. Worse, a 404 from a malformed module/tool
  // pair came out as "Unknown module/tool: calendar/undefined", which
  // reads like the user's fault.
  //
  // This maps the common cases to something a shop owner can act on,
  // and keeps the raw detail underneath for when it's still useful.
  // ---------------------------------------------------------------------
  function humanizeRunError(e, m, toolName) {
    var raw = (e && e.message) ? e.message : String(e || 'Unknown error');
    var modName = (m && m.name) || 'the module';
    var toolLabel = toolName || 'this tool';

    // Pull the HTTP status out of the ureq error string.
    var statusMatch = raw.match(/HTTP\s+(\d{3})/);
    var status = statusMatch ? parseInt(statusMatch[1], 10) : 0;

    // Pull a JSON `detail` / `message` / `error` field out of the body,
    // if there is one.
    var detail = '';
    var bodyMatch = raw.match(/\{[\s\S]*\}\s*$/);
    if (bodyMatch) {
      try {
        var parsed = JSON.parse(bodyMatch[0]);
        if (parsed && typeof parsed === 'object') {
          detail = parsed.detail || parsed.message || parsed.error || '';
          if (Array.isArray(detail)) {
            // FastAPI validation errors: [{loc:[...], msg:"...", type:"..."}]
            detail = detail
              .map(function (d) {
                if (!d) return '';
                var field = Array.isArray(d.loc) ? d.loc[d.loc.length - 1] : '';
                var msg = d.msg || d.detail || '';
                return field ? field + ': ' + msg : msg;
              })
              .filter(Boolean)
              .join('; ');
          }
        }
      } catch (e2) {
        detail = '';
      }
    }
    if (!detail) {
      // No JSON body -- strip the noisy request prefix and use the rest.
      var stripped = raw
        .replace(/^https?:\/\/\S+\s*/, '')
        .replace(/^[\w/.\-]+\s+(GET|POST|PUT|PATCH|DELETE)\s*:\s*/i, '')
        .replace(/^HTTP\s+\d{3}:\s*/i, '')
        .trim();
      detail = stripped && stripped !== raw ? stripped : raw;
    }

    var headline;
    if (status === 404) {
      // Most 404s here are the malformed module/tool pair from the
      // picker bug. Say so plainly and point at the retry.
      if (/unknown module\/tool/i.test(detail)) {
        headline =
          'The server did not recognize that tool. Close this and pick the tool again from the module list.';
      } else {
        headline = 'The module or tool was not found on the server.';
      }
    } else if (status === 400 || status === 422) {
      headline = detail
        ? 'The module rejected those values: ' + detail
        : 'The module rejected those values.';
    } else if (status === 401 || status === 403) {
      headline =
        'Your session is no longer valid. Sign in again in Miracle Claw, then run this tool once more.';
    } else if (status === 501) {
      headline = detail || 'This tool is not available yet.';
    } else if (status === 502 || status === 503 || status === 504) {
      // Lesson 1146 (2026-11-02, David): 502/503/504 are "temporarily
      // unavailable", matching the shared mapper in
      // src/module-run-shared.js so all three surfaces say the same
      // thing about the same failure.
      headline = 'MAIC is temporarily unavailable. Try again in a moment.';
    } else if (status === 429) {
      headline = 'Too many requests. Wait a moment and try again.';
    } else if (status >= 500) {
      headline = 'MAIC is temporarily unavailable. Try again in a moment.';
    } else if (status === 0) {
      headline =
        'Could not reach the MAIC server. Check your connection and retry.';
    } else {
      headline = 'Could not run ' + toolLabel + '.';
    }

    // 501 COMING_SOON gets a distinct, friendly rendering: the server
    // tells us exactly what to do, so pass it through verbatim rather
    // than burying it in "could not run".
    //
    // Lesson 1146: the headline now matches the shared mapper --
    // "not available yet -- in private beta" -- and the detail carries
    // the waitlist link so the user can act on it.
    if (status === 501) {
      return {
        headline:
          'That tool is not available yet -- it is in private beta. The waitlist link is in the details below.',
        detail: detail,
        actionable: true,
      };
    }

    return {
      headline: headline,
      detail: status ? 'HTTP ' + status + (detail ? ' · ' + detail : '') : detail,
      actionable: false,
    };
  }

  // ---------------------------------------------------------------------
  // Module help overlay (Lesson 1144, 2026-11-02, David).
  //
  // The chat toolbar's Modules dropdown previously had no module-level
  // help at all -- only per-field descriptions on the run form. A user
  // who wanted to know what a module actually DID, what to type into
  // it, or why it errored had no in-chat answer.
  //
  // Content comes from TOOLBAR_MODULE_HELP, which is GENERATED from
  // src/data/module-help.js + src/data/maic-module-help.js by
  // scripts/sync-toolbar-module-help.mjs. The toolbar has to be
  // self-contained (it runs inside the OpenClaw dist page and cannot
  // import from MC's src/), so this is a generated copy -- keep the
  // canonical files as the source of truth and re-run the sync script.
  // ---------------------------------------------------------------------
  function helpIdFor(m) {
    // Try the module's own id first, then the alias map in both
    // directions, so a locally-bundled module still finds its help.
    var candidates = [];
    if (m && m.id) candidates.push(m.id);
    var raw = (m && m._raw) || {};
    if (raw.module_id) candidates.push(raw.module_id);
    if (raw.id) candidates.push(raw.id);
    for (var c = 0; c < candidates.length; c++) {
      var id = candidates[c];
      if (TOOLBAR_MODULE_HELP[id]) return id;
      var canon = canonicalId(id);
      if (TOOLBAR_MODULE_HELP[canon]) return canon;
    }
    return null;
  }

  function helpEl(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function openModuleHelpModal(m) {
    var helpId = helpIdFor(m);
    var help = helpId ? TOOLBAR_MODULE_HELP[helpId] : null;

    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel mc-oc-help-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');

    var title = helpEl('h2', 'mc-oc-modal-title', '❔ ' + (m.name || helpId || 'Module'));
    panel.appendChild(title);

    if (!help) {
      // No help content for this module. Don't dead-end the user --
      // fall back to the live manifest instructions, which every MAIC
      // module carries in client_manifest.instructions.
      var raw = (m && m._raw) || {};
      var cm = raw.client_manifest || {};
      var instructions = cm.instructions || (m && m.instructions) || '';
      var rawModule = raw || {};
      var moduleDesc =
        (typeof m.description === 'string' ? m.description : '') ||
        (typeof rawModule.description === 'string' ? rawModule.description : '') ||
        (typeof cm.description === 'string' ? cm.description : '') ||
        '';
      panel.appendChild(helpEl('p', 'mc-oc-modal-subtitle', moduleDesc || 'No description provided.'));
      if (instructions) {
        var inst = helpEl('details', 'mc-oc-modal-instructions');
        inst.appendChild(helpEl('summary', null, 'How to use ' + (m.name || 'this module')));
        var ip = helpEl('p', null, instructions);
        inst.appendChild(ip);
        panel.appendChild(inst);
      }
      var toolNames = (m.tools || []).map(function (t) { return t.name || t.id; }).filter(Boolean);
      if (toolNames.length) {
        var toolsWrap = helpEl('div', 'mc-oc-help-section');
        toolsWrap.appendChild(helpEl('h3', 'mc-oc-help-h3', 'Tools'));
        var ul = helpEl('ul', 'mc-oc-help-list');
        toolNames.forEach(function (n) { ul.appendChild(helpEl('li', null, n)); });
        toolsWrap.appendChild(ul);
        panel.appendChild(toolsWrap);
      }
    } else {
      panel.appendChild(helpEl('p', 'mc-oc-modal-subtitle', help.whatItDoes || ''));

      if (help.whoFor || help.whyUseIt) {
        var intro = helpEl('div', 'mc-oc-help-section mc-oc-help-intro');
        if (help.whoFor) {
          var who = helpEl('div', 'mc-oc-help-line');
          who.appendChild(helpEl('span', 'mc-oc-help-label', "Who's it for:"));
          who.appendChild(helpEl('span', null, ' ' + help.whoFor));
          intro.appendChild(who);
        }
        if (help.whyUseIt) {
          var why = helpEl('div', 'mc-oc-help-line');
          why.appendChild(helpEl('span', 'mc-oc-help-label', "Why you'd use it:"));
          why.appendChild(helpEl('span', null, ' ' + help.whyUseIt));
          intro.appendChild(why);
        }
        panel.appendChild(intro);
      }

      if (help.windowsUI && help.windowsUI.length) {
        var uiSec = helpEl('div', 'mc-oc-help-section');
        uiSec.appendChild(helpEl('h3', 'mc-oc-help-h3', '🖥️ Using it in Miracle Claw'));
        help.windowsUI.forEach(function (s) {
          var sub = helpEl('div', 'mc-oc-help-subsection');
          sub.appendChild(helpEl('h4', 'mc-oc-help-h4', s.surface || ''));
          var ol = helpEl('ol', 'mc-oc-help-steps');
          (s.steps || []).forEach(function (st) { ol.appendChild(helpEl('li', null, st)); });
          sub.appendChild(ol);
          uiSec.appendChild(sub);
        });
        panel.appendChild(uiSec);
      }

      if (help.examples && help.examples.length) {
        var exSec = helpEl('div', 'mc-oc-help-section');
        exSec.appendChild(helpEl('h3', 'mc-oc-help-h3', '💡 Example things to ask MAIC'));
        var exUl = helpEl('ul', 'mc-oc-help-list');
        help.examples.forEach(function (ex) {
          var li = helpEl('li', null, ex);
          exUl.appendChild(li);
        });
        exSec.appendChild(exUl);
        panel.appendChild(exSec);
      }

      var cmdSec = null;
      function addCmdSection(titleText, list) {
        if (!list || !list.length) return;
        if (!cmdSec) cmdSec = helpEl('div', 'mc-oc-help-section');
        cmdSec.appendChild(helpEl('h3', 'mc-oc-help-h3', titleText));
        var ul = helpEl('ul', 'mc-oc-help-list');
        list.forEach(function (c) {
          var li = helpEl('li', 'mc-oc-help-cmd-item');
          li.appendChild(helpEl('code', 'mc-oc-help-cmd', c.cmd || ''));
          li.appendChild(helpEl('span', 'mc-oc-help-cmd-desc', c.desc || ''));
          ul.appendChild(li);
        });
        cmdSec.appendChild(ul);
      }
      addCmdSection('⌨️ Command-line reference', help.terminal);
      addCmdSection('💬 Chat commands', help.chat);
      if (cmdSec) panel.appendChild(cmdSec);

      if (help.troubleshooting && help.troubleshooting.length) {
        var trSec = helpEl('div', 'mc-oc-help-section');
        trSec.appendChild(helpEl('h3', 'mc-oc-help-h3', '🛠️ Troubleshooting'));
        help.troubleshooting.forEach(function (t) {
          var item = helpEl('div', 'mc-oc-help-trouble');
          item.appendChild(helpEl('div', 'mc-oc-help-trouble-problem', t.problem || ''));
          item.appendChild(helpEl('div', 'mc-oc-help-trouble-fix', t.fix || ''));
          trSec.appendChild(item);
        });
        panel.appendChild(trSec);
      }
    }

    var actions = helpEl('div', 'mc-oc-modal-actions');
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () {
      document.body.removeChild(backdrop);
    });
    actions.appendChild(close);

    // If the module has runnable tools, offer "Run it" from the help
    // overlay too -- the user usually opened help because they were
    // about to run it.
    if (m.tools && m.tools.length) {
      var runBtn2 = document.createElement('button');
      runBtn2.type = 'button';
      runBtn2.className = 'mc-oc-modal-button is-primary';
      runBtn2.textContent = 'Run it →';
      runBtn2.addEventListener('click', function () {
        document.body.removeChild(backdrop);
        openModuleRunModal(m);
      });
      actions.appendChild(runBtn2);
    }
    panel.appendChild(actions);
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);
  }

  function openModuleInfoModal(m) {
    // rc57.13: show the module description as the modal subtitle so the
    // info modal matches the Terminal launchpad card. Before rc57.13
    // the toolbar showed a generic "no client-side tools" line for every
    // sidecar (firecrawl, calendar, etc.) with no other context. Now
    // users see the module's description + category before the hint.
    var rawModule = m._raw || {};
    var rawCm = rawModule.client_manifest || {};
    var moduleDesc = (typeof m.description === 'string' ? m.description : '')
      || (typeof rawModule.description === 'string' ? rawModule.description : '')
      || (typeof rawCm.description === 'string' ? rawCm.description : '')
      || 'No description provided.';
    var category = rawModule.category || rawCm.category || m.category || '';
    var subtitleParts = [];
    if (category) subtitleParts.push('[' + category + ']');
    subtitleParts.push(moduleDesc);
    var subtitle = subtitleParts.join(' ');
    // Show a single non-clickable hint row so users know what to do.
    // We render it as a <p> via the existing subtitle channel and pass
    // empty rows so showModuleModal only shows the Close button.
    showModuleModal(
      m.name + ' (v' + (m.version || '?') + ')',
      subtitle + '\n\nThis module does not expose client-side tools. Open it in the launchpad to run.',
      [],
      null
    );
  }

  async function openModuleRunModal(m) {
    // If the module has exactly one tool, skip the picker and go
    // straight to the run form. Otherwise show a tool picker.
    //
    // Lesson 1144 (2026-11-02, David): the tool picker used to map
    // each tool down to a flat `{label, value}` row and then pass THAT
    // row straight to showRunForm(). showRunForm() expects the MAIC
    // tool object (`tool.name`, `tool.description`, `tool.form_schema`),
    // so every multi-tool module -- calendar (15 tools), firecrawl
    // (2), marketing-suite (2) -- rendered a raw JSON textarea instead
    // of its real fields, and then sent `tool: tool.name || tool.id`
    // with both undefined. The server rejected that with
    //   404 Unknown module/tool: calendar/undefined
    // which is exactly the "I filled in the fields and Search gave me
    // an error" report. The fix: keep the ORIGINAL tool object on the
    // row (as `row.tool`) and hand THAT to showRunForm().
    var tool = m.tools[0];
    if (m.tools.length === 1) {
      showRunForm(m, tool);
    } else {
      showModuleModal(
        m.name + ' -- pick a tool',
        'Choose a tool to run.',
        m.tools.map(function (t) {
          return {
            label: t.name || t.id,
            value: t.id || t.name,
            // Carry the real tool through the picker so showRunForm()
            // gets `name` / `description` / `form_schema`.
            tool: t,
          };
        }),
        function (picked) {
          showRunForm(m, (picked && picked.tool) || picked);
        }
      );
    }
  }

  function showModuleModal(title, subtitle, rows, onPick) {
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = title;
    panel.appendChild(h);
    if (subtitle) {
      var sub = document.createElement('p');
      sub.className = 'mc-oc-modal-subtitle';
      sub.textContent = subtitle;
      panel.appendChild(sub);
    }
    if (rows && rows.length > 0) {
      rows.forEach(function (r) {
        var row = document.createElement('button');
        row.type = 'button';
        row.className = 'mc-oc-modal-row';
        row.style.cursor = 'pointer';
        row.style.width = '100%';
        row.style.textAlign = 'left';
        var name = document.createElement('span');
        name.className = 'mc-oc-modal-row-name';
        name.textContent = r.label;
        var val = document.createElement('span');
        val.className = 'mc-oc-modal-row-value';
        val.textContent = r.value || '';
        row.appendChild(name);
        row.appendChild(val);
        row.addEventListener('click', function () {
          document.body.removeChild(backdrop);
          if (onPick) onPick(r);
        });
        panel.appendChild(row);
      });
    }
    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () {
      document.body.removeChild(backdrop);
    });
    actions.appendChild(close);
    panel.appendChild(actions);
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);
  }

  function showRunForm(m, tool) {
    // Lesson 1144 (2026-11-02, David): defensive -- a missing tool must
    // NEVER reach mc_run_module as `tool: undefined`. The server would
    // answer 404 "Unknown module/tool: <id>/undefined", which is a
    // confusing dead end for the user. If we somehow got here without a
    // usable tool, tell them plainly instead.
    if (!tool || !(tool.name || tool.id)) {
      showToast(
        'This module has no runnable tools. Open it in the launchpad to install its sidecar.',
        'error'
      );
      return;
    }
    var toolName = tool.name || tool.id;
    // Lesson 985 follow-up (2026-09-16): the toolbar used to render a
    // single JSON-textarea modal that "ran" the module but pasted the
    // result into an obscure chat-input selector chain. Two problems:
    //   1. Users saw a different popup from the Launchpad, and
    //   2. Even when the run succeeded, the paste target might not be
    //      found (openclaw chat composer doesn't match any of the
    //      placeholder selectors), so users saw no feedback at all.
    // Fix: render the SAME form-schema fields the Launchpad /
    // modules-menu use, and on Run show the result in a result area
    // with Send-to-Chat / Copy / Close buttons -- same UX as the
    // Launchpad so users get a consistent experience.
    // rc57.21: add a <details class="mc-oc-modal-instructions"> block
    // reading m._raw.client_manifest.instructions, matching the
    // launchpad (dashboard_launchpad.js:519-523) and the Terminal
    // toolbar (modules-menu.js, rc57.20). Same data path -- the
    // dropdown's normalised m shape carries _raw, and _raw.client_manifest
    // has the instructions string MAIC ships with every module.
    // The instruction text is HTML-escaped via the same escapeHtml()
    // helper used everywhere else, capped at 2000 chars to prevent
    // runaway schemas, and rendered with white-space: pre-line so
    // blank lines act as paragraph breaks (matches the Terminal's
    // .mc-modal-instructions p rule).
    var rawModuleForInstructions = m._raw || {};
    var rawCmForInstructions = rawModuleForInstructions.client_manifest || {};
    var instructions = (typeof rawCmForInstructions.instructions === 'string'
      ? rawCmForInstructions.instructions
      : '');
    if (instructions) instructions = instructions.trim().slice(0, 2000);
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = 'Run ' + m.name + ' ? ' + toolName;
    panel.appendChild(h);
    var sub = document.createElement('p');
    sub.className = 'mc-oc-modal-subtitle';
    var desc = (tool.description || '').slice(0, 200);
    sub.textContent = desc || 'Provide inputs and run. The result shows below.';
    panel.appendChild(sub);
    if (instructions) {
      var det = document.createElement('details');
      det.className = 'mc-oc-modal-instructions';
      var summary = document.createElement('summary');
      summary.textContent = 'How to use ' + m.name;
      det.appendChild(summary);
      var p = document.createElement('p');
      p.innerHTML = escapeHtml(instructions);
      det.appendChild(p);
      panel.appendChild(det);
    }

    var form = document.createElement('form');
    form.className = 'mc-oc-modal-form';
    form.setAttribute('data-module', m.id);
    form.setAttribute('data-tool', toolName);

    // Render form_schema fields if we have them. Otherwise show a
    // JSON textarea (rare -- most modules publish a schema).
    var schema = tool.form_schema || (tool.inputs && tool.inputs.form_schema) || null;
    var renderedFields = false;
    if (schema && typeof schema === 'object' && schema.type === 'object' && schema.properties) {
      try {
        var props = schema.properties || {};
        var required = Array.isArray(schema.required) ? schema.required : [];
        Object.keys(props).forEach(function (name) {
          var prop = props[name];
          if (!prop || typeof prop !== 'object') return;
          var field = document.createElement('div');
          field.className = 'mc-oc-modal-field';
          var labelEl = document.createElement('label');
          labelEl.textContent = prop.title || name;
          if (required.indexOf(name) !== -1) {
            var req = document.createElement('span');
            req.className = 'mc-oc-modal-req';
            req.textContent = ' *';
            labelEl.appendChild(req);
          }
          labelEl.setAttribute('for', 'mc-oc-field-' + name);
          field.appendChild(labelEl);
          var inputEl;
          var t = prop.type || 'string';
          if (Array.isArray(prop.enum) && prop.enum.length > 0) {
            inputEl = document.createElement('select');
            inputEl.innerHTML = '<option value="">-- select --</option>' +
              prop.enum.map(function (v) { return '<option value="' + escapeHtml(v) + '">' + escapeHtml(v) + '</option>'; }).join('');
          } else if (t === 'number' || t === 'integer') {
            inputEl = document.createElement('input');
            inputEl.type = 'number';
            inputEl.step = t === 'integer' ? '1' : 'any';
            if (prop.minimum !== undefined) inputEl.min = String(prop.minimum);
            if (prop.maximum !== undefined) inputEl.max = String(prop.maximum);
          } else if (t === 'boolean') {
            inputEl = document.createElement('input');
            inputEl.type = 'checkbox';
            inputEl.value = 'true';
          } else {
            inputEl = document.createElement('input');
            inputEl.type = 'text';
            if (prop.minLength !== undefined) inputEl.minLength = prop.minLength;
            if (prop.maxLength !== undefined) inputEl.maxLength = prop.maxLength;
          }
          inputEl.id = 'mc-oc-field-' + name;
          inputEl.name = name;
          inputEl.dataset.fieldType = t;
          if (prop.default !== undefined && prop.default !== null) {
            if (t === 'boolean') inputEl.checked = !!prop.default;
            else inputEl.value = String(prop.default);
          }
          if (required.indexOf(name) !== -1) inputEl.required = true;
          // rc57.12: apply placeholder (trimmed, capped 120 chars) on
          // text/textarea/select so users see an inline example.
          // Lesson 1132 — same rules as dashboard_launchpad.js
          // renderField(): trim then cap to prevent runaway schemas.
          if (prop.placeholder !== undefined && prop.placeholder !== null) {
            var phStr = String(prop.placeholder).trim().slice(0, 120);
            if (phStr && (inputEl.tagName === 'INPUT' || inputEl.tagName === 'TEXTAREA')) {
              inputEl.placeholder = phStr;
            }
          }
          field.appendChild(inputEl);
          // rc57.12: render per-field help text under the input so
          // the toolbar matches the launchpad / dashboard_launchpad
          // UX. Without this the MC Windows Tools menu shows a
          // generic module card while the Terminal shows rich
          // per-field help (Lesson 1131 — parity gap).
          // Description capped at 240 chars, HTML-escaped (defense
          // against schema injection), rendered as <p>.
          if (prop.description !== undefined && prop.description !== null) {
            var descStr = String(prop.description).trim().slice(0, 240);
            if (descStr) {
              var helpEl = document.createElement('p');
              helpEl.className = 'mc-oc-modal-field-help';
              helpEl.innerHTML = escapeHtml(descStr);
              field.appendChild(helpEl);
            }
          }
          form.appendChild(field);
          renderedFields = true;
        });
      } catch (e) { renderedFields = false; }
    }
    if (!renderedFields) {
      // No form_schema: fall back to JSON textarea.
      var label = document.createElement('div');
      label.style.fontSize = '11px';
      label.style.color = 'rgba(255,255,255,0.55)';
      label.style.textTransform = 'uppercase';
      label.style.letterSpacing = '0.08em';
      label.style.margin = '12px 0 4px';
      label.textContent = 'Inputs (JSON)';
      form.appendChild(label);
      var ta = document.createElement('textarea');
      ta.name = '__json';
      ta.style.width = '100%';
      ta.style.minHeight = '100px';
      ta.style.padding = '8px';
      ta.style.background = 'rgba(0,0,0,0.4)';
      ta.style.color = '#f5f5f7';
      ta.style.border = '1px solid rgba(255,255,255,0.20)';
      ta.style.borderRadius = '4px';
      ta.style.font = '12px/1.4 ui-monospace, "SF Mono", Menlo, Consolas, monospace';
      ta.style.resize = 'vertical';
      ta.placeholder = '{}';
      form.appendChild(ta);
    }

    var result = document.createElement('div');
    result.className = 'mc-oc-modal-result';
    result.hidden = true;
    form.appendChild(result);

    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'mc-oc-modal-button';
    cancel.textContent = 'Cancel';
    cancel.addEventListener('click', function () { document.body.removeChild(backdrop); });
    var run = document.createElement('button');
    run.type = 'submit';
    run.className = 'mc-oc-modal-button is-primary';
    run.textContent = 'Run';
    actions.appendChild(cancel);
    actions.appendChild(run);
    form.appendChild(actions);
    panel.appendChild(form);
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);

    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      run.disabled = true;
      run.textContent = 'Running...';
      // Collect inputs from the rendered fields OR from the JSON textarea.
      var inputs = {};
      if (renderedFields) {
        var allFields = form.querySelectorAll('[data-field-type]');
        for (var k = 0; k < allFields.length; k++) {
          var f = allFields[k];
          var name = f.name;
          var t = f.dataset.fieldType;
          if (t === 'number' || t === 'integer') {
            var n = f.value === '' ? undefined : Number(f.value);
            if (n !== undefined && !Number.isNaN(n)) inputs[name] = n;
          } else if (t === 'boolean') {
            inputs[name] = !!f.checked;
          } else {
            inputs[name] = f.value;
          }
        }
      } else {
        var jsonText = (form.querySelector('textarea[name="__json"]').value || '').trim();
        if (jsonText) {
          try { inputs = JSON.parse(jsonText); }
          catch (e) {
            showToast('Invalid JSON: ' + e.message, 'error');
            run.disabled = false;
            run.textContent = 'Run';
            return;
          }
        }
      }
      result.hidden = false;
      result.textContent = 'Running...';
      try {
        var resp = await tauriInvoke('mc_run_module', {
          moduleId: m.id,
          // Lesson 985: MAIC tools have `name` not `id`. The chat
          // toolbar previously sent `tool: tool.id` (undefined),
          // which Rust rejected with "tool is required". The
          // launchpad (src/pages/dashboard_launchpad.js:548) and
          // modules-menu openModuleModal both correctly use
          // `tool: firstTool.name`. Match that convention.
          tool: toolName,
          inputs: inputs,
          surface: 'openclaw_chat',
        });
        // Lesson 1138 (2026-10-08, David): MAIC returns
        // `{outputs: {client_side: true, message: "..."}}` for
        // modules that run locally (firecrawl, voice-memo,
        // calendar-companion, etc. — anything with a sidecar on
        // disk). The previous code just JSON.stringified the stub
        // and David read the "This module runs locally..." string as
        // the tool erroring out. Now we detect client_side and show
        // a clear "Open in launchpad" CTA in the action row.
        if (
          resp &&
          resp.outputs &&
          typeof resp.outputs === 'object' &&
          resp.outputs.client_side === true
        ) {
          var csm =
            (typeof resp.outputs.message === 'string' && resp.outputs.message) ||
            'This module runs locally on this machine. Open the launchpad and use the Your Tools tile to invoke it.';
          result.classList.add('is-client-side');
          result.innerHTML = '';
          var csHeading = document.createElement('div');
          csHeading.className = 'mc-oc-modal-result-client-side-heading';
          csHeading.textContent = '🖥  Runs locally on this machine';
          var csMsg = document.createElement('p');
          csMsg.className = 'mc-oc-modal-result-client-side-msg';
          csMsg.textContent = csm;
          result.appendChild(csHeading);
          result.appendChild(csMsg);
          // Skip the Send-to-Chat / Copy actions (no real output to
          // send) and replace with an Open-in-launchpad button.
          var prevActions = form.parentNode.querySelector('.mc-oc-modal-result-actions');
          if (prevActions) prevActions.remove();
          var csActions = document.createElement('div');
          csActions.className = 'mc-oc-modal-result-actions';
          var openLaunch = document.createElement('button');
          openLaunch.type = 'button';
          openLaunch.className = 'mc-oc-modal-button is-primary';
          openLaunch.textContent = '↗ Open in launchpad';
          openLaunch.addEventListener('click', function () {
            try {
              document.body.removeChild(backdrop);
              window.location.hash = '#/dashboard-launchpad';
            } catch (e) {
              showToast('Could not open launchpad: ' + (e && e.message ? e.message : e), 'error');
            }
          });
          csActions.appendChild(openLaunch);
          var csClose = document.createElement('button');
          csClose.type = 'button';
          csClose.className = 'mc-oc-modal-button';
          csClose.textContent = 'Close';
          csClose.addEventListener('click', function () { document.body.removeChild(backdrop); });
          csActions.appendChild(csClose);
          // Lesson 1144 (2026-11-02, David): this used to do
          //   form.parentNode.insertBefore(csActions, result.nextSibling)
          // but `result` is a child of `form`, so `result.nextSibling`
          // lives in a DIFFERENT parent than `form.parentNode`. The
          // browser threw "The child can not be found in the parent"
          // and the whole client_side branch died with a generic
          // "Run failed" -- so the user NEVER saw the "Runs locally"
          // message for firecrawl / voice-memo, exactly the modules
          // that need it most. Insert next to `result` inside its own
          // parent instead.
          result.parentNode.insertBefore(csActions, result.nextSibling);
          run.textContent = 'Run again';
          run.disabled = false;
          run.addEventListener('click', function () { form.requestSubmit(); });
          showToast('Module runs locally', 'info');
          return;
        }
        var outText = '';
        if (resp && typeof resp.outputs !== 'undefined') {
          outText = typeof resp.outputs === 'string' ? resp.outputs : JSON.stringify(resp.outputs, null, 2);
        } else if (resp && typeof resp.output !== 'undefined') {
          outText = typeof resp.output === 'string' ? resp.output : JSON.stringify(resp.output, null, 2);
        } else if (resp && typeof resp.result !== 'undefined') {
          outText = typeof resp.result === 'string' ? resp.result : JSON.stringify(resp.result, null, 2);
        } else {
          outText = JSON.stringify(resp, null, 2);
        }
        result.textContent = outText || '(no output)';
        result.dataset.rawOutput = outText;

        // Action row: Send-to-Chat / Copy / Close.
        var resultActions = document.createElement('div');
        resultActions.className = 'mc-oc-modal-result-actions';
        var sendChat = document.createElement('button');
        sendChat.type = 'button';
        sendChat.className = 'mc-oc-modal-button is-primary';
        sendChat.textContent = '? Send to Chat';
        sendChat.addEventListener('click', async function () {
          var ok = await pasteIntoChatInput(outText);
          if (ok) showToast('Result sent to chat input', 'success');
        });
        var copy = document.createElement('button');
        copy.type = 'button';
        copy.className = 'mc-oc-modal-button';
        copy.textContent = '[clipboard] Copy';
        copy.addEventListener('click', async function () {
          try {
            if (navigator.clipboard && navigator.clipboard.writeText) {
              await navigator.clipboard.writeText(outText);
              showToast('Copied to clipboard', 'success');
            } else { showToast('Clipboard not available', 'error'); }
          } catch (e) { showToast('Copy failed: ' + e.message, 'error'); }
        });
        var close = document.createElement('button');
        close.type = 'button';
        close.className = 'mc-oc-modal-button';
        close.textContent = '? Close';
        close.addEventListener('click', function () { document.body.removeChild(backdrop); });
        resultActions.appendChild(sendChat);
        resultActions.appendChild(copy);
        resultActions.appendChild(close);
        result.parentNode.insertBefore(resultActions, result.nextSibling);

        run.textContent = 'Run again';
        run.disabled = false;
        // After the first run, clicking Run should submit again --
        // the form is already wired, so simply toggle the actions
        // row off via a new hidden input flag.
        run.addEventListener('click', function () { form.requestSubmit(); });
        showToast('Module ran', 'success');
      } catch (e) {
        result.classList.add('is-error');
        // Lesson 1144 (2026-11-02, David): surface a human-readable
        // reason, not the raw ureq string. The raw detail is kept in a
        // collapsible <details> so power users can still see the status
        // code and server body.
        var friendly = humanizeRunError(e, m, toolName);
        result.textContent = '';
        var errHead = document.createElement('div');
        errHead.className = 'mc-oc-modal-result-error-headline';
        errHead.textContent = friendly.headline;
        result.appendChild(errHead);
        if (friendly.detail) {
          var errDet = document.createElement('details');
          errDet.className = 'mc-oc-modal-result-error-detail';
          var errSum = document.createElement('summary');
          errSum.textContent = 'Technical details';
          errDet.appendChild(errSum);
          var errBody = document.createElement('pre');
          errBody.className = 'mc-oc-modal-result-error-body';
          errBody.textContent = friendly.detail;
          errDet.appendChild(errBody);
          result.appendChild(errDet);
        }
        run.disabled = false;
        run.textContent = 'Run';
        showToast(friendly.headline, 'error');
      }
    });
  }

  // Try to find the openclaw chat input and paste the module result into it.
  // Falls back to clipboard if we can't find the input.
  async function pasteIntoChatInput(payload) {
    var text;
    if (typeof payload === 'string') {
      text = payload;
    } else {
      // Try to surface a human-readable summary.
      if (payload.output != null) text = typeof payload.output === 'string' ? payload.output : JSON.stringify(payload.output, null, 2);
      else if (payload.result != null) text = typeof payload.result === 'string' ? payload.result : JSON.stringify(payload.result, null, 2);
      else if (payload.outputs != null) text = typeof payload.outputs === 'string' ? payload.outputs : JSON.stringify(payload.outputs, null, 2);
      else text = JSON.stringify(payload, null, 2);
    }
    if (!text) return false;
    // Try common openclaw chat input selectors. The OpenClaw chat
    // composer doesn't expose a `placeholder` attr matching the
    // legacy selectors (chat.composer.placeholder is a translated
    // i18n string with no fixed English substring). Use a broader
    // chain: textarea -> [contenteditable] -> contenteditable
    // descendants -> input[type='text'] (rare).
    var selectors = [
      'textarea',
      'textarea[placeholder*="essage" i]',
      'textarea[placeholder*="chat" i]',
      'textarea[placeholder*="type" i]',
      '[contenteditable="true"]',
      '[contenteditable=""]',
      'div[contenteditable]',
      'input[type="text"]',
    ];
    var el = null;
    for (var i = 0; i < selectors.length; i++) {
      var candidates = document.querySelectorAll(selectors[i]);
      for (var j = 0; j < candidates.length; j++) {
        var c = candidates[j];
        if (c.offsetParent !== null) { el = c; break; }
      }
      if (el) break;
    }
    if (el && 'value' in el) {
      var existing = el.value || '';
      el.value = existing + (existing ? '\n\n' : '') + text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.focus();
      return true;
    }
    if (el && el.isContentEditable) {
      el.innerText = (el.innerText || '') + '\n\n' + text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.focus();
      return true;
    }
    // Fallback: clipboard
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(text);
        showToast('Copied to clipboard -- paste into chat (Ctrl+V)', 'info');
        return true;
      } else {
        showToast('Could not locate chat input; result in console', 'error');
        console.log('[mc-toolbar] module result:', text);
        return false;
      }
    } catch (e) {
      showToast('Could not locate chat input; result in console', 'error');
      console.log('[mc-toolbar] module result:', text);
    }
  }

  // ---------------------------------------------------------------------------
  // Voice: invoke mc_voice_transcribe via mc_module_call dispatcher
  // (Lesson 1001, rc57.1) -- uses whisper.cpp sidecar instead of
  // broken SAPI 5 native capture.
  // ---------------------------------------------------------------------------

  async function callVoiceModule(command, params) {
    return tauriInvoke('mc_module_call', { command: command, params: params || {} });
  }

  // Auto-download whisper model on first use (~75MB, once).
  var _voiceReadyCache = null;
  async function ensureVoiceReady() {
    if (_voiceReadyCache === true) return true;
    if (_voiceReadyCache === false) return false;
    try {
      var status = await callVoiceModule('mc_voice_check', {});
      if (status && status.model_loaded) {
        _voiceReadyCache = true;
        return true;
      }
      showToast('Downloading whisper model (~75 MB, first time)...', 'info', 30000);
      await callVoiceModule('mc_voice_download_model', {});
      var status2 = await callVoiceModule('mc_voice_check', {});
      if (status2 && status2.model_loaded) {
        _voiceReadyCache = true;
        return true;
      }
    } catch (e) {
      _voiceReadyCache = false;
      throw e;
    }
    _voiceReadyCache = false;
    return false;
  }

  async function onVoiceClick(btn) {
    if (btn.getAttribute('data-state') === 'listening') return;
    setVoiceState(btn, 'listening');
    try {
      var ready = await ensureVoiceReady();
      if (!ready) {
        setVoiceState(btn, 'idle');
        showToast('Voice module not ready (download may have failed)', 'error');
        return;
      }
      var result = await callVoiceModule('mc_voice_transcribe', {
        seconds: 15,
        vad_enabled: true,
        silence_ms: 1500,
      });
      setVoiceState(btn, 'idle');
      var text = (result && typeof result.text === 'string') ? result.text.trim() : '';
      if (text) {
        await pasteIntoChatInput(String(text));
        var durMs = (result && typeof result.duration_ms === 'number') ? result.duration_ms : 0;
        var txMs = (result && typeof result.transcribe_ms === 'number') ? result.transcribe_ms : 0;
        showToast('Voice captured in ' + (durMs + txMs) + ' ms', 'success');
      } else {
        showToast('No speech detected', 'info');
      }
    } catch (e) {
      setVoiceState(btn, 'idle');
      _voiceReadyCache = null;
      var msg = (e && e.message) ? e.message : String(e);
      if (/NotInstalled|sidecar not found|module 'voice'/i.test(msg)) {
        showToast('Voice module not installed. Open Settings -> Modules to install it.', 'error', 6000);
      } else {
        showToast('Voice capture failed: ' + msg, 'error', 5000);
      }
    }
  }

  function setVoiceState(btn, state) {
    btn.setAttribute('data-state', state);
  }

  // ---------------------------------------------------------------------------
  // Attach: drop files, copy paths to clipboard (same UX as terminal toolbar)
  // ---------------------------------------------------------------------------

  var _attached = [];

  function openAttachModal() {
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = '📎 Attach files';
    panel.appendChild(h);
    var sub = document.createElement('p');
    sub.className = 'mc-oc-modal-subtitle';
    sub.textContent = 'Drop files below. We will copy the file paths to your clipboard so you can paste them into the chat (Ctrl+V).';
    panel.appendChild(sub);

    var dz = document.createElement('div');
    dz.className = 'mc-oc-attach-dropzone';
    dz.textContent = 'Drop files here, or click to choose';
    dz.setAttribute('role', 'button');
    dz.setAttribute('tabindex', '0');
    var fileInput = document.createElement('input');
    fileInput.type = 'file';
    fileInput.multiple = true;
    fileInput.style.display = 'none';
    dz.addEventListener('click', function () { fileInput.click(); });
    dz.addEventListener('keydown', function (ev) {
      if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); fileInput.click(); }
    });
    fileInput.addEventListener('change', function () {
      addAttached(Array.from(fileInput.files || []));
      fileInput.value = '';
    });
    ['dragenter', 'dragover'].forEach(function (evName) {
      dz.addEventListener(evName, function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        dz.classList.add('is-dragover');
      });
    });
    ['dragleave', 'drop'].forEach(function (evName) {
      dz.addEventListener(evName, function (ev) {
        ev.preventDefault();
        ev.stopPropagation();
        if (evName === 'dragleave' && ev.target !== dz) return;
        dz.classList.remove('is-dragover');
      });
    });
    dz.addEventListener('drop', function (ev) {
      var dt = ev.dataTransfer;
      if (!dt) return;
      addAttached(Array.from(dt.files || []));
    });
    panel.appendChild(dz);
    panel.appendChild(fileInput);

    var list = document.createElement('div');
    list.className = 'mc-oc-attach-list';
    panel.appendChild(list);

    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () {
      document.body.removeChild(backdrop);
      _attached = [];
    });
    var copy = document.createElement('button');
    copy.type = 'button';
    copy.className = 'mc-oc-modal-button is-primary';
    copy.textContent = 'Copy paths to clipboard';
    copy.disabled = true;
    copy.addEventListener('click', async function () {
      var paths = _attached.map(function (f) { return f.path || f.name; });
      var joined = paths.join('\n');
      try {
        await navigator.clipboard.writeText(joined);
        showToast('Copied ' + paths.length + ' path(s) -- paste into chat (Ctrl+V)', 'success');
        document.body.removeChild(backdrop);
        _attached = [];
      } catch (e) {
        showToast('Clipboard write failed: ' + e.message, 'error');
      }
    });
    actions.appendChild(close);
    actions.appendChild(copy);
    panel.appendChild(actions);

    function addAttached(files) {
      files.forEach(function (f) {
        // In a Tauri webview, File.path is set (Tauri patches the File
        // ctor to include the absolute path). If it's not, we just
        // record the name.
        var entry = { name: f.name, path: f.path || f.name, size: f.size };
        _attached.push(entry);
        var item = document.createElement('div');
        item.className = 'mc-oc-attach-list-item';
        var nameEl = document.createElement('span');
        nameEl.className = 'mc-oc-attach-list-item-name';
        nameEl.textContent = entry.path;
        var sz = document.createElement('span');
        sz.style.color = 'rgba(255,255,255,0.55)';
        sz.style.fontSize = '11px';
        sz.textContent = humanSize(entry.size);
        item.appendChild(nameEl);
        item.appendChild(sz);
        list.appendChild(item);
      });
      copy.disabled = _attached.length === 0;
    }
    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) {
        document.body.removeChild(backdrop);
        _attached = [];
      }
    });
    document.body.appendChild(backdrop);
  }

  function humanSize(n) {
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return Math.round(n / 1024) + ' KB';
    if (n < 1024 * 1024 * 1024) return Math.round(n / 1024 / 1024) + ' MB';
    return (n / 1024 / 1024 / 1024).toFixed(1) + ' GB';
  }

  // ---------------------------------------------------------------------------
  // Secrets: list tool names (no values). Uses mc_list_tools.
  // ---------------------------------------------------------------------------

  async function openSecretsModal() {
    var backdrop = document.createElement('div');
    backdrop.className = 'mc-oc-modal-backdrop';
    var panel = document.createElement('div');
    panel.className = 'mc-oc-modal-panel';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');
    var h = document.createElement('h2');
    h.className = 'mc-oc-modal-title';
    h.textContent = '🔑 Secrets & local tools';
    panel.appendChild(h);
    var sub = document.createElement('p');
    sub.className = 'mc-oc-modal-subtitle';
    sub.textContent = 'Names of the local tools and secrets available to MAIC. Values are never shown in the chat UI.';
    panel.appendChild(sub);

    var section = document.createElement('div');
    section.className = 'mc-oc-modal-section';
    var sectionH = document.createElement('h3');
    sectionH.textContent = 'Local tools';
    section.appendChild(sectionH);
    var loading = document.createElement('div');
    loading.style.color = 'rgba(255,255,255,0.55)';
    loading.style.fontStyle = 'italic';
    loading.textContent = 'Loading...';
    section.appendChild(loading);
    panel.appendChild(section);

    var actions = document.createElement('div');
    actions.className = 'mc-oc-modal-actions';
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'mc-oc-modal-button';
    close.textContent = 'Close';
    close.addEventListener('click', function () { document.body.removeChild(backdrop); });
    actions.appendChild(close);
    panel.appendChild(actions);

    backdrop.appendChild(panel);
    backdrop.addEventListener('click', function (ev) {
      if (ev.target === backdrop) document.body.removeChild(backdrop);
    });
    document.body.appendChild(backdrop);

    try {
      var resp = await tauriInvoke('mc_list_tools');
      var tools = Array.isArray(resp) ? resp : [];
      section.removeChild(loading);
      if (tools.length === 0) {
        var empty = document.createElement('div');
        empty.style.color = 'rgba(255,255,255,0.55)';
        empty.style.fontStyle = 'italic';
        empty.textContent = 'No local tools enabled.';
        section.appendChild(empty);
        return;
      }
      tools.forEach(function (t) {
        var row = document.createElement('div');
        row.className = 'mc-oc-modal-row';
        var name = document.createElement('span');
        name.className = 'mc-oc-modal-row-name';
        name.textContent = String(t);
        row.appendChild(name);
        section.appendChild(row);
      });
    } catch (e) {
      section.removeChild(loading);
      var err = document.createElement('div');
      err.style.color = '#ff8080';
      err.textContent = 'Failed to list tools: ' + (e && e.message ? e.message : e);
      section.appendChild(err);
    }
  }

  // ---------------------------------------------------------------------------
  // Mount
  // ---------------------------------------------------------------------------

  function loadCSS() {
    if (document.getElementById('mc-oc-toolbar-css')) return;
    var link = document.createElement('link');
    link.id = 'mc-oc-toolbar-css';
    link.rel = 'stylesheet';
    link.href = './mc-toolbar.css';
    document.head.appendChild(link);
  }

  function mount() {
    if (!shouldMount()) return;
    if (!document.body) {
      document.addEventListener('DOMContentLoaded', mount, { once: true });
      return;
    }
    loadCSS();

    var bar = document.createElement('div');
    bar.id = 'mc-oc-toolbar';
    bar.className = 'mc-oc-toolbar';
    bar.setAttribute('role', 'toolbar');
    bar.setAttribute('aria-label', 'MiracleClaw chat tools');

    var attachBtn = makeButton('mc-oc-attach-btn', '📎', 'Attach files to next chat message', 'Attach files');
    attachBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      openAttachModal();
    });
    bar.appendChild(attachBtn);

    var secretsBtn = makeButton('mc-oc-secrets-btn', '🔑', 'Add usernames, passwords, API keys to the MC secrets vault', 'Secrets');
    secretsBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      // Lesson 980: open the actual MC secrets card (same overlay the
      // Terminal page uses) instead of a tools-list modal. The MC main
      // window navigates to #mcAutoOpen=secrets which the terminal
      // page picks up and auto-opens the secrets overlay on. When the
      // user closes the overlay, mc_close_overlay navigates them back
      // to the chat URL (stashed by mc_open_overlay).
      // Lesson 985 (2026-09-16 07:15 MDT, David): Tauri 2's IPC layer
      // converts JS camelCase arg keys to snake_case Rust param names
      // (same convention as __TAURI__.core.invoke). The Secrets
      // button previously sent { overlay_key: 'secrets' } (snake_case),
      // which the IPC layer rejected with:
      //   "Failed to open Secrets: invalid args 'overlayKey' for
      //    command 'mc_open_overlay': command mc_open_overlay
      //    missing required key overlayKey"
      // -- the conversion landed on a key Tauri could not match to
      // the Rust signature `fn mc_open_overlay(overlay_key: String,
      // ...)`. The fix: send camelCase `overlayKey`, matching the
      // rest of the toolbar (mc_run_module uses moduleId/tool/
      // inputs/surface, mc_voice_native_capture uses timeoutMs).
      tauriInvoke('mc_open_overlay', { overlayKey: 'secrets' })
        .then(function () { /* main window will navigate */ })
        .catch(function (e) {
          console.error('[mc-toolbar] mc_open_overlay(secrets) failed:', e);
          alert('Failed to open Secrets: ' + (e && e.message ? e.message : e));
        });
    });
    bar.appendChild(secretsBtn);

    var voiceBtn = makeButton('mc-oc-voice-btn', '🎙', 'Voice input (Windows SAPI)', 'Voice input');
    voiceBtn.classList.add('mc-oc-voice-btn');
    voiceBtn.setAttribute('data-state', 'idle');
    voiceBtn.removeAttribute('aria-haspopup');
    voiceBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      onVoiceClick(voiceBtn);
    });
    bar.appendChild(voiceBtn);

    // rc57.25: 📅 Calendar — direct action, same family as Attach/Secrets/
    // Voice (one click = do the thing). Unlike the Modules button, which
    // stays a list of less-common modules. Targets the real `calendar`
    // module (NOT `calendar-companion`, which is private-beta and 501s).
    var calBtn = makeButton('mc-oc-calendar-btn', '📅', 'Open calendar', 'Open calendar');
    calBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      closeDropdown();
      if (window.McCalendar && typeof window.McCalendar.open === 'function') {
        window.McCalendar.open({ invoke: tauriInvoke });
      } else {
        showToast('Calendar is still loading — try again in a moment.', 'error');
      }
    });
    bar.appendChild(calBtn);

    var modulesBtn = makeButton('mc-oc-modules-btn', '🧩', 'Installed modules', 'Open modules menu');
    modulesBtn.addEventListener('click', function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      openModulesMenu();
    });
    var modulesCount = document.createElement('span');
    modulesCount.id = 'mc-oc-modules-count';
    modulesCount.className = 'mc-oc-icon-link-count';
    modulesCount.setAttribute('hidden', '');
    modulesCount.textContent = '0';
    modulesBtn.appendChild(modulesCount);
    bar.appendChild(modulesBtn);

    document.body.appendChild(bar);

    // Sibling dropdown (Lesson 968) for the modules menu.
    var modulesMenu = document.createElement('div');
    modulesMenu.id = 'mc-oc-modules-menu';
    modulesMenu.className = 'mc-oc-dropdown';
    modulesMenu.setAttribute('hidden', '');
    modulesMenu.setAttribute('data-trigger', 'mc-oc-modules-btn');
    modulesMenu.setAttribute('role', 'menu');
    var modulesHeader = document.createElement('div');
    modulesHeader.className = 'mc-oc-dropdown-header';
    modulesHeader.textContent = 'Installed modules';
    modulesMenu.appendChild(modulesHeader);
    var modulesBody = document.createElement('div');
    modulesBody.className = 'mc-oc-dropdown-body';
    modulesMenu.appendChild(modulesBody);
    document.body.appendChild(modulesMenu);

    // Close on outside click + scroll/resize (Lesson 976).
    document.addEventListener('click', function (ev) {
      if (!openDropdown) return;
      if (openDropdown.contains(ev.target)) return;
      var triggerId = openDropdown.getAttribute('data-trigger');
      if (triggerId && document.getElementById(triggerId) && document.getElementById(triggerId).contains(ev.target)) return;
      closeDropdown();
    });
    window.addEventListener('scroll', closeDropdown, true);
    window.addEventListener('resize', closeDropdown);
    document.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape' && openDropdown) closeDropdown();
    });

    // Initial count + retry after a delay (in case the openclaw app
    // finishes mounting after us).
    refreshModulesCount();
    setTimeout(refreshModulesCount, 2000);
    setTimeout(refreshModulesCount, 6000);
  }

  // Defer to next tick so we run after openclaw's own scripts.
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', mount, { once: true });
  } else {
    setTimeout(mount, 0);
  }
})();
