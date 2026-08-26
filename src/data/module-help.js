// src/data/module-help.js — Add-On Module help content (rc53.28, 2026-08-26;
// enriched for non-CLI users in rc53.29, 2026-08-26, per David:
//
//   "have to do a good job of explaining these programs for the newer
//    type user that has never used commands, veteran cli users should
//    have no trouble"
//
// Canonical help content for every Add-On Module card. Each module's
// Help button (❔) opens an overlay that renders this data.
//
// Shape per module:
//   whoFor         — one-line audience tag, e.g. "Anyone who types
//                    faster than they talk". Optional but recommended.
//   whyUseIt       — one-line value prop in plain English. Optional.
//   whatItDoes     — one-liner shown as the overlay's subtitle.
//   windowsUI      — list of {surface, steps} for the Tauri desktop UI
//                    surfaces (Dashboard / Terminal / Files / Settings /
//                    Chat / Extras hub). Each surface = a clickable flow.
//   examples       — list of natural-language prompts a user can paste
//                    into MAIC chat. Most important section for newer
//                    users — shows what the AI can actually do.
//   terminal       — list of {cmd, desc} of mc-* slash commands or
//                    invokeModule calls usable in the Terminal page.
//   chat           — list of @module-slash commands usable in MAIC chat.
//                    Empty if the module has no chat-side surface.
//   troubleshooting — list of {problem, fix} for the most common gotchas.
//
// All content is plain text — no markdown, no HTML. The renderer escapes
// it for safety. Keep examples concrete (real commands, real expected
// outputs) so the user can copy-paste-test.
export const MODULE_HELP = {
  voice: {
    whoFor:
      "Anyone who talks faster than they type, or who can't easily use a keyboard (hands full, on the move).",
    whyUseIt:
      "Talk into your mic and MAIC turns your speech into text — no typing required. Everything runs locally on your machine, so no audio ever leaves your computer.",
    whatItDoes:
      "Push-to-talk microphone that transcribes your speech locally with whisper.cpp and drops the text into any chat surface. No audio ever leaves your machine.",
    windowsUI: [
      {
        surface: "Dashboard",
        steps: [
          "Click the 🎙 button (bottom-right of the dashboard).",
          "Speak normally — your words appear as text in a pop-up card.",
          "Edit the text if needed, then click \"Send to OpenClaw chat\" to send it as a message.",
        ],
      },
      {
        surface: "Terminal",
        steps: [
          "Click the 🎙 Voice button in the terminal toolbar.",
          "Speak into your microphone.",
          "Transcript appears in the terminal input row. Press Enter to send.",
        ],
      },
      {
        surface: "MAIC Chat",
        steps: [
          "Click the 🎙 button in the chat input toolbar.",
          "Speak — your words are transcribed and dropped into the chat input.",
          "Press Enter or click send.",
        ],
      },
    ],
    examples: [
      "Talk through your ideas instead of writing them out — just hit the mic and explain what you're thinking.",
      "Dictate a long email draft while looking at your notes.",
      "Use voice instead of typing when your hands are full (working on a car, eating lunch, etc.).",
    ],
    terminal: [
      {
        cmd: "/voice transcribe",
        desc: "Capture up to 30 seconds of audio (stops automatically when you stop talking), return the Whisper transcript.",
      },
      {
        cmd: "/voice check",
        desc: "Show whether the model is loaded, your mic device, audio host, and the model file location.",
      },
      {
        cmd: "/voice download_model",
        desc: "Download the ggml-tiny.en.bin Whisper model (~75MB) from milagrocloud.com with SHA256 verification. Safe to run multiple times.",
      },
      {
        cmd: "/voice cancel",
        desc: "Stop the current recording (if any) and return immediately.",
      },
    ],
    chat: [
      {
        cmd: "@voice transcribe",
        desc: "Same as /voice transcribe, but routed through MAIC chat. The transcript lands in your next message.",
      },
    ],
    troubleshooting: [
      {
        problem: "I click the mic button and the pop-up says \"No transcript captured\".",
        fix: "Three things can cause this, and the pop-up's Diagnostics section will tell you which one. (1) Microphone is silent or muted — check Windows Sound settings → make sure the right input device is selected and not muted. (2) Voice Activity Detector (VAD) didn't hear speech as speech — try speaking louder, getting closer to the mic, or disabling VAD in Settings → Modules → Voice. (3) Whisper heard audio but couldn't transcribe — try speaking more clearly, or re-install Voice v0.1.8 to make sure you have the latest fixes.",
      },
      {
        problem: "Pop-up says \"whisper model missing\".",
        fix: "The Whisper model hasn't been downloaded yet. Click the card's \"Download model\" button (~75MB download), or run /voice download_model in Terminal.",
      },
      {
        problem: "Transcription is slow (45-120 seconds for short utterances).",
        fix: "This is normal for CPU-only Whisper — your computer's processor is doing the speech recognition by itself. On most modern computers expect ~10-30× realtime (so a 10-second clip takes 0.3-1 seconds). Voice uses the tiny.en model by default for ~4× faster transcription. Switch to base.en for higher accuracy if you can spare the latency.",
      },
      {
        problem: "The 🎙 button is greyed out.",
        fix: "Voice module isn't installed. Open Settings → Modules → Catalog → Install Voice.",
      },
      {
        problem: "Transcription captures audio but the words come out wrong.",
        fix: "Whisper tiny.en is optimized for English and works best in quiet environments. Try moving away from background noise, or installing the larger base.en model for better accuracy (slower but more accurate).",
      },
    ],
  },
  translate: {
    whoFor:
      "Anyone working across languages — customer emails in Spanish, French supplier docs, bilingual support tickets.",
    whyUseIt:
      "Paste any text and pick a language — get a translation in seconds. Uses your MAIC account, so it knows business terms and your style preferences.",
    whatItDoes:
      "Translate text between 100+ languages using your local MAIC account. Drop a snippet into MAIC chat or use the standalone translate action.",
    windowsUI: [
      {
        surface: "MAIC Chat",
        steps: [
          "Click 🌍 in the chat input toolbar (or type /translate).",
          "Paste text, pick the source language (or 'auto') and the target language, click Translate.",
          "Translation lands in the chat input. Edit or send.",
        ],
      },
    ],
    examples: [
      "Translate a Spanish-language customer email so you can reply in English.",
      "Translate your English product description into Portuguese for a Brazilian listing.",
      "Translate a French supplier's invoice so you can verify line items.",
      "\"Translate this contract clause into plain English\" — for legal review.",
    ],
    terminal: [
      {
        cmd: "/translate text",
        desc: "Open the translate dialog. Enter text, source language (or 'auto'), and target language.",
      },
      {
        cmd: "/translate check",
        desc: "Show whether MAIC is reachable and the list of supported language codes.",
      },
    ],
    chat: [
      {
        cmd: "/translate <text> from <src> to <tgt>",
        desc: "Translate text inline. Example: /translate Hello world from en to es",
      },
      {
        cmd: "@translate",
        desc: "Open the translate dialog in chat.",
      },
    ],
    troubleshooting: [
      {
        problem: "Translate returns \"MAIC not reachable\".",
        fix: "MAIC backend is down or you're logged out. Check your account in Settings → Account. The translate sidecar routes through MAIC at https://maicserver.com/v1/chat/completions.",
      },
      {
        problem: "Translation looks wrong or stale.",
        fix: "Translation uses MAIC's chat completion API and is deterministic. If a translation seems off, the model is doing its best — for technical content, try a smaller chunk or specify 'from en' explicitly to skip auto-detect.",
      },
      {
        problem: "Language code not recognized.",
        fix: "Supported codes: es en fr de pt it nl ru zh ja ko ar hi tr pl sv da no fi el cs ro hu he th vi id ms tl uk. Anything else falls back to 'auto'.",
      },
    ],
  },
  pdf: {
    whoFor:
      "Anyone who reads a lot of PDFs or Word docs — contracts, manuals, invoices, research papers, reports.",
    whyUseIt:
      "Drop a PDF or Word file into chat and MAIC reads it for you. Ask questions, get a summary, or extract specific info — no copy-paste, no scrolling.",
    whatItDoes:
      "Drop a PDF or DOCX into MAIC chat and get clean text, a summary, or ask any question about the document. Files stay local until MAIC sees the extracted text.",
    windowsUI: [
      {
        surface: "MAIC Chat",
        steps: [
          "Drag a PDF or DOCX file into the chat input.",
          "MC extracts the text and prompts you: Extract / Summarize / Ask.",
          "Pick an action or write your own question.",
        ],
      },
      {
        surface: "Files page",
        steps: [
          "Browse to a PDF/DOCX.",
          "Click the row → \"Send to chat\" to drop the extracted text into MAIC chat.",
        ],
      },
    ],
    examples: [
      "Drop a 50-page manual into chat and ask \"What's the warranty section say?\" — instead of Ctrl+F-ing through it.",
      "Summarize a contract before you sign it.",
      "Pull all the line items out of an invoice PDF into a clean list.",
      "Compare two PDFs by extracting both and asking MAIC about the differences.",
      "Ask \"What does this error code mean in the troubleshooting guide?\" with the PDF as context.",
    ],
    terminal: [
      {
        cmd: "/pdf check",
        desc: "Show PDF module status: which formats are supported, whether MAIC is reachable.",
      },
      {
        cmd: "/pdf extract <path>",
        desc: "Extract text from a file. Output goes to stdout. Example: /pdf extract ~/Documents/manual.pdf",
      },
      {
        cmd: "/pdf summarize <path>",
        desc: "Extract text and ask MAIC for a summary. Returns the summary as a single string.",
      },
      {
        cmd: "/pdf ask <path> <question>",
        desc: "Extract text and ask MAIC a question about it. Example: /pdf ask contract.pdf What is the termination clause?",
      },
    ],
    chat: [
      {
        cmd: "/pdf extract <path>",
        desc: "Same as Terminal command but output drops into chat input.",
      },
      {
        cmd: "/pdf summarize <path>",
        desc: "Same.",
      },
      {
        cmd: "/pdf ask <path> <question>",
        desc: "Same.",
      },
    ],
    troubleshooting: [
      {
        problem: "PDF returns empty text.",
        fix: "v0.1.0 only handles text-based PDFs. Scanned or image-only PDFs require OCR, which ships in v0.2. If you have a text PDF that's empty, the file may be corrupted — try opening it in another viewer.",
      },
      {
        problem: "DOCX extraction is missing tables or images.",
        fix: "v0.1.0 extracts paragraphs only — tables and images are flattened or skipped. Use v0.2 (ships with pdfium-render) for layout-preserving extraction.",
      },
      {
        problem: "PDF is too large.",
        fix: "Max context window is 32,000 characters. For larger files, the sidecar truncates with a notice. Split the PDF or ask a more specific question.",
      },
      {
        problem: "Supported formats.",
        fix: "v0.1.0: PDF, DOCX, TXT, MD, RTF, HTML. v0.2 adds: scanned PDF (OCR), PPTX (planned).",
      },
    ],
  },
  youtube: {
    whoFor:
      "Anyone who watches tutorials, interviews, lectures, podcasts-on-YouTube — and wants the content as text they can search, quote, or summarize.",
    whyUseIt:
      "Paste a YouTube URL and get the full transcript, a chapter breakdown, or an AI-generated summary. Saves the transcript locally so you can search it later.",
    whatItDoes:
      "Paste a YouTube URL and get the full transcript, a chapter breakdown, and an AI-generated summary. Saves the transcript locally so you can search it later.",
    windowsUI: [
      {
        surface: "MAIC Chat",
        steps: [
          "Paste a YouTube URL into the chat input.",
          "MC detects the URL and prompts: Transcript / Summary / Chapters.",
          "Pick an action or ask a question about the video.",
        ],
      },
    ],
    examples: [
      "Paste a 2-hour podcast URL and ask \"What did they say about hiring?\" — instead of watching the whole thing.",
      "Get a chapter breakdown of a long tutorial so you can jump to the section you need.",
      "Summarize a keynote speech into 5 bullet points.",
      "Pull a timestamped transcript of a video you want to quote in a blog post.",
    ],
    terminal: [
      {
        cmd: "/youtube transcript <url>",
        desc: "Fetch the transcript (with timestamps) for a YouTube video.",
      },
      {
        cmd: "/youtube summary <url>",
        desc: "Fetch the transcript and ask MAIC for a summary.",
      },
      {
        cmd: "/youtube chapters <url>",
        desc: "Return the chapter breakdown with timestamps.",
      },
      {
        cmd: "/youtube check",
        desc: "Show module status: yt-dlp available, last fetch status.",
      },
    ],
    chat: [
      {
        cmd: "/youtube transcript <url>",
        desc: "Same as Terminal — output drops into chat input.",
      },
      {
        cmd: "/youtube summary <url>",
        desc: "Same.",
      },
    ],
    troubleshooting: [
      {
        problem: "Transcript unavailable for this video.",
        fix: "Some videos have transcripts disabled by the uploader. If the video has auto-generated captions, the sidecar falls back to those. Otherwise the sidecar can route the video's audio through Whisper (slow).",
      },
      {
        problem: "Module not installed.",
        fix: "If the YouTube Transcript card says \"Coming Soon\" instead of \"Install\", the module hasn't shipped in your current version of MC. Check for an update in Settings → Modules.",
      },
    ],
  },
  email: {
    whoFor:
      "Anyone who writes a lot of business email — sales outreach, customer replies, follow-ups, internal announcements.",
    whyUseIt:
      "Tell MAIC who you're emailing and why — get a complete draft tailored to the recipient. Pick a tone (formal, friendly, direct) and edit before sending.",
    whatItDoes:
      "Draft cold outreach, follow-ups, and reply emails. Pick a tone (formal / friendly / direct) and the module generates a complete email tailored to the recipient.",
    windowsUI: [
      {
        surface: "MAIC Chat",
        steps: [
          "Click ✉️ in the chat input toolbar (or type /email).",
          "Pick a template (cold outreach / follow-up / reply), paste the recipient context, choose a tone.",
          "Generated email lands in the chat input. Edit or send.",
        ],
      },
    ],
    examples: [
      "\"Draft a cold outreach email to a fleet manager introducing our brake service\" — pick the 'formal' tone.",
      "\"Write a friendly follow-up to a customer who got a quote last Tuesday and hasn't responded.\"",
      "\"Draft a reply to this support thread — the customer's printer is offline and they tried restarting.\"",
      "\"Write an internal email announcing the new shop hours next week.\"",
    ],
    terminal: [
      {
        cmd: "/email draft",
        desc: "Open the draft wizard: pick template, enter recipient + context, choose tone.",
      },
      {
        cmd: "/email check",
        desc: "Show module status: whether MAIC is reachable, supported tones.",
      },
    ],
    chat: [
      {
        cmd: "/email draft",
        desc: "Same as Terminal but the draft drops into the chat input.",
      },
      {
        cmd: "/email reply <thread>",
        desc: "Paste the email thread you're replying to and the tone. Returns a draft reply.",
      },
    ],
    troubleshooting: [
      {
        problem: "Module not installed.",
        fix: "If the Email Writer card says \"Coming Soon\" instead of \"Install\", the module hasn't shipped in your current version of MC. Check for an update in Settings → Modules.",
      },
      {
        problem: "Draft is too generic.",
        fix: "The more context you give (recipient name, company, what you're offering, why you're reaching out), the better the draft. Start with: \"I'm reaching out to [name] at [company] about [topic].\"",
      },
    ],
  },
  firecrawl: {
    whoFor:
      "Anyone who needs to read long web pages — research, competitor analysis, getting a blog post into MAIC chat.",
    whyUseIt:
      "Turn a messy web page into clean markdown that MAIC can read, summarize, or quote. No more copy-pasting chunks and hitting token limits.",
    whatItDoes:
      "Crawl a website and extract clean markdown content. Useful for feeding long web pages into MAIC chat without hitting token limits.",
    windowsUI: [
      {
        surface: "Settings → Modules",
        steps: [
          "Set your Firecrawl API key in Settings → Modules → Firecrawl.",
          "Click \"Run\" next to the Firecrawl card.",
        ],
      },
      {
        surface: "MAIC Chat",
        steps: [
          "Paste a URL into chat → click 🔥 when prompted → content is crawled + summarized.",
        ],
      },
    ],
    examples: [
      "Crawl a competitor's product page and ask \"What are their pricing tiers?\"",
      "Pull a long blog post into chat and ask for a 3-sentence summary.",
      "Crawl a directory of URLs and get a markdown index you can search.",
    ],
    terminal: [
      {
        cmd: "/firecrawl start",
        desc: "Start the Firecrawl worker (required before first use).",
      },
      {
        cmd: "/firecrawl stop",
        desc: "Stop the worker.",
      },
      {
        cmd: "/firecrawl crawl <url>",
        desc: "Crawl a URL, return markdown.",
      },
      {
        cmd: "/firecrawl check",
        desc: "Show API key status + worker state.",
      },
    ],
    chat: [
      {
        cmd: "/firecrawl <url>",
        desc: "Crawl a URL, drop the markdown into chat input.",
      },
    ],
    troubleshooting: [
      {
        problem: "Crawl fails with 401.",
        fix: "API key not set or invalid. Open Settings → Modules → Firecrawl → paste your key.",
      },
    ],
  },
  leadgen: {
    whoFor:
      "Sales teams, local services, B2B prospecting — anyone who needs a list of potential customers in a specific area + industry.",
    whyUseIt:
      "Search for businesses by location + industry and get name, address, phone, website, and a few enrichment signals — all in one query.",
    whatItDoes:
      "Find business leads by location + industry. Returns name, address, phone, website, and a few enrichment signals.",
    windowsUI: [
      {
        surface: "Settings → Modules",
        steps: [
          "Set your leadgen API key in Settings → Modules.",
          "Click \"Run\" next to the card.",
        ],
      },
    ],
    examples: [
      "\"Find me all the auto repair shops in Albuquerque with under 10 Google reviews\" — quick prospecting for partnership outreach.",
      "\"Get me a list of dentists in Rio Rancho, NM\" — for a new client referral program.",
      "\"Search for HVAC contractors in 87109 zip code\" — local competitor map.",
    ],
    terminal: [
      {
        cmd: "/leadgen search <industry> in <location>",
        desc: "Example: /leadgen search auto repair in Albuquerque, NM",
      },
      {
        cmd: "/leadgen check",
        desc: "Show API key status.",
      },
    ],
    chat: [
      {
        cmd: "@leadgen search <industry> in <location>",
        desc: "Same but routes through MAIC chat.",
      },
    ],
    troubleshooting: [
      {
        problem: "No results returned.",
        fix: "Check the industry spelling. Try a broader location (state vs city).",
      },
    ],
  },
  // ============================================================================
  // CRM Sync — coming in rc53.29 (rc53.29, 2026-08-26, David)
  //
  // Push enriched contacts and lead status to HubSpot, Pipedrive, Notion,
  // and other CRMs. Designed for sales teams + service shops that already
  // use a CRM and want MAIC to push structured data into it.
  // ============================================================================
  crm: {
    whoFor:
      "Sales teams, service shops, agencies — anyone who keeps customer data in HubSpot, Pipedrive, or Notion and wants MAIC to push enriched contacts and lead status into those systems automatically.",
    whyUseIt:
      "MAIC pulls contact info and lead status out of your chats, emails, voice notes, and transcripts — then pushes it straight into your CRM as structured records. No more manual data entry, no more copy-paste between tools.",
    whatItDoes:
      "Push enriched contacts and lead status to HubSpot, Pipedrive, Notion, and other CRMs. MAIC structures the data, CRM Sync delivers it via API.",
    windowsUI: [
      {
        surface: "Settings → Modules → CRM Sync",
        steps: [
          "Click the CRM Sync card to open its settings.",
          "Pick a target (HubSpot, Pipedrive, or Notion) and click Connect.",
          "Authorize in the browser window that opens. Your API key is stored locally.",
          "Repeat for any other CRMs you want to push to. All three can be active at once.",
        ],
      },
      {
        surface: "MAIC Chat",
        steps: [
          "After chatting with a lead or customer, type \"/crm push this contact to HubSpot\" or \"/crm sync recent\".",
          "MAIC previews the structured record before pushing — you confirm.",
          "CRM Sync delivers it. You get a confirmation toast with the CRM record ID.",
        ],
      },
    ],
    examples: [
      "After a 20-minute sales call with a fleet manager, type \"/crm push to HubSpot\" — MAIC extracts name, company, fleet size, pain points, and next steps, and pushes them as a HubSpot contact + deal.",
      "End of week: type \"/crm sync recent\" — MAIC pulls the last 7 days of contacts mentioned in chat and pushes the ones it can structure cleanly.",
      "\"Add this lead to my Notion CRM database — name is Sarah, company is Acme Logistics, they're a fleet of 12 trucks\" — quick capture.",
      "\"Show me what got pushed to Pipedrive this week\" — audit trail.",
    ],
    terminal: [
      {
        cmd: "/crm check",
        desc: "Show CRM Sync status: which targets are configured, last sync timestamps, API key health.",
      },
      {
        cmd: "/crm list_targets",
        desc: "List the connected CRMs (HubSpot / Pipedrive / Notion) and their connection status.",
      },
      {
        cmd: "/crm connect <target>",
        desc: "Connect a new CRM target. target = hubspot | pipedrive | notion. Opens browser for OAuth.",
      },
      {
        cmd: "/crm preview <contact_json>",
        desc: "Dry-run: show what would be pushed without actually pushing. Example: /crm preview '{\"name\":\"Sarah Chen\",\"company\":\"Acme Logistics\"}'",
      },
      {
        cmd: "/crm push <contact_json>",
        desc: "Push a structured contact to all connected CRMs. Example: /crm push '{\"name\":\"Sarah\",\"email\":\"s@acme.com\",\"stage\":\"qualified\"}'",
      },
      {
        cmd: "/crm sync_recent",
        desc: "Pull recent enriched contacts from MAIC session memory and push the ones that match contact patterns.",
      },
      {
        cmd: "/crm audit",
        desc: "Show the last 20 push attempts with status (success/failure), target, record ID, and timestamp.",
      },
    ],
    chat: [
      {
        cmd: "/crm push",
        desc: "Push the current conversation's extracted contact + lead status to all connected CRMs. MAIC previews first.",
      },
      {
        cmd: "/crm sync_recent",
        desc: "Pull the last 7 days of contacts from MAIC session memory and push.",
      },
      {
        cmd: "/crm audit",
        desc: "Show the last 20 push attempts.",
      },
    ],
    troubleshooting: [
      {
        problem: "OAuth window closed before I finished — connection didn't save.",
        fix: "Re-open Settings → Modules → CRM Sync → click Connect for the target again. OAuth windows have a 5-minute timeout.",
      },
      {
        problem: "Push fails with 401 / 403.",
        fix: "API key expired or was revoked. Open Settings → Modules → CRM Sync → Disconnect → Connect again. For HubSpot, also check that your connected app still has the scopes 'crm.objects.contacts.write' and 'crm.objects.deals.write'.",
      },
      {
        problem: "MAIC can't extract a contact from the conversation.",
        fix: "CRM Sync needs at least a name + email or name + company. Make sure the contact info is mentioned in chat (not just in a linked file). For voice transcripts, use the Voice module first so the transcript is in chat.",
      },
      {
        problem: "Notion push creates the contact but doesn't link it to my CRM database.",
        fix: "Notion pushes create standalone database entries. To link them to a parent CRM database, open Settings → Modules → CRM Sync → Notion → set the 'Parent Database ID' from your Notion database URL.",
      },
      {
        problem: "HubSpot rate limit hit (429 errors).",
        fix: "HubSpot allows 100 requests per 10 seconds per app. CRM Sync batches pushes in groups of 10, so you won't hit this unless you're pushing thousands at once. Wait 10 seconds and try again.",
      },
    ],
  },
  // ============================================================================
  // Text-to-Speech — coming in rc53.29 (rc53.29, 2026-08-26, David)
  //
  // Read MAIC's responses aloud in chat. Multiple voices, configurable
  // speed. Uses bundled eSpeak NG (offline, ~5MB, no cloud).
  // ============================================================================
  tts: {
    whoFor:
      "Anyone who reads more by listening than by looking — multitaskers, accessibility users, long technical responses, hands-free chat.",
    whyUseIt:
      "MAIC's responses can be long. Instead of reading them off the screen, click a button and have MAIC speak the answer out loud. Pick a voice you like, set the speed that works for you, and chat without looking at the screen.",
    whatItDoes:
      "Read MAIC's responses aloud in chat. Multiple voices, configurable speed. Bundled eSpeak NG runs offline — no audio leaves your machine.",
    windowsUI: [
      {
        surface: "MAIC Chat",
        steps: [
          "After installing TTS, you'll see a 🔊 button next to each AI response.",
          "Click 🔊 to have MAIC read that response aloud.",
          "Click again to stop. Adjust voice + speed in Settings → Modules → Text-to-Speech.",
        ],
      },
      {
        surface: "Settings → Modules → Text-to-Speech",
        steps: [
          "Pick a voice from the dropdown (en-us, en+f3 for slightly warmer, en+m4 for male, etc.).",
          "Drag the speed slider (default 175 words per minute; lower for slower, higher for faster).",
          "Click \"Test voice\" to hear a sample before saving.",
        ],
      },
    ],
    examples: [
      "Long technical explanation from MAIC? Click 🔊 and listen while you look at the related code.",
      "Hands full working on a car — keep listening to MAIC's diagnostic walkthrough instead of reading.",
      "Want to learn a new topic while walking? Read the docs with MAIC, then have MAIC explain them out loud.",
      "Accessibility: if reading long responses on screen is tiring, listen instead.",
    ],
    terminal: [
      {
        cmd: "/tts check",
        desc: "Show TTS status: whether eSpeak NG is bundled, current voice + speed.",
      },
      {
        cmd: "/tts speak <text>",
        desc: "Speak a snippet of text out loud. Example: /tts speak \"Hello, this is a voice test.\"",
      },
      {
        cmd: "/tts save <text> <path.wav>",
        desc: "Render text to a WAV file instead of playing it. Example: /tts save \"This is the spoken version\" ~/Desktop/greeting.wav",
      },
      {
        cmd: "/tts set_voice <name>",
        desc: "Switch voice. Names follow eSpeak conventions: en-us, en+f3 (slightly warmer), en+m4 (male), etc. Run /tts check for the full list.",
      },
      {
        cmd: "/tts set_speed <wpm>",
        desc: "Set speaking speed in words per minute. Default 175. Try 130 for slower, 220 for faster.",
      },
    ],
    chat: [
      {
        cmd: "/tts",
        desc: "Toggle the per-response 🔊 button on/off. When on, every MAIC response gets a speak button.",
      },
    ],
    troubleshooting: [
      {
        problem: "TTS audio sounds robotic.",
        fix: "That's eSpeak NG — it's a small offline engine with intentionally synthesized voices. For more natural voices, swap to MAIC cloud TTS (v0.2) which uses neural voices.",
      },
      {
        problem: "No audio plays after clicking 🔊.",
        fix: "Check your system volume and that the right output device is selected in Windows Sound settings. Run /tts check to confirm eSpeak NG is bundled correctly.",
      },
      {
        problem: "Voice is too fast / too slow.",
        fix: "Open Settings → Modules → Text-to-Speech and drag the speed slider. Or run /tts set_speed 130 (slower) or /tts set_speed 220 (faster).",
      },
      {
        problem: "Voice doesn't pronounce technical words right (API names, code, etc.).",
        fix: "eSpeak NG does its best with technical text. For code or weird acronyms, paste the text with hyphens or spelled-out syllables. v0.2 (MAIC cloud TTS) handles technical terms much better.",
      },
      {
        problem: "🔊 button isn't appearing next to MAIC responses.",
        fix: "Open Settings → Modules → Text-to-Speech and confirm the module is enabled. Toggle the per-response button with /tts in chat.",
      },
    ],
  },
};

export function getModuleHelp(id) {
  return MODULE_HELP[id] || null;
}