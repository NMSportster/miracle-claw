// src/data/maic-module-help.js — Help content for the MAIC-hosted modules
// (rc57.35, 2026-11-02, Lesson 1144).
//
// Why this file exists:
//   src/data/module-help.js was written at rc53.28/29 for the OLD local
//   add-on catalog (voice, translate, pdf, ocr, youtube, email, leadgen,
//   crm, tts). MAIC's live module catalog has since been replaced by a
//   completely different set of products:
//
//     ad-performance     brand-asset-kit   calendar
//     calendar-companion data-analyzer     email-campaigns-pro
//     firecrawl          marketing-suite   voice-memo
//
// Seven of those nine had NO help entry at all, so their "Help" button
// was dead. This file holds the help content for the MAIC-hosted set.
//
// Shape matches MODULE_HELP in ./module-help.js exactly so the same
// renderer (src/pages/module-help.js) can draw both.
//
// All content is plain text — no markdown, no HTML. The renderer escapes
// it for safety. Keep examples concrete (real values the user can paste)
// so a newer user can test the tool immediately.
//
// Tool names + field names below are the REAL ones from the production
// `modules.client_manifest` JSONB (verified 2026-11-02), so the examples
// line up with what the Run form actually asks for.

export const MAIC_MODULE_HELP = {
  // ---------------------------------------------------------------------
  "ad-performance": {
    whoFor:
      "Anyone running paid ads who wants a quick read on what is working and what is burning money.",
    whyUseIt:
      "Give it a date range and it summarizes clicks, conversions, and ROI so you know where to move your next dollar.",
    whatItDoes:
      "Analyzes ad campaign metrics and flags underperformers. Google Ads sync is not wired up yet, so the run returns a filled-in analysis template you can paste your numbers into.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button in the top-right of the chat window.",
          "Click Ad Performance, then press Run.",
          "Fill in Date range (for example: last 30 days). Add a Campaign ID if you have one.",
          "Press Run again to see the result, then Send to Chat to drop it into your conversation.",
        ],
      },
    ],
    examples: [
      "Date range: last 30 days — summarize clicks, conversions, and ROI.",
      "Date range: last 7 days, Campaign ID: brake-special-q3 — which ads are underperforming?",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem:
          "The result looks like an empty template with no real numbers in it.",
        fix:
          "That is expected right now. Google Ads sync is still coming, so the module hands you a correctly-structured analysis table for you to paste your metrics into. The layout, the fields, and the underperformer logic are the real thing.",
      },
      {
        problem: "I get an error saying Date range is required.",
        fix:
          "Date range is the one required field. Enter something like 'last 30 days' or '2026-10-01 to 2026-10-31'. Campaign ID is optional.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  "brand-asset-kit": {
    whoFor:
      "Small business owners and marketers who need a bio that actually sounds like their brand.",
    whyUseIt:
      "Pick a platform, add a few keywords that match your voice, and get a bio written to fit that platform's style — punchy for Twitter, professional for LinkedIn.",
    whatItDoes:
      "Generates on-brand social bios tuned to the platform you choose.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Brand Asset Kit.",
          "Press Run.",
          "Choose a Platform from the drop-down (twitter, linkedin, instagram, or website).",
          "Optionally add Keywords — comma separated, for example: tire repair, San Francisco Rd, fast.",
          "Press Run, then Send to Chat.",
        ],
      },
    ],
    examples: [
      "Platform: twitter, Keywords: auto repair, Albuquerque, honest estimates.",
      "Platform: linkedin, Keywords: family shop, second generation, since 1994.",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem: "The bio is too generic.",
        fix:
          "Add specific keywords — your actual services, your town, what makes you different. The more concrete the keywords, the more the bio sounds like you instead of a template.",
      },
      {
        problem: "I get an error saying Platform is required.",
        fix:
          "Platform is the one required field. Pick one from the drop-down. Keywords are optional but make a big difference.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  calendar: {
    whoFor:
      "Anyone with a packed schedule — booking jobs, running back-to-back appointments, or coordinating across devices.",
    whyUseIt:
      "Your calendar lives in your Miracle Claw account, so it shows up on every device you sign in on. Recurring events, categories, reminders, free-slot finding, and .ics import are all built in.",
    whatItDoes:
      "Add, edit, move, and cancel events; mark them Busy, Tentative, or Free; search by any word in a title, note, or location; find open slots; import .ics files; and extract appointments out of pasted messages.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Miracle Claw Calendar.",
          "With 15 tools you'll get a tool picker — pick the one you want.",
          "Search Events asks for a word to look for and how many results to show.",
          "Add Event asks for a title, a start, and an end. Times are ISO 8601, for example 2026-11-03T14:00:00.",
          "Extract Appointment takes a pasted message and reads the date, time, location, and title back to you.",
          "Free Slot finds openings that don't clash with anything you've saved.",
        ],
      },
    ],
    examples: [
      "Search Events: q = 'dentist', limit = 10",
      "Add Event: title = 'Brake job — Smith', start_at = 2026-11-03T14:00:00, end_at = 2026-11-03T15:00:00, location = 'San Francisco Rd NE'",
      "Extract Appointment: text = 'Dentist Tuesday Dec 5 at 2pm, Dr Pham on Paseo del Norte'",
      "Free Slot: window_start = 2026-11-03T09:00:00, window_end = 2026-11-03T17:00:00, duration_minutes = 30",
      "Import ICS: paste the whole .ics text from BEGIN:VCALENDAR to END:VCALENDAR",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem:
          "Free Slot and Busy Range don't see my Google Calendar or Outlook events.",
        fix:
          "Honest limit: those two tools only know about events saved inside Miracle Claw. They don't read external calendars, so check Google or Outlook for conflicts first. Import is a one-time copy, not a live two-way sync.",
      },
      {
        problem: "Extract Appointment read the time wrong.",
        fix:
          "It parses the message you paste and uses your timezone to work out the time. If the message says 'tomorrow' or 'next Friday', set Reference Date (YYYY-MM-DD) so it resolves against the right day. It saves nothing until you confirm — read it back and fix anything it got wrong.",
      },
      {
        problem: "An event isn't blocking my free-slot search.",
        fix:
          "Check the Show As setting. Anything marked Free is skipped by the search, so a long lunch marked 'free' still leaves your afternoon open.",
      },
      {
        problem: "I deleted a category and now my events lost their color.",
        fix:
          "That's by design. Deleting a category removes the color label but leaves every event exactly where it was.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  "calendar-companion": {
    whoFor:
      "People who need a time that works for everyone and would rather not read six calendars themselves.",
    whyUseIt:
      "Tell it how long the meeting is and who's attending, and it suggests a time that works for everyone.",
    whatItDoes:
      "Smart scheduling with conflict detection. This one is in private beta — Google Calendar sync is on the way.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Calendar Companion.",
          "Press Run.",
          "Enter Duration in minutes (between 15 and 480).",
          "Optionally add Attendees as comma-separated email addresses.",
          "Press Run.",
        ],
      },
    ],
    examples: [
      "Duration: 30 minutes, Attendees: shop@example.com, david@example.com",
      "Duration: 60 minutes — find a slot for a brake-system review.",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem:
          "I get 'This module is in private beta. Join the waitlist at https://miracleclaw.com/waitlist/calendar-companion'.",
        fix:
          "That's the expected response for this one — Calendar Companion hasn't shipped its server-side runner yet. It's not a mistake on your end. Use the main Miracle Claw Calendar module's Free Slot tool in the meantime; it does the same job against the events saved in your account.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  "data-analyzer": {
    whoFor:
      "Anyone staring at a spreadsheet and wanting the story in it without writing the formulas.",
    whyUseIt:
      "Point it at a public CSV and it summarizes the data, pulls out the most interesting rows, and flags anomalies. Files up to 5 MB.",
    whatItDoes:
      "CSV and spreadsheet analysis with plain-English summaries.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Data Analyzer.",
          "Press Run.",
          "Paste a public CSV URL into CSV URL.",
          "Optionally fill in Focus — e.g. 'highest revenue months' or 'rows with missing values'.",
          "Press Run, then Send to Chat.",
        ],
      },
    ],
    examples: [
      "CSV URL: https://example.com/sales.csv, Focus: which months did we lose money?",
      "CSV URL: https://example.com/inventory.csv — flag anything that looks wrong.",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem: "I get an error saying CSV URL is required.",
        fix:
          "CSV URL is the one required field. It must be a public http:// or https:// address — a link to a file your browser can fetch. Local paths like C:\\data.csv and private-network addresses are blocked on purpose.",
      },
      {
        problem: "The summary missed my columns.",
        fix:
          "Use the Focus field to say what matters — 'revenue by month', 'duplicate records', 'rows over 10,000 miles'. The summarizer uses it to decide what's worth calling out.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  "email-campaigns-pro": {
    whoFor:
      "Shop owners and small-business marketers who send the same campaign every few weeks and want it drafted, not written from scratch.",
    whyUseIt:
      "Describe the goal in a sentence or two and you get a 3-email drip with A/B subject lines for each step.",
    whatItDoes:
      "Sequenced email drips with A/B subject lines.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Email Campaigns Pro.",
          "Press Run.",
          "Describe the goal — for example: 'promote a brake special'.",
          "Optionally fill in Audience — for example: 'customers who haven't visited in 6 months'.",
          "Press Run, then Send to Chat.",
        ],
      },
    ],
    examples: [
      "Goal: win back inactive customers, Audience: shop customers last seen 6+ months ago",
      "Goal: promote a brake special, Audience: local drivers on San Francisco Rd",
      "Goal: follow up after a service — remind customers about their 6-month checkup.",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem: "The emails sound generic.",
        fix:
          "Be specific in the Goal. 'Promote a brake special' gets a generic draft; 'promote our $99 brake pad special with free installation, mention we do same-day' gets a specific one. The Audience field helps even more.",
      },
      {
        problem: "I get an error saying Goal is required.",
        fix:
          "Goal is the one required field. One or two sentences is plenty — Audience is optional.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  firecrawl: {
    whoFor:
      "Anyone who needs the actual text of a web page instead of a wall of navigation and ads.",
    whyUseIt:
      "Paste a URL and it pulls clean text and metadata. Use Crawl Site to grab a whole site. Runs locally on your machine — nothing is sent to our servers.",
    whatItDoes:
      "Pulls clean content from any public web page, or crawls a site from a starting URL.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Firecrawl.",
          "Pick a tool: Fetch URL (one page) or Crawl Site (many pages).",
          "Fetch URL asks for a URL. Crawl Site asks for a Start URL and optionally Max pages (1–100, default 10).",
          "Press Run.",
          "This module runs locally, so the result tells you to use the Your Tools tile in the launchpad to actually invoke it.",
        ],
      },
    ],
    examples: [
      "Fetch URL: https://example.com/pricing",
      "Crawl Site: start_url = https://example.com, max_pages = 25",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem:
          "The result says 'Runs locally on this machine' instead of showing the page text.",
        fix:
          "That's expected — Firecrawl is one of the modules that runs on your machine rather than on our servers. Press 'Open in launchpad' and use the Your Tools tile there to invoke it. That path has the sidecar configured.",
      },
      {
        problem: "I get an error about an internal host or a private network address.",
        fix:
          "Firecrawl refuses to fetch localhost, .local, .internal, and private-network addresses on purpose — a module shouldn't be able to probe the server's own network. Use a public http:// or https:// address.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  "marketing-suite": {
    whoFor:
      "Anyone who has one brief in their head and needs it to become an email, a social post, and ad copy at once.",
    whyUseIt:
      "Write a one-paragraph brief and get matching emails, social posts, and ad copy. Pick a tone to set the voice.",
    whatItDoes:
      "Email campaigns, social posts, and ad copy from a single brief.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Marketing Suite.",
          "Pick a tool: Draft Email or Social Post.",
          "Draft Email asks for To audience and Key points, and optionally a Tone (friendly, professional, urgent, playful).",
          "Social Post asks for a Platform (twitter, linkedin, facebook, instagram) and a Topic.",
          "Press Run, then Send to Chat.",
        ],
      },
    ],
    examples: [
      "Draft Email: to_audience = 'shop customers', key_points = 'brake pad special, free install, same-day', tone = 'friendly'",
      "Social Post: platform = instagram, topic = 'your car's 5 telltale signs of brake trouble'",
      "Draft Email: to_audience = 'past service customers', key_points = '6-month checkup reminder', tone = 'professional'",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem: "The draft doesn't mention my specifics.",
        fix:
          "Key points is where the specifics live — list the actual offers, prices, and service names. To audience sets who it's talking to. Be concrete; the draft mirrors what you give it.",
      },
      {
        problem: "I get an error about to_audience or key_points.",
        fix:
          "Both are required for Draft Email. For Social Post, Platform and Topic are both required.",
      },
    ],
  },

  // ---------------------------------------------------------------------
  "voice-memo": {
    whoFor:
      "Anyone with their hands busy — on a lift, under a car, halfway through a part — who still needs to note something down.",
    whyUseIt:
      "Record a voice memo from the chat input mic icon and it transcribes it locally and pulls out the action items. Audio never leaves your device.",
    whatItDoes:
      "Transcribes audio memos and extracts action items, using the local whisper.cpp sidecar.",
    windowsUI: [
      {
        surface: "Chat toolbar → Modules",
        steps: [
          "Click the 🧩 Modules button, then click Voice Memo.",
          "Press Run.",
          "Paste a public URL to an audio file into Audio file URL (.m4a, .mp3, or .wav).",
          "Press Run.",
          "This module runs locally — press 'Open in launchpad' to reach the Your Tools tile that drives the whisper.cpp sidecar.",
        ],
      },
    ],
    examples: [
      "Audio file URL: https://example.com/voice-note.m4a",
      "Record straight from the chat input mic icon, then transcribe the recording.",
    ],
    terminal: [],
    chat: [],
    troubleshooting: [
      {
        problem:
          "The result says 'Runs locally on this machine' instead of a transcript.",
        fix:
          "Expected — Voice Memo transcribes through the local whisper.cpp sidecar, not a server. Press 'Open in launchpad' and use the Your Tools tile there. Audio stays on your machine either way.",
      },
      {
        problem: "My audio URL won't work.",
        fix:
          "It needs to be a public http:// or https:// address pointing directly at the audio file, not a share page. Common formats: .m4a, .mp3, .wav.",
      },
    ],
  },
};

export function getMaicModuleHelp(id) {
  return MAIC_MODULE_HELP[id] || null;
}
