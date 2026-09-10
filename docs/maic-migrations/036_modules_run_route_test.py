"""Unit tests for POST /v1/modules/{module_id}/run.

These tests assume migrations 035 and 036 are applied, and the
8 starter modules are seeded. They use a real PG connection —
set MAIC_TEST_DATABASE_URL to point at a test DB.

Run with:
    cd /opt/maic && python3 -m unittest api.routes.modules_run_test -v

Spec:
    projects/miracle-claw/docs/mc-module-driven-surfaces-2026-09.md
    §3.5 (install flow), §5.2 (TUI), §5.3 (mclaw CLI).
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
        dsn=TEST_DATABASE_URL, min_size=1, max_size=2, command_timeout=10,
    )


# Skip the whole module if no test DB is reachable. The CI environment
# for the production MAIC server doesn't have a test DB; these are run
# manually before deploys from a dev box.
@unittest.skipUnless(
    os.environ.get("MAIC_RUN_ROUTE_TESTS") == "1",
    "set MAIC_RUN_ROUTE_TESTS=1 to enable run-route tests",
)
class TestRunModule(unittest.IsolatedAsyncioTestCase):
    """Test the module run route end-to-end (with mocked LiteLLM)."""

    async def asyncSetUp(self) -> None:
        self.pool = await _make_pool()
        # Wipe any prior test state
        async with self.pool.acquire() as conn:
            await conn.execute(
                "DELETE FROM module_runs WHERE user_id IN (SELECT id FROM users WHERE email = 'test-run@example.com')"
            )
            await conn.execute("DELETE FROM module_installs WHERE user_id IN (SELECT id FROM users WHERE email = 'test-run@example.com')")
            await conn.execute("DELETE FROM users WHERE email = 'test-run@example.com'")
            # Insert a test user
            await conn.execute(
                "INSERT INTO users (email, name, tier) VALUES ('test-run@example.com', 'test-run', 'pro')"
            )

    async def asyncTearDown(self) -> None:
        async with self.pool.acquire() as conn:
            await conn.execute(
                "DELETE FROM module_runs WHERE user_id IN (SELECT id FROM users WHERE email = 'test-run@example.com')"
            )
            await conn.execute("DELETE FROM module_installs WHERE user_id IN (SELECT id FROM users WHERE email = 'test-run@example.com')")
            await conn.execute("DELETE FROM users WHERE email = 'test-run@example.com'")
        await self.pool.close()

    async def test_run_requires_install(self) -> None:
        """User without the module installed gets 403."""
        from fastapi import HTTPException
        from api.routes.modules_run import run_module, RunIn

        # Mock Principal
        class FakePrincipal:
            user_id = None
            name = "test-run"
            tier = "pro"

        async with self.pool.acquire() as conn:
            uid = await conn.fetchval(
                "SELECT id FROM users WHERE email = 'test-run@example.com'"
            )

        class P:
            pass
        p = P()
        p.user_id = uid
        p.name = "test-run"
        p.tier = "pro"

        with self.assertRaises(HTTPException) as ctx:
            await run_module(
                body=RunIn(tool="fetch_url", inputs={"url": "https://example.com"}, surface="tui"),
                module_id="firecrawl",
                p=p,
            )
        self.assertEqual(ctx.exception.status_code, 403)

    async def test_run_rejects_unknown_tool(self) -> None:
        """Unknown tool name on an installed module returns 404."""
        from fastapi import HTTPException
        from api.routes.modules_run import run_module, RunIn

        async with self.pool.acquire() as conn:
            uid = await conn.fetchval(
                "SELECT id FROM users WHERE email = 'test-run@example.com'"
            )
            await conn.execute(
                "INSERT INTO module_installs (user_id, module_id) VALUES ($1, 'firecrawl')",
                uid,
            )

        class P:
            pass
        p = P()
        p.user_id = uid
        p.name = "test-run"
        p.tier = "pro"

        with self.assertRaises(HTTPException) as ctx:
            await run_module(
                body=RunIn(tool="nonexistent", inputs={}, surface="tui"),
                module_id="firecrawl",
                p=p,
            )
        self.assertEqual(ctx.exception.status_code, 404)

    async def test_run_rejects_non_first_party(self) -> None:
        """Non-first-party modules return 400 (no v1 dispatcher)."""
        from fastapi import HTTPException
        from api.routes.modules_run import run_module, RunIn

        async with self.pool.acquire() as conn:
            uid = await conn.fetchval(
                "SELECT id FROM users WHERE email = 'test-run@example.com'"
            )
            # Insert a non-first-party module + install it
            await conn.execute(
                """
                INSERT INTO modules (id, name, description, icon, category,
                                     tier_required, author, client_manifest,
                                     is_first_party, is_active, rating, install_count)
                VALUES ('third-party-test', 'Third', '3p', 'X', 'test',
                        'free', 'tester', '{"tools": []}'::jsonb,
                        FALSE, TRUE, 0, 0)
                ON CONFLICT (id) DO UPDATE SET is_first_party = FALSE
                """
            )
            await conn.execute(
                "INSERT INTO module_installs (user_id, module_id) VALUES ($1, 'third-party-test')",
                uid,
            )

        class P:
            pass
        p = P()
        p.user_id = uid
        p.name = "test-run"
        p.tier = "pro"

        with self.assertRaises(HTTPException) as ctx:
            await run_module(
                body=RunIn(tool="anything", inputs={}, surface="tui"),
                module_id="third-party-test",
                p=p,
            )
        self.assertEqual(ctx.exception.status_code, 400)

    async def test_run_logs_to_module_runs(self) -> None:
        """Successful run inserts a module_runs row with surface='tui'."""
        # This test requires the real LiteLLM to be reachable; we mock it
        # via monkeypatching httpx.AsyncClient.
        import httpx
        from api.routes import modules_run

        class FakeResponse:
            status_code = 200
            def raise_for_status(self): pass
            def json(self):
                return {"choices": [{"message": {"content": "stub model output"}}]}

        class FakeAsyncClient:
            def __init__(self, *a, **kw): pass
            async def __aenter__(self): return self
            async def __aexit__(self, *a): pass
            async def post(self, *a, **kw): return FakeResponse()

        original = modules_run.httpx.AsyncClient
        modules_run.httpx.AsyncClient = FakeAsyncClient
        try:
            from api.routes.modules_run import run_module, RunIn

            async with self.pool.acquire() as conn:
                uid = await conn.fetchval(
                    "SELECT id FROM users WHERE email = 'test-run@example.com'"
                )
                await conn.execute(
                    "INSERT INTO module_installs (user_id, module_id) VALUES ($1, 'firecrawl')",
                    uid,
                )

            class P:
                pass
            p = P()
            p.user_id = uid
            p.name = "test-run"
            p.tier = "pro"

            result = await run_module(
                body=RunIn(tool="fetch_url", inputs={"url": "https://example.com"}, surface="tui"),
                module_id="firecrawl",
                p=p,
            )
            self.assertEqual(result.module_id, "firecrawl")
            self.assertEqual(result.tool, "fetch_url")
            self.assertEqual(result.outputs, "stub model output")
            self.assertGreater(result.run_id, 0)

            # Verify module_runs row
            async with self.pool.acquire() as conn:
                row = await conn.fetchrow(
                    "SELECT surface, tool_name, duration_ms FROM module_runs WHERE id = $1",
                    result.run_id,
                )
            self.assertEqual(row["surface"], "tui")
            self.assertEqual(row["tool_name"], "fetch_url")
        finally:
            modules_run.httpx.AsyncClient = original
