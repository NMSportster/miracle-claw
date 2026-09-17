-- Migration 035: client_manifest module system
-- (rc56.0 module-driven surfaces, David 2026-09-09, spec:
--  projects/miracle-claw/docs/mc-module-driven-surfaces-2026-09.md)
--
-- Purpose: introduce the module system that powers the new dashboard
-- launchpad, the marketplace page, and the cross-surface (Tauri app +
-- mc-openclaw TUI + mclaw CLI) module rendering.
--
-- Three tables:
--
--   - modules          — registry of every available module
--   - module_installs  — which user has which module installed
--   - module_runs      — usage log for "Recent" section on dashboard
--
-- Plus a CHECK constraint on category against the 16-category
-- controlled vocabulary (see §3.3 of the spec).
--
-- Design notes:
--
--   - modules.client_manifest is JSONB so we can iterate the schema
--     without DDL migrations every time we add a field. The shape is
--     documented in §3.2 of the spec.
--
--   - category is a TEXT with a CHECK constraint against the 16 allowed
--     values. We could have used a Postgres ENUM but ENUMs are a pain
--     to extend (requires ALTER TYPE ADD VALUE outside a transaction).
--     A CHECK is easier to evolve.
--
--   - subcategory is freeform text. We'll add a controlled vocabulary
--     when we hit 50+ modules (per spec §8, out of scope for v1).
--
--   - tier_required defaults to 'free'. Modules that need a paid tier
--     set it explicitly. Used by the install endpoint to gate
--     installs for users on lower tiers.
--
--   - rating + install_count are denormalized counters updated
--     out-of-band (we don't have ratings UI yet, but the column is
--     there for the marketplace sort to work).
--
--   - module_installs has a unique (user_id, module_id) so a user
--     can't double-install the same module.
--
--   - module_runs is append-only. The "Recent" section on the
--     dashboard reads the last 3 runs per user. No retention cron
--     yet; we'll add a 90-day vacuum in a follow-up.
--
-- Idempotent. Safe to re-run.

BEGIN;

-- ─────────────────────────────────────────────────────────────────────
-- 1. The 16-category controlled vocabulary
-- ─────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS module_categories (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    emoji       TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0
);

INSERT INTO module_categories (id, label, emoji, sort_order) VALUES
    ('productivity',    'Productivity',     '✏️', 10),
    ('communication',   'Communication',    '✉️', 20),
    ('research',        'Research',         '🔍', 30),
    ('data-analysis',   'Data Analysis',    '📊', 40),
    ('creative',        'Creative',         '🎨', 50),
    ('marketing',       'Marketing',        '📣', 60),
    ('sales',           'Sales',            '💼', 70),
    ('finance',         'Finance',          '💰', 80),
    ('legal',           'Legal',            '⚖️', 90),
    ('engineering',     'Engineering',      '🛠️', 100),
    ('hr',              'HR',               '👥', 110),
    ('scheduling',      'Scheduling',       '📅', 120),
    ('files',           'Files',            '📄', 130),
    ('media',           'Media',            '🎬', 140),
    ('voice',           'Voice',            '🎙️', 150),
    ('other',           'Other',            '📦', 999)
ON CONFLICT (id) DO UPDATE SET
    label = EXCLUDED.label,
    emoji = EXCLUDED.emoji,
    sort_order = EXCLUDED.sort_order;

-- ─────────────────────────────────────────────────────────────────────
-- 2. modules — registry of every available module
-- ─────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS modules (
    id                TEXT PRIMARY KEY,           -- slug, e.g. "firecrawl"
    name              TEXT NOT NULL,
    description       TEXT NOT NULL,
    icon              TEXT NOT NULL DEFAULT '📦',
    category          TEXT NOT NULL,
    subcategory       TEXT,
    tier_required     TEXT NOT NULL DEFAULT 'free'
        CHECK (tier_required IN ('free', 'pro', 'team', 'enterprise')),
    author            TEXT NOT NULL DEFAULT 'Miracle Claw Labs',
    client_manifest   JSONB NOT NULL,             -- shape per spec §3.2
    rating            NUMERIC(3, 2) NOT NULL DEFAULT 0.00
        CHECK (rating >= 0 AND rating <= 5),
    install_count     INTEGER NOT NULL DEFAULT 0,
    is_first_party    BOOLEAN NOT NULL DEFAULT FALSE,
    is_active         BOOLEAN NOT NULL DEFAULT TRUE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT modules_category_fkey
        FOREIGN KEY (category) REFERENCES module_categories(id)
        ON UPDATE CASCADE
);

CREATE INDEX IF NOT EXISTS modules_category_idx
    ON modules (category, is_active);

CREATE INDEX IF NOT EXISTS modules_rating_idx
    ON modules (rating DESC, install_count DESC)
    WHERE is_active = TRUE;

CREATE INDEX IF NOT EXISTS modules_recent_idx
    ON modules (created_at DESC)
    WHERE is_active = TRUE;

-- updated_at trigger
CREATE OR REPLACE FUNCTION update_modules_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_modules_updated_at ON modules;
CREATE TRIGGER trg_modules_updated_at
BEFORE UPDATE ON modules
FOR EACH ROW EXECUTE FUNCTION update_modules_updated_at();

-- ─────────────────────────────────────────────────────────────────────
-- 3. module_installs — per-user installed modules
-- ─────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS module_installs (
    id              BIGSERIAL PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    module_id       TEXT NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at    TIMESTAMPTZ,
    UNIQUE (user_id, module_id)
);

CREATE INDEX IF NOT EXISTS module_installs_user_idx
    ON module_installs (user_id, installed_at DESC);

CREATE INDEX IF NOT EXISTS module_installs_last_used_idx
    ON module_installs (user_id, last_used_at DESC)
    WHERE last_used_at IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────
-- 4. module_runs — append-only usage log
-- ─────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS module_runs (
    id              BIGSERIAL PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    module_id       TEXT NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    tool_name       TEXT NOT NULL,
    inputs          JSONB,
    outputs         JSONB,
    duration_ms     INTEGER,
    surface         TEXT,                          -- "tauri" / "tui" / "mclaw"
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS module_runs_user_recent_idx
    ON module_runs (user_id, created_at DESC);

-- ─────────────────────────────────────────────────────────────────────
-- 5. Seed the 8 starter modules (per spec §4.1, 6-8 across categories)
-- ─────────────────────────────────────────────────────────────────────

INSERT INTO modules (id, name, description, icon, category, subcategory, tier_required, author, client_manifest, is_first_party, rating, install_count) VALUES
    (
        'marketing-suite',
        'Marketing Suite',
        'Email campaigns, social posts, and ad copy from a single brief',
        '📣',
        'marketing',
        'campaigns',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "marketing-suite",
            "name": "Marketing Suite",
            "description": "Email campaigns, social posts, and ad copy from a single brief",
            "icon": "📣",
            "category": "marketing",
            "subcategory": "campaigns",
            "tier_required": "free",
            "tools": [
                {
                    "name": "draft_email",
                    "description": "Compose a marketing email from key points",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "to_audience": { "type": "string", "title": "To audience", "minLength": 1 },
                            "tone": { "type": "string", "title": "Tone", "enum": ["friendly", "professional", "urgent", "playful"] },
                            "key_points": { "type": "string", "title": "Key points", "minLength": 1 }
                        },
                        "required": ["to_audience", "key_points"]
                    }
                },
                {
                    "name": "social_post",
                    "description": "Generate a social post for a platform",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "platform": { "type": "string", "title": "Platform", "enum": ["twitter", "linkedin", "facebook", "instagram"] },
                            "topic": { "type": "string", "title": "Topic", "minLength": 1 }
                        },
                        "required": ["platform", "topic"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.7,
        0
    ),
    (
        'email-campaigns-pro',
        'Email Campaigns Pro',
        'Sequenced email drips with A/B subject lines',
        '✉️',
        'communication',
        'newsletters',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "email-campaigns-pro",
            "name": "Email Campaigns Pro",
            "description": "Sequenced email drips with A/B subject lines",
            "icon": "✉️",
            "category": "communication",
            "subcategory": "newsletters",
            "tier_required": "free",
            "tools": [
                {
                    "name": "sequence",
                    "description": "Build a 3-step email sequence",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "goal": { "type": "string", "title": "Goal", "minLength": 1 },
                            "audience": { "type": "string", "title": "Audience" }
                        },
                        "required": ["goal"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.5,
        0
    ),
    (
        'firecrawl',
        'Firecrawl',
        'Pull clean content from any web page',
        '🌐',
        'research',
        'web-scraping',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "firecrawl",
            "name": "Firecrawl",
            "description": "Pull clean content from any web page",
            "icon": "🌐",
            "category": "research",
            "subcategory": "web-scraping",
            "tier_required": "free",
            "tools": [
                {
                    "name": "fetch_url",
                    "description": "Fetch clean content from a URL",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "title": "URL", "minLength": 1 }
                        },
                        "required": ["url"]
                    }
                },
                {
                    "name": "crawl_site",
                    "description": "Crawl a whole site (up to N pages)",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "start_url": { "type": "string", "title": "Start URL", "minLength": 1 },
                            "max_pages": { "type": "integer", "title": "Max pages", "minimum": 1, "maximum": 100, "default": 10 }
                        },
                        "required": ["start_url"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.8,
        0
    ),
    (
        'ad-performance',
        'Ad Performance',
        'Analyze ad campaign metrics and find underperformers',
        '📊',
        'data-analysis',
        'advertising',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "ad-performance",
            "name": "Ad Performance",
            "description": "Analyze ad campaign metrics and find underperformers",
            "icon": "📊",
            "category": "data-analysis",
            "subcategory": "advertising",
            "tier_required": "free",
            "tools": [
                {
                    "name": "summarize",
                    "description": "Summarize ad performance for a date range",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "date_range": { "type": "string", "title": "Date range", "minLength": 1 },
                            "campaign_id": { "type": "string", "title": "Campaign ID (optional)" }
                        },
                        "required": ["date_range"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.3,
        0
    ),
    (
        'calendar-companion',
        'Calendar Companion',
        'Smart scheduling with conflict detection',
        '📅',
        'scheduling',
        'meetings',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "calendar-companion",
            "name": "Calendar Companion",
            "description": "Smart scheduling with conflict detection",
            "icon": "📅",
            "category": "scheduling",
            "subcategory": "meetings",
            "tier_required": "free",
            "tools": [
                {
                    "name": "find_slot",
                    "description": "Find an open time slot for a meeting",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "duration_minutes": { "type": "integer", "title": "Duration (minutes)", "minimum": 15, "maximum": 480, "default": 30 },
                            "attendees": { "type": "string", "title": "Attendees (comma-separated emails)" }
                        },
                        "required": ["duration_minutes"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.6,
        0
    ),
    (
        'brand-asset-kit',
        'Brand Asset Kit',
        'Generate on-brand social graphics and bios',
        '🎨',
        'creative',
        'branding',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "brand-asset-kit",
            "name": "Brand Asset Kit",
            "description": "Generate on-brand social graphics and bios",
            "icon": "🎨",
            "category": "creative",
            "subcategory": "branding",
            "tier_required": "free",
            "tools": [
                {
                    "name": "bio",
                    "description": "Write a brand-aligned bio",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "platform": { "type": "string", "title": "Platform", "enum": ["twitter", "linkedin", "instagram", "website"] },
                            "keywords": { "type": "string", "title": "Keywords (comma-separated)" }
                        },
                        "required": ["platform"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.4,
        0
    ),
    (
        'voice-memo',
        'Voice Memo',
        'Transcribe audio memos and extract action items',
        '🎙️',
        'voice',
        'transcription',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "voice-memo",
            "name": "Voice Memo",
            "description": "Transcribe audio memos and extract action items",
            "icon": "🎙️",
            "category": "voice",
            "subcategory": "transcription",
            "tier_required": "free",
            "tools": [
                {
                    "name": "transcribe",
                    "description": "Transcribe an audio file",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "audio_url": { "type": "string", "title": "Audio file URL", "minLength": 1 }
                        },
                        "required": ["audio_url"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.2,
        0
    ),
    (
        'data-analyzer',
        'Data Analyzer',
        'CSV/spreadsheet analysis with plain-English summaries',
        '📊',
        'data-analysis',
        'spreadsheets',
        'free',
        'Miracle Claw Labs',
        '{
            "module_id": "data-analyzer",
            "name": "Data Analyzer",
            "description": "CSV/spreadsheet analysis with plain-English summaries",
            "icon": "📊",
            "category": "data-analysis",
            "subcategory": "spreadsheets",
            "tier_required": "free",
            "tools": [
                {
                    "name": "summarize",
                    "description": "Summarize a CSV",
                    "form_schema": {
                        "type": "object",
                        "properties": {
                            "csv_url": { "type": "string", "title": "CSV URL", "minLength": 1 },
                            "focus": { "type": "string", "title": "What to focus on" }
                        },
                        "required": ["csv_url"]
                    }
                }
            ]
        }'::jsonb,
        TRUE,
        4.5,
        0
    )
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    description = EXCLUDED.description,
    icon = EXCLUDED.icon,
    category = EXCLUDED.category,
    subcategory = EXCLUDED.subcategory,
    client_manifest = EXCLUDED.client_manifest,
    updated_at = now();

COMMIT;
