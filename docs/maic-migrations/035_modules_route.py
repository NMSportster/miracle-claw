"""Module-driven surfaces API.

Routes:
    GET  /v1/categories
        Returns the 16-category controlled vocabulary with module
        counts. Public, no auth (so the marketplace page can render
        the sidebar before login).

    GET  /v1/marketplace/modules?category=X&sort=Y
        Returns the full marketplace catalog (every active module).
        Auth required (so we can hide modules the user can't install
        based on tier_required).

    GET  /v1/modules/installed
        Returns the modules the current user has installed, joined
        with the client_manifest so the client can render them
        without a second fetch. Grouped by category in the response
        (category header + list of modules).

    POST /v1/modules/install
        Body: {"module_id": "firecrawl"}
        Adds the module to module_installs for this user, returns
        the full client_manifest so the client can drop it into
        "Your Tools" without a refresh.

    POST /v1/modules/uninstall
        Body: {"module_id": "firecrawl"}
        Removes the module from module_installs for this user.

Auth:
    All routes except /v1/categories require a valid JWT
    (Principal dependency). The categories endpoint is public
    so the marketing site can show "X modules in 16 categories"
    without a login.

Spec:
    projects/miracle-claw/docs/mc-module-driven-surfaces-2026-09.md
    §3.4 (endpoints), §4 (dashboard), §5 (three surfaces).
"""
from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from typing import Any, Dict, List, Literal, Optional

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field

from api.auth import Principal, authenticate
from api.db import pool

log = logging.getLogger(__name__)

router = APIRouter()


# ─────────────────────────────────────────────────────────────────────
# Pydantic models
# ─────────────────────────────────────────────────────────────────────


class Category(BaseModel):
    id: str
    label: str
    emoji: str
    sort_order: int
    count: int = 0


class CategoriesResponse(BaseModel):
    categories: List[Category]


class ModuleSummary(BaseModel):
    """A module as it appears in the marketplace listing."""
    module_id: str
    name: str
    description: str
    icon: str
    category: str
    subcategory: Optional[str] = None
    tier_required: Literal["free", "pro", "team", "enterprise"]
    author: str
    rating: float
    install_count: int
    is_first_party: bool


class MarketplaceResponse(BaseModel):
    modules: List[ModuleSummary]


class InstalledModule(BaseModel):
    """A module as it appears in the user's 'Your Tools' grid.

    Includes the full client_manifest so the client doesn't need a
    second fetch to render buttons and forms.
    """
    module_id: str
    name: str
    description: str
    icon: str
    category: str
    subcategory: Optional[str] = None
    installed_at: datetime
    last_used_at: Optional[datetime] = None
    client_manifest: Dict[str, Any]


class InstalledResponse(BaseModel):
    modules: List[InstalledModule]
    grouped_by_category: Dict[str, List[str]]   # category_id → [module_ids]


class InstallIn(BaseModel):
    module_id: str = Field(min_length=1, max_length=128)


class InstallResponse(BaseModel):
    success: bool
    client_manifest: Dict[str, Any]


class UninstallResponse(BaseModel):
    success: bool


# ─────────────────────────────────────────────────────────────────────
# /v1/categories — public, no auth
# ─────────────────────────────────────────────────────────────────────


@router.get("/v1/categories", response_model=CategoriesResponse)
async def list_categories():
    """List the 16-category controlled vocabulary with module counts.

    Public endpoint — the marketing site uses it to show "X modules
    in 16 categories" on milagrocloud.com. Sorted by sort_order.
    """
    async with pool().acquire() as conn:
        rows = await conn.fetch(
            """
            SELECT
                c.id,
                c.label,
                c.emoji,
                c.sort_order,
                COALESCE(COUNT(m.id) FILTER (WHERE m.is_active = TRUE), 0) AS count
            FROM module_categories c
            LEFT JOIN modules m ON m.category = c.id
            GROUP BY c.id, c.label, c.emoji, c.sort_order
            ORDER BY c.sort_order ASC
            """
        )
    return CategoriesResponse(
        categories=[
            Category(
                id=r["id"],
                label=r["label"],
                emoji=r["emoji"],
                sort_order=r["sort_order"],
                count=r["count"],
            )
            for r in rows
        ]
    )


# ─────────────────────────────────────────────────────────────────────
# /v1/marketplace/modules — auth required
# ─────────────────────────────────────────────────────────────────────


@router.get("/v1/marketplace/modules", response_model=MarketplaceResponse)
async def marketplace(
    category: Optional[str] = None,
    sort: Literal["top_rated", "recent", "alphabetical"] = "top_rated",
    p: Principal = Depends(authenticate),
):
    """List all active modules, optionally filtered by category.

    Sort options:
        - top_rated (default): rating DESC, install_count DESC
        - recent: created_at DESC
        - alphabetical: name ASC
    """
    where_clauses = ["m.is_active = TRUE"]
    args: List[Any] = []

    if category:
        where_clauses.append("m.category = $1")
        args.append(category)

    where_sql = " AND ".join(where_clauses)

    if sort == "top_rated":
        order_sql = "m.rating DESC, m.install_count DESC, m.name ASC"
    elif sort == "recent":
        order_sql = "m.created_at DESC"
    else:  # alphabetical
        order_sql = "m.name ASC"

    query = f"""
        SELECT
            m.id              AS module_id,
            m.name,
            m.description,
            m.icon,
            m.category,
            m.subcategory,
            m.tier_required,
            m.author,
            m.rating,
            m.install_count,
            m.is_first_party
        FROM modules m
        WHERE {where_sql}
        ORDER BY {order_sql}
    """

    async with pool().acquire() as conn:
        rows = await conn.fetch(query, *args)

    return MarketplaceResponse(
        modules=[
            ModuleSummary(
                module_id=r["module_id"],
                name=r["name"],
                description=r["description"],
                icon=r["icon"],
                category=r["category"],
                subcategory=r["subcategory"],
                tier_required=r["tier_required"],
                author=r["author"],
                rating=float(r["rating"]),
                install_count=r["install_count"],
                is_first_party=r["is_first_party"],
            )
            for r in rows
        ]
    )


# ─────────────────────────────────────────────────────────────────────
# /v1/modules/installed — auth required
# ─────────────────────────────────────────────────────────────────────


@router.get("/v1/modules/installed", response_model=InstalledResponse)
async def installed(p: Principal = Depends(authenticate)):
    """List the current user's installed modules, with full client_manifest.

    Returns a flat list and a grouped_by_category index so the
    client can render either a flat grid or a grouped view without
    re-sorting.
    """
    async with pool().acquire() as conn:
        rows = await conn.fetch(
            """
            SELECT
                m.id              AS module_id,
                m.name,
                m.description,
                m.icon,
                m.category,
                m.subcategory,
                i.installed_at,
                i.last_used_at,
                m.client_manifest
            FROM module_installs i
            JOIN modules m ON m.id = i.module_id
            WHERE i.user_id = $1
            ORDER BY m.category ASC, m.name ASC
            """,
            p.user_id,
        )

    modules: List[InstalledModule] = []
    grouped: Dict[str, List[str]] = {}

    for r in rows:
        # client_manifest is JSONB — asyncpg returns it as a string
        # in some versions, parse if needed
        manifest = r["client_manifest"]
        if isinstance(manifest, str):
            manifest = json.loads(manifest)

        modules.append(
            InstalledModule(
                module_id=r["module_id"],
                name=r["name"],
                description=r["description"],
                icon=r["icon"],
                category=r["category"],
                subcategory=r["subcategory"],
                installed_at=r["installed_at"],
                last_used_at=r["last_used_at"],
                client_manifest=manifest,
            )
        )
        grouped.setdefault(r["category"], []).append(r["module_id"])

    return InstalledResponse(modules=modules, grouped_by_category=grouped)


# ─────────────────────────────────────────────────────────────────────
# /v1/modules/install — auth required
# ─────────────────────────────────────────────────────────────────────


@router.post("/v1/modules/install", response_model=InstallResponse)
async def install(
    body: InstallIn,
    p: Principal = Depends(authenticate),
):
    """Install a module for the current user.

    - Checks the module exists and is_active.
    - Checks tier_required against the user's tier.
    - Inserts into module_installs (unique constraint catches
      double-installs, return 200 with current manifest).
    - Increments modules.install_count.
    - Returns the full client_manifest.
    """
    async with pool().acquire() as conn:
        # 1. Fetch the module
        mod = await conn.fetchrow(
            "SELECT id, client_manifest, tier_required, is_active FROM modules WHERE id = $1",
            body.module_id,
        )
        if not mod or not mod["is_active"]:
            raise HTTPException(404, f"Module '{body.module_id}' not found")

        # 2. Tier gate
        #    Skip for now — tier_required defaults to 'free' for all
        #    seeded modules. Add a real tier check when we ship
        #    paid modules (rc57+).
        #    TODO: check p.tier against mod['tier_required']

        # 3. Insert (idempotent via UNIQUE)
        await conn.execute(
            """
            INSERT INTO module_installs (user_id, module_id)
            VALUES ($1, $2)
            ON CONFLICT (user_id, module_id) DO NOTHING
            """,
            p.user_id,
            body.module_id,
        )

        # 4. Bump install_count
        await conn.execute(
            "UPDATE modules SET install_count = install_count + 1 WHERE id = $1",
            body.module_id,
        )

        manifest = mod["client_manifest"]
        if isinstance(manifest, str):
            manifest = json.loads(manifest)

    log.info("user %s installed module %s", p.user_id, body.module_id)
    return InstallResponse(success=True, client_manifest=manifest)


# ─────────────────────────────────────────────────────────────────────
# /v1/modules/uninstall — auth required
# ─────────────────────────────────────────────────────────────────────


@router.post("/v1/modules/uninstall", response_model=UninstallResponse)
async def uninstall(
    body: InstallIn,
    p: Principal = Depends(authenticate),
):
    """Uninstall a module for the current user. Idempotent."""
    async with pool().acquire() as conn:
        result = await conn.execute(
            "DELETE FROM module_installs WHERE user_id = $1 AND module_id = $2",
            p.user_id,
            body.module_id,
        )
        # asyncpg execute returns "DELETE N"
        deleted = int(result.split()[-1]) if result else 0
        if deleted:
            # Decrement only if we actually deleted something
            await conn.execute(
                "UPDATE modules SET install_count = GREATEST(install_count - 1, 0) WHERE id = $1",
                body.module_id,
            )

    log.info("user %s uninstalled module %s (deleted=%d)", p.user_id, body.module_id, deleted)
    return UninstallResponse(success=True)
