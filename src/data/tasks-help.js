// src/data/tasks-help.js — Tasks page help content (rc55.18, 2026-08-30)
//
// Canonical help content for the Tasks page (src/pages/tasks.js).
// The Tasks page header has a ❔ Help button that opens a full-screen
// overlay with this data. Modeled after src/data/module-help.js
// (Add-On Module help), but Tasks is a CORE page (not an Add-On
// Module), so it has its own data file + renderer pair:
//
//   src/pages/tasks-help.js        — overlay UI + openTasksHelp()
//   src/data/tasks-help.js         — this file (content)
//   src/pages/tasks.js             — page that mounts the ❔ button
//
// David 2026-08-30 18:08 MDT: "create a help file in Tasks and tutorial
// for users that might want help using that page." Same audience as
// Module Help (David's 2026-08-26 framing: "newer type user that has
// never used commands, veteran cli users should have no trouble").
//
// Schema (sections are independent — only render the ones present):
//   whoFor           — one-line audience tag
//   whyUseIt         — one-line value prop in plain English
//   whatItDoes       — one-liner shown as the overlay subtitle
//   windowsUI        — list of {surface, steps} for each UI surface
//                      (Dashboard / Terminal / Files / Settings /
//                      Chat). Each surface = a numbered flow.
//   examples         — list of natural-language prompts for MAIC chat.
//                      Most important section for newer users — shows
//                      what the AI can actually do.
//   chat             — list of {cmd, desc} for MC task slash commands
//                      usable in MAIC chat
//   troubleshooting  — list of {problem, fix} for common gotchas
//
// All content is plain text — no markdown, no HTML. The renderer
// (tasks-help.js) escapes it for safety. Keep examples concrete (real
// commands, real expected outputs) so the user can copy-paste-test.

export const TASKS_HELP = {
  id: "tasks",
  name: "Tasks",
  icon: "✅",
  version: "1.1.0",
  publisher: "Miracle Claw",

  whoFor:
    "Anyone juggling follow-ups, recurring to-dos, or reminders between chat sessions — especially if you want MAIC (the AI assistant) to remember tasks you talked about earlier.",

  whyUseIt:
    "Tasks gives you a persistent to-do list that lives across chat sessions. You can add tasks by typing, by asking MAIC, or by date — and your tasks stay synced between your computer and the MAIC cloud so the AI knows what's still on your plate.",

  whatItDoes:
    "A simple, date-organized task list inside Miracle Claw. Add, edit, check off, and delete tasks; sync them with MAIC's cloud so the assistant in chat can reference them across sessions.",

  windowsUI: [
    {
      surface: "Dashboard → Tasks page",
      steps: [
        "From the Dashboard, click the ✅ Tasks tile (or the Tasks icon in the sidebar if you've pinned it).",
        "You'll land on the Tasks page. Free-tier users see an upgrade message; paid-tier users see the full list.",
        "Use the form at the top to add a task: type a name, optionally add details, pick a priority (Low/Medium/High), and choose a date.",
        "Click + Add. The task appears in the list, grouped under its date.",
        "Check the box on the left of any task to mark it done. Click ✎ to edit. Click 🗑 to delete (asks for confirmation).",
      ],
    },
    {
      surface: "Header buttons",
      steps: [
        "🔄 Sync now — pushes your local task changes to MAIC's cloud and pulls any changes MAIC made. You'll get a summary of what changed.",
        "↻ Refresh — reloads the task list from your computer's local storage (no network).",
        "🔑 Set token — (optional) paste a long-lived MAIC API token so sync doesn't depend on your session. Most users can skip this.",
        "← Dashboard — go back to the main page.",
      ],
    },
    {
      surface: "Date groupings",
      steps: [
        "Tasks are grouped by date in three buckets: Today, Upcoming, and Past (in chronological order).",
        "Each group has a header like \"Today · 2026-08-30\" or \"Wednesday · 2026-09-02\".",
        "Completed tasks stay in their date group but show as crossed-out so you can see what you finished on a given day.",
      ],
    },
    {
      surface: "Syncing with MAIC",
      steps: [
        "When the page first loads, it silently pulls any task changes MAIC made in chat (a small ⟳ indicator appears briefly in the header).",
        "Click 🔄 Sync now any time you want to push/pull manually. You'll see a confirmation dialog first.",
        "If MAIC is unreachable (network down, server maintenance), the page shows ⚠ Stale in the header — your local tasks still work.",
        "Conflicts resolve by most recent update; if local and remote have the same timestamp, the server wins.",
      ],
    },
  ],

  examples: [
    "Ask MAIC in chat: \"Add a task to call the customer about her Civic brakes on Friday.\" → MAIC adds it for you and you'll see it appear on the Tasks page next time you sync.",
    "Ask MAIC: \"What tasks do I have today?\" → MAIC reads from your list and summarizes what's on your plate.",
    "Ask MAIC: \"Mark the brake-call task as done.\" → MAIC checks it off.",
    "Ask MAIC: \"Move the oil-change task to next Monday.\" → MAIC updates the date.",
    "Ask MAIC: \"Delete the tire-rotation task, I already did it.\" → MAIC removes it from your list.",
    "Right after finishing a customer conversation in chat: \"Save a follow-up for next week to check if the repair held up.\" → MAIC adds it with the right date.",
  ],

  chat: [
    {
      cmd: "Add a task to [name] on [date]",
      desc: "MAIC adds the task. Priority defaults to Medium unless you say otherwise (\"high priority…\" or \"low priority…\").",
    },
    {
      cmd: "What tasks do I have today?",
      desc: "Lists everything dated today (open and completed). Useful at the start of the day or before a customer call.",
    },
    {
      cmd: "What's on my plate this week?",
      desc: "Lists everything dated within the next 7 days, grouped by day.",
    },
    {
      cmd: "Mark [task name] as done",
      desc: "Sets the task to completed. MAIC may ask which task you meant if the name is ambiguous.",
    },
    {
      cmd: "Move [task name] to [new date]",
      desc: "Updates the date. Useful for rescheduling follow-ups.",
    },
    {
      cmd: "Delete [task name]",
      desc: "Removes the task from your list. Use sparingly — MAIC may ask for confirmation if it sounds like an important task.",
    },
    {
      cmd: "List my overdue tasks",
      desc: "Shows everything dated before today that isn't completed yet.",
    },
  ],

  troubleshooting: [
    {
      problem: "I see \"Tasks is a Pro feature\" and an upgrade button.",
      fix: "Tasks requires a paid MAIC tier (Starter, Pro, Team, or Enterprise). Click \"See plans →\" to view pricing. Free users can read this help but can't save or sync tasks.",
    },
    {
      problem: "The header says \"⚠ Stale\" after I tried to sync.",
      fix: "MAIC's cloud is unreachable from your network — check your internet connection, or try again in a minute. Your local tasks still work; sync just won't round-trip until MAIC is back. If you're behind a firewall, MAIC uses https://maicserver.com on port 443.",
    },
    {
      problem: "I added a task but it's not showing up after sync.",
      fix: "Click 🔄 Sync now to push your local changes. If you added it in chat (via MAIC), wait for the silent sync on page mount, or click Sync now. If it still doesn't appear, check the bottom of the list — past-date tasks live below upcoming ones.",
    },
    {
      problem: "MAIC said it added a task, but I can't find it on the Tasks page.",
      fix: "Click 🔄 Sync now. The Tasks page only auto-syncs once when you open it. If the task was added in a chat session BEFORE you opened the page, the sync on mount should have pulled it. If not, manual sync will. If still missing, the task may have been added with a date you didn't expect — try \"list my tasks\" in chat to see all of them.",
    },
    {
      problem: "Two devices show different task lists.",
      fix: "Click 🔄 Sync now on the device with the OLDER list first — local changes get pushed up. Then click Sync now on the OTHER device to pull the merged result. Tasks use a last-write-wins rule per field, so concurrent edits on the same task may overwrite each other.",
    },
    {
      problem: "Deleting a task asks for confirmation. Can I disable that?",
      fix: "Not currently — the confirmation protects against accidental deletes (especially since sync pushes deletes to MAIC). If you frequently delete tasks in bulk, ask MAIC in chat to \"delete all tasks dated last week\" and it can batch the work.",
    },
    {
      problem: "Tasks don't appear in MAIC chat even after sync.",
      fix: "MAIC chat-side task tools are paid-tier-gated. If you're on a paid tier, sign out of MAIC chat (via Settings) and sign back in so the tool list refreshes. If you're on the Free tier, MAIC won't have the task tools — your tasks will still save locally, just without chat-side AI assistance.",
    },
  ],
};