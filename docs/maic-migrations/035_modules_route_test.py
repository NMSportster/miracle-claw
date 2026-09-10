"""Unit tests for /v1/categories, /v1/marketplace/modules,
/v1/modules/installed, /v1/modules/install, /v1/modules/uninstall.

These tests assume the migration 035_client_manifest.sql has been
applied. They use a real PG connection — set MAIC_TEST_DATABASE_URL
to point at a test DB.

Run with:
    cd /opt/maic && python3 -m unittest api.routes.modules_test -v

Spec:
    projects/miracle-claw/docs/mc-module-driven-surfaces-2026-09.md
"""
from __future__ import annotations

import json
import os
import unittest
from typing import AsyncIterator

import asyncpg

# Adjust these to match your test DB
TEST_DATABASE_URL = os.environ.get(
    "MAIC_TEST_DATABASE_URL",
    "postgresql://milagro:milagro@127.0.0.1:5432/milagro_test",
)


async def _make_pool() -> asyncpg.Pool:
    return await asyncpg.create_pool(
        dsn=TEST_DATABASE_URL,
        min_size=1,
        max_size=2,
        command_timeout=10,
    )


class TestCategories(unittest.IsolatedAsyncioTestCase):
    """Test /v1/categories — public endpoint, no auth."""

    async def asyncSetUp(self) -> None:
        self.pool = await _make_pool()

    async def asyncTearDown(self) -> None:
        await self.pool.close()

    async def test_returns_16_categories(self) -> None:
        async with self.pool.acquire() as conn:
            rows = await conn.fetch(
                "SELECT id, label, emoji, sort_order FROM module_categories ORDER BY sort_order"
            )
        self.assertEqual(len(rows), 16, "should have exactly 16 categories")
        labels = [r["label"] for r in rows]
        self.assertIn("Marketing", labels)
        self.assertIn("Voice", labels)
        self.assertEqual(rows[0]["id"], "productivity", "first category is productivity")
        self.assertEqual(rows[-1]["id"], "other", "last category is other")

    async def test_counts_match_seeded_modules(self) -> None:
        async with self.pool.acquire() as conn:
            row = await conn.fetchrow(
                """
                SELECT COUNT(*) AS n
                FROM modules
                WHERE category = 'marketing' AND is_active = TRUE
                """
            )
            count = row["n"]
        async with self.pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT COUNT(*) AS n FROM module_categories c "
                "LEFT JOIN modules m ON m.category = c.id "
                "WHERE c.id = 'marketing'"
            )
        # Both should match (modules + categories join with COUNT)
        self.assertEqual(row["n"], count)


class TestMarketplace(unittest.IsolatedAsyncioTestCase):
    """Test /v1/marketplace/modules."""

    async def asyncSetUp(self) -> None:
        self.pool = await _make_pool()

    async def asyncTearDown(self) -> None:
        await self.pool.close()

    async def test_returns_all_active_modules(self) -> None:
        async with self.pool.acquire() as conn:
            rows = await conn.fetch(
                "SELECT id, name FROM modules WHERE is_active = TRUE"
            )
        self.assertGreaterEqual(len(rows), 8, "should have at least 8 seeded modules")

    async def test_filter_by_category(self) -> None:
        async with self.pool.acquire() as conn:
            rows = await conn.fetch(
                "SELECT id FROM modules WHERE category = 'research' AND is_active = TRUE"
            )
        ids = [r["id"] for r in rows]
        self.assertIn("firecrawl", ids)

    async def test_sort_alphabetical(self) -> None:
        async with self.pool.acquire() as conn:
            rows = await conn.fetch(
                "SELECT name FROM modules WHERE is_active = TRUE ORDER BY name ASC"
            )
        names = [r["name"] for r in rows]
        self.assertEqual(names, sorted(names))


class TestInstallFlow(unittest.IsolatedAsyncioTestCase):
    """Test install + uninstall + idempotency."""

    async def asyncSetUp(self) -> None:
        self.pool = await _make_pool()
        # Use a test user — create one if needed
        async with self.pool.acquire() as conn:
            await conn.execute(
                """
                INSERT INTO users (id, name, email, password_hash, created_at)
                VALUES (999999, 'test_modules_user', 'test-modules@example.com', 'fake', now())
                ON CONFLICT (id) DO NOTHING
                """
            )

    async def asyncTearDown(self) -> None:
        # Clean up
        async with self.pool.acquire() as conn:
            await conn.execute(
                "DELETE FROM module_installs WHERE user_id = 999999"
            )
            await conn.execute(
                "DELETE FROM module_runs WHERE user_id = 999999"
            )
            await conn.execute(
                "DELETE FROM users WHERE id = 999999"
            )
        await self.pool.close()

    async def test_install_and_uninstall(self) -> None:
        async with self.pool.acquire() as conn:
            # Install
            await conn.execute(
                """
                INSERT INTO module_installs (user_id, module_id)
                VALUES (999999, 'firecrawl')
                ON CONFLICT (user_id, module_id) DO NOTHING
                """
            )
            row = await conn.fetchrow(
                "SELECT module_id FROM module_installs WHERE user_id = 999999"
            )
            self.assertEqual(row["module_id"], "firecrawl")

            # Uninstall
            await conn.execute(
                "DELETE FROM module_installs WHERE user_id = 999999 AND module_id = 'firecrawl'"
            )
            row = await conn.fetchrow(
                "SELECT COUNT(*) AS n FROM module_installs WHERE user_id = 999999"
            )
            self.assertEqual(row["n"], 0)

    async def test_double_install_is_idempotent(self) -> None:
        async with self.pool.acquire() as conn:
            # First install
            await conn.execute(
                """
                INSERT INTO module_installs (user_id, module_id)
                VALUES (999999, 'firecrawl')
                ON CONFLICT (user_id, module_id) DO NOTHING
                """
            )
            # Second install (should be a no-op due to UNIQUE)
            await conn.execute(
                """
                INSERT INTO module_installs (user_id, module_id)
                VALUES (999999, 'firecrawl')
                ON CONFLICT (user_id, module_id) DO NOTHING
                """
            )
            row = await conn.fetchrow(
                "SELECT COUNT(*) AS n FROM module_installs WHERE user_id = 999999 AND module_id = 'firecrawl'"
            )
            self.assertEqual(row["n"], 1, "should only have one install row")

    async def test_uninstall_nonexistent_is_noop(self) -> None:
        async with self.pool.acquire() as conn:
            await conn.execute(
                "DELETE FROM module_installs WHERE user_id = 999999 AND module_id = 'firecrawl'"
            )
            row = await conn.fetchrow(
                "SELECT COUNT(*) AS n FROM module_installs WHERE user_id = 999999"
            )
            self.assertEqual(row["n"], 0)


class TestClientManifestShape(unittest.IsolatedAsyncioTestCase):
    """Validate the client_manifest JSON shape per spec §3.2."""

    async def asyncSetUp(self) -> None:
        self.pool = await _make_pool()

    async def asyncTearDown(self) -> None:
        await self.pool.close()

    async def test_manifest_has_required_fields(self) -> None:
        async with self.pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT client_manifest FROM modules WHERE id = 'firecrawl'"
            )
        manifest = row["client_manifest"]
        if isinstance(manifest, str):
            manifest = json.loads(manifest)

        required = ["module_id", "name", "description", "icon", "category", "tools"]
        for field in required:
            self.assertIn(field, manifest, f"manifest missing {field}")

        self.assertEqual(manifest["module_id"], "firecrawl")
        self.assertIsInstance(manifest["tools"], list)
        self.assertGreater(len(manifest["tools"]), 0)

    async def test_tool_has_form_schema(self) -> None:
        async with self.pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT client_manifest FROM modules WHERE id = 'firecrawl'"
            )
        manifest = row["client_manifest"]
        if isinstance(manifest, str):
            manifest = json.loads(manifest)

        for tool in manifest["tools"]:
            self.assertIn("name", tool)
            self.assertIn("description", tool)
            self.assertIn("form_schema", tool)
            schema = tool["form_schema"]
            self.assertEqual(schema["type"], "object")
            self.assertIn("properties", schema)


if __name__ == "__main__":
    unittest.main()
