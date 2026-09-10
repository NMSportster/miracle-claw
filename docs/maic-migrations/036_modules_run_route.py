"""Module run endpoint for Week 3 (mc-terminal TUI + Tauri launchpad).

Routes:
    POST /v1/modules/{module_id}/run
        Body: {"tool": "fetch_url", "inputs": {"url": "https://..."}, "surface": "tui"}
        Validates the user has the module installed, dispatches via
        LiteLLM with a synthetic prompt that includes the module
        context, logs the run to module_runs with the surface tag,
        and returns the model's response.

        v1 dispatch: the model IS the executor. The prompt is shaped
        so the model knows which tool to invoke, what inputs it
        received, and what the expected output shape is. This is
        intentionally simple — the first-party modules we seed
        (email campaigns, firecrawl, social posts) are all
        LLM-doable. When we add non-LLM modules (HTTP fetchers,
        cron schedulers, etc.) this handler will grow a tool
        dispatcher table.

Auth: requires a valid JWT (Principal dependency).

Spec: projects/miracle-claw/docs/mc-module-driven-surfaces-2026-09.md
      §3.5 (install flow), §5.2 (TUI), §5.3 (mclaw CLI — same path).
      Also closes the "module invocation TODO" in
      projects/miracle-claw/src/pages/dashboard_launchpad.js.
"""
from __future__ import annotations

import json
import logging
import time
from typing import Any, Dict, Literal

import httpx
from fastapi import APIRouter, Depends, HTTPException, Path
from pydantic import BaseModel, Field

from api.auth import Principal, authenticate
from api.config import get_settings
from api.db import pool

log = logging.getLogger(__name__)

router = APIRouter()


# ─────────────────────────────────────────────────────────────────────
# Pydantic models
# ─────────────────────────────────────────────────────────────────────


class RunIn(BaseModel):
    tool: str = Field(min_length=1, max_length=128)
    inputs: Dict[str, Any] = Field(default_factory=dict)
    surface: Literal["tauri", "tui", "mclaw"] = "tui"


class RunResponse(BaseModel):
    outputs: str                       # the model's text response
    duration_ms: int
    module_id: str
    tool: str
    run_id: int                        # primary key in module_runs


# ─────────────────────────────────────────────────────────────────────
# /v1/modules/{module_id}/run — auth required
# ─────────────────────────────────────────────────────────────────────


@router.post(
    "/v1/modules/{module_id}/run",
    response_model=RunResponse,
)
async def run_module(
    body: RunIn,
    module_id: str = Path(min_length=1, max_length=128),
    p: Principal = Depends(authenticate),
):
    """Run a tool on a first-party module.

    For v1, dispatch is via LiteLLM with a synthetic prompt. This is
    intentionally simple — first-party modules are all LLM-doable
    (content generation, light scraping, social posts). When we add
    non-LLM tools, this route grows a tool dispatcher.
    """
    t0 = time.monotonic()

    async with pool().acquire() as conn:
        # 1. Module must exist + be active + be first-party (v1)
        module_row = await conn.fetchrow(
            """
            SELECT id, name, description, client_manifest, tier_required,
                   is_active, is_first_party
            FROM modules WHERE id = $1
            """,
            module_id,
        )
        if not module_row or not module_row["is_active"]:
            raise HTTPException(
                status_code=404, detail=f"module {module_id!r} not found"
            )
        if not module_row["is_first_party"]:
            raise HTTPException(
                status_code=400,
                detail=f"module {module_id!r} is not first-party (no v1 dispatcher)",
            )

        # 2. User must have the module installed
        install_row = await conn.fetchrow(
            "SELECT 1 FROM module_installs WHERE user_id = $1 AND module_id = $2",
            p.user_id, module_id,
        )
        if not install_row:
            raise HTTPException(
                status_code=403,
                detail=f"module {module_id!r} is not installed for this user",
            )

        # 3. The requested tool must exist in the client_manifest
        manifest = module_row["client_manifest"] or {}
        # asyncpg returns JSONB as a string in some versions; parse if so.
        if isinstance(manifest, str):
            manifest = json.loads(manifest)
        tools = manifest.get("tools", [])
        tool_def = next((t for t in tools if t.get("name") == body.tool), None)
        if tool_def is None:
            available = [t.get("name") for t in tools]
            raise HTTPException(
                status_code=404,
                detail=f"tool {body.tool!r} not in module {module_id!r}. available: {available}",
            )

        # 4. Build the synthetic prompt and dispatch to LiteLLM
        description = tool_def.get("description", "(no description)")
        prompt_lines = [
            f"You are invoking the `{body.tool}` tool from the `{module_id}` module.",
            f"Module description: {manifest.get('description', '(none)')}",
            f"Tool description: {description}",
            "",
            "Inputs (JSON):",
            json.dumps(body.inputs, indent=2, sort_keys=True),
            "",
            "Return your response as plain text. If the tool produces structured",
            "output, format it as a markdown code block. Do not include any",
            "preamble or explanation — just the response.",
        ]
        synthetic_messages = [
            {"role": "system", "content":
                "You are the executor for a Miracle Claw module. "
                "Follow the user's tool spec exactly. "
                "Return only the tool's output."},
            {"role": "user", "content": "\n".join(prompt_lines)},
        ]

        s = get_settings()
        litellm_url = s.litellm_base_url
        litellm_key = s.litellm_master_key
        async with httpx.AsyncClient(timeout=60.0) as http:
            resp = await http.post(
                f"{litellm_url}/v1/chat/completions",
                headers={"Authorization": f"Bearer {litellm_key}"},
                json={
                    "model": "milagro-oc-minimax",
                    "messages": synthetic_messages,
                    "user": p.name,
                },
            )
            resp.raise_for_status()
            data = resp.json()
            try:
                model_response = data["choices"][0]["message"]["content"]
            except (KeyError, IndexError, TypeError) as e:
                log.error("malformed litellm response: %s", data)
                raise HTTPException(
                    status_code=502,
                    detail=f"malformed response from upstream model: {e}",
                )

    duration_ms = int((time.monotonic() - t0) * 1000)

    # 5. Log to module_runs + bump last_used_at
    async with pool().acquire() as conn:
        run_id = await conn.fetchval(
            """
            INSERT INTO module_runs
                (user_id, module_id, tool_name, inputs, outputs, duration_ms, surface)
            VALUES ($1, $2, $3, $4::jsonb, $5::jsonb, $6, $7)
            RETURNING id
            """,
            p.user_id,
            module_id,
            body.tool,
            json.dumps(body.inputs),
            json.dumps({"text": model_response}),
            duration_ms,
            body.surface,
        )
        await conn.execute(
            "UPDATE module_installs SET last_used_at = now() "
            "WHERE user_id = $1 AND module_id = $2",
            p.user_id, module_id,
        )

    log.info(
        "user %s ran %s.%s via %s in %dms (run_id=%d)",
        p.user_id, module_id, body.tool, body.surface, duration_ms, run_id,
    )

    return RunResponse(
        outputs=model_response,
        duration_ms=duration_ms,
        module_id=module_id,
        tool=body.tool,
        run_id=run_id,
    )
