// src/data/module-help.js — Add-On Module help content (rc53.28, 2026-08-26)
//
// Canonical help content for every Add-On Module card. Each module's
// Help button (❔) opens an overlay that renders this data.
//
// Shape per module:
//   whatItDoes     — one-liner, used as the overlay's subtitle
//   windowsUI      — list of {surface, steps} for the Tauri desktop UI
//                    surfaces (Dashboard / Terminal / Files / Settings /
//                    Chat / Extras hub). Each surface = a clickable flow.
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
    whatItDoes:
      "Push-to-talk microphone that transcribes your speech locally with whisper.cpp and drops the text into any chat surface. No audio ever leaves your machine.",
    windowsUI: [
      {
        surface: "Dashboard",
        steps: [
          "Click the 🎙 FAB (bottom-right of dashboard).",
          "Speak into your microphone. Capture stops automatically when you stop talking for 1.5s, or after 30s.",
          "Transcript lands in the dashboard chat input. Edit if needed, then send.",
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
    terminal: [
      {
        cmd: "/voice transcribe",
        desc: "Capture 30s of audio (or stop early on silence), return the Whisper transcript.",
      },
      {
        cmd: "/voice check",
        desc: "Show model loaded?, mic device, audio host, and the path whisper.cpp is using.",
      },
      {
        cmd: "/voice download_model",
        desc: "Download the ggml-tiny.en.bin Whisper model (~75MB) from milagrocloud.com with SHA256 verification. Idempotent.",
      },
      {
        cmd: "/voice cancel",
        desc: "Stop the current capture (if any) and return immediately.",
      },
    ],
    chat: [
      {
        cmd: "@voice transcribe",
        desc: "Same as /voice transcribe, routed through MAIC chat. Drop the transcript into the next message.",
      },
    ],
    troubleshooting: [
      {
        problem: "🎙 says 'No Speech Detected' on Dashboard but works in Terminal.",
        fix: "The Dashboard 🎙 and Terminal 🎙 both call mc_voice_transcribe with the same args. If Terminal works, the sidecar + mic are healthy. Try clicking again with longer speech (3-5s of clear talking). If still failing, check Windows microphone privacy settings — 'Allow desktop apps to access your microphone' must be on.",
      },
      {
        problem: "🎙 says 'whisper model missing'.",
        fix: "The Whisper model hasn't been downloaded yet. Click the card's 'Download model' button (~75MB download), or run /voice download_model in Terminal.",
      },
      {
        problem: "Transcription is slow (45-120s for short utterances).",
        fix: "This is normal for CPU-bound Whisper. On modern CPUs expect ~10-30× realtime. For ~4× faster inference, the sidecar uses the tiny.en model by default. Switch to base.en for higher accuracy if you can spare the latency.",
      },
      {
        problem: "🎙 button is greyed out.",
        fix: "Voice module isn't installed. Open Settings → Modules → Catalog → Install Voice.",
      },
    ],
  },
  translate: {
    whatItDoes:
      "Translate text between 100+ languages using your local MAIC account. Drop a snippet into MAIC chat or use the standalone translate action.",
    windowsUI: [
      {
        surface: "MAIC Chat",
        steps: [
          "Click 🌍 in the chat input toolbar (or type /translate).",
          "Paste text, pick source + target language, click Translate.",
          "Translation lands in the chat input. Edit or send.",
        ],
      },
    ],
    terminal: [
      {
        cmd: "/translate text",
        desc: "Open the translate dialog. Enter text, source lang (or 'auto'), and target lang.",
      },
      {
        cmd: "/translate check",
        desc: "Show MAIC reachability and the list of supported language codes.",
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
        problem: "Translate returns 'MAIC not reachable'.",
        fix: "MAIC backend is down or you're logged out. Check your account in Settings → Account. The translate sidecar routes through MAIC at https://maicserver.com/v1/chat/completions.",
      },
      {
        problem: "Translation looks wrong or stale.",
        fix: "Translation uses MAIC's chat completion API (temp=0, deterministic). If a translation seems off, the model is doing its best — for technical content, try a smaller chunk or specify 'from en' explicitly to skip auto-detect.",
      },
      {
        problem: "Language code not recognized.",
        fix: "Supported codes: es en fr de pt it nl ru zh ja ko ar hi tr pl sv da no fi el cs ro hu he th vi id ms tl uk. Anything else falls back to 'auto'.",
      },
    ],
  },
  pdf: {
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
          "Click the row → 'Send to chat' to drop the extracted text into MAIC chat.",
        ],
      },
    ],
    terminal: [
      {
        cmd: "/pdf check",
        desc: "Show PDF module status: which formats are supported, MAIC reachability.",
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
        fix: "v0.1.0 only handles text-based PDFs. Scanned/image-only PDFs require OCR, which ships in v0.2. If you have a text PDF that's empty, the file may be corrupted — try opening it in another viewer.",
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
        desc: "Show module status: youtube-dl/yt-dlp available, last fetch status.",
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
        fix: "v0.1.0 ships in the next build (rc53.28). Until then, the YouTube card is Coming Soon.",
      },
    ],
  },
  email: {
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
    terminal: [
      {
        cmd: "/email draft",
        desc: "Open the draft wizard: pick template, enter recipient + context, choose tone.",
      },
      {
        cmd: "/email check",
        desc: "Show module status: MAIC reachability, supported tones.",
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
        fix: "v0.1.0 ships in the next build (rc53.28). Until then, the Email Writer card is Coming Soon.",
      },
      {
        problem: "Draft is too generic.",
        fix: "The more context you give (recipient name, company, what you're offering, why you're reaching out), the better the draft. Start with: 'I'm reaching out to [name] at [company] about [topic].'",
      },
    ],
  },
  firecrawl: {
    whatItDoes:
      "Crawl a website and extract clean markdown content. Useful for feeding long web pages into MAIC chat without hitting token limits.",
    windowsUI: [
      {
        surface: "Settings → Modules",
        steps: [
          "Set your Firecrawl API key in Settings → Modules → Firecrawl.",
          "Click 'Run' next to the Firecrawl card.",
        ],
      },
      {
        surface: "MAIC Chat",
        steps: [
          "Paste a URL into chat → click 🔥 when prompted → content is crawled + summarized.",
        ],
      },
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
    whatItDoes:
      "Find business leads by location + industry. Returns name, address, phone, website, and a few enrichment signals.",
    windowsUI: [
      {
        surface: "Settings → Modules",
        steps: [
          "Set your leadgen API key in Settings → Modules.",
          "Click 'Run' next to the card.",
        ],
      },
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
};

export function getModuleHelp(id) {
  return MODULE_HELP[id] || null;
}
