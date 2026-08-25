// src/pages/pricing.js — In-app Plans & Pricing page (Lesson 564, 2026-08-24 17:30 MDT)
//
// David wanted a small button on the Dashboard that takes the user to
// a dedicated pricing page inside MC — not a 6-card grid crammed into
// the dashboard, not a redirect to milagrocloud.com/pricing in the OS
// browser. This page IS the in-app pricing page.
//
// Architecture (mirrors `extras.js` and `secrets.js`):
//   - Register: `register("pricing", pricingPage)` in main.js
//   - Navigation builder: `["pricing", () => pricingCtx()]` in
//     installNavigation() so the dashboard's "Plans & Pricing →"
//     button can `navigate("pricing")`.
//   - Page contract: mount(root, ctx), requiresAuth: true,
//     onBackToDashboard wired in pricingCtx().
//
// Data source: `mc_list_plans` returns the same 6 plans as
// `/v1/billing/plans` on MAIC (Free, Starter, Starter Plus, Pro,
// Pro Plus, Team). We never hardcode the list — the source of truth
// lives on MAIC so this page stays in sync the moment a plan is
// renamed or repriced.
//
// Stripe Checkout flow: each paid-plan CTA calls
// `mc_open_checkout_url(plan_code)`, which POSTs to
// `/v1/billing/checkout` with the user's JWT and opens the returned
// Stripe URL in the OS default browser. On success, Stripe redirects
// to /welcome?plan=<code> on milagrocloud.com (handled by Lesson 553's
// /welcome page). The web pricing page and this in-app page share the
// same plan data + same Stripe flow; both end up on the same success
// page.
//
// Why a separate page vs. an in-dashboard card (Lesson 564)?
//   - Dashboard should be calm: just usage, big tiles, key actions.
//     A 6-card pricing grid was visual clutter.
//   - The web /pricing page already has the full marketing context
//     (FAQ, comparison table, testimonials). We don't need to
//     replicate that here.
//   - But David still wanted one-click access from inside MC for the
//     common "I want to upgrade" flow — so a thin pricing page that
//     shows the live plans + Stripe CTAs lives here.

import { invoke } from "@tauri-apps/api/core";

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));
}

function formatNumber(n) {
  if (n == null) return "—";
  return Number(n).toLocaleString("en-US");
}

export const pricingPage = {
  label: "Plans & Pricing",
  icon: "💳",
  requiresAuth: true,

  async mount(root, ctx = {}) {
    root.innerHTML = `
      <section class="pricing-page" aria-label="Plans and pricing">
        <header class="pricing-header">
          <button class="icon-link" id="pricing-back" title="Back to dashboard" aria-label="Back">←</button>
          <h1 class="pricing-title">💳 Plans &amp; Pricing</h1>
          <span class="badge-ok pricing-badge" title="Pricing pulled live from MAIC — same source as milagrocloud.com/pricing">
            Live from MAIC
          </span>
        </header>

        <p class="pricing-intro muted">
          Pick the plan that fits. Pay in your browser on Stripe — your dashboard
          stays right here. On success you'll land on the welcome page with
          everything configured.
        </p>

        <div class="pricing-grid" id="pricing-grid">
          <p class="muted">Loading plans…</p>
        </div>

        <footer class="pricing-footer muted small">
          <p>
            Questions? Email
            <a href="mailto:support@milagrocloud.com">support@milagrocloud.com</a>
            or check <a href="https://milagrocloud.com/pricing" target="_blank" rel="noopener">milagrocloud.com/pricing</a>
            for the full marketing page.
          </p>
        </footer>
      </section>
    `;

    const back = root.querySelector("#pricing-back");
    if (back && typeof ctx.onBackToDashboard === "function") {
      back.addEventListener("click", ctx.onBackToDashboard);
    }

    // Load plans + tier in parallel. Tier is used to highlight the
    // user's current plan; plans come from /v1/billing/plans.
    const [plans, tier] = await Promise.all([
      invoke("mc_list_plans").catch((err) => {
        console.warn("[pricing] mc_list_plans failed:", err);
        return [];
      }),
      invoke("mc_get_tier").catch((err) => {
        console.warn("[pricing] mc_get_tier failed:", err);
        return null;
      }),
    ]);

    const grid = root.querySelector("#pricing-grid");
    if (grid) {
      if (Array.isArray(plans) && plans.length > 0) {
        const currentPlanCode = tier?.plan_code || tier?.tier || "free";
        grid.innerHTML = plans.map((p) => renderPlanCard(p, currentPlanCode)).join("");
        wireCardButtons(grid);
      } else {
        // MAIC unreachable / no plans — friendly fallback.
        grid.innerHTML = `
          <div class="pricing-empty">
            <p class="muted">Couldn't load plans right now.</p>
            <p class="muted small">
              Check your connection or visit
              <a href="https://milagrocloud.com/pricing" target="_blank" rel="noopener">milagrocloud.com/pricing</a>
              in your browser.
            </p>
          </div>`;
      }
    }
  },

  unmount() {
    // No long-lived listeners here. Buttons are scoped to the grid
    // and torn down with the DOM. If we add a "compare plans" modal
    // in v2, mount a cleanup hook here.
  },
};

// ============================================================================
// Card rendering
// ============================================================================

function renderPlanCard(plan, currentPlanCode) {
  const isCurrent = (plan.code || "").toLowerCase() === (currentPlanCode || "").toLowerCase();
  const isFree = plan.code === "free";
  const dollars = (plan.monthly_cents / 100).toFixed(0);
  const priceLabel = plan.monthly_cents === 0 ? "$0" : `$${dollars}`;
  const featured = plan.code === "pro"; // "Most popular" — matches the web pricing page
  const ctaLabel = isCurrent ? "Current plan" : (isFree ? "Always free" : `Choose ${escapeHtml(plan.name)}`);
  const ctaDisabled = isCurrent || isFree;

  // Per-plan bullets. The seats badge mirrors what the web pricing
  // page shows so the two surfaces stay consistent.
  const seats = plan.code === "pro_plus" ? "5 seats included"
              : plan.code === "team"     ? "8 seats included"
              : null;

  return `
    <article class="plan-card ${featured ? "featured" : ""} ${isCurrent ? "current" : ""}" data-plan="${escapeHtml(plan.code)}">
      ${featured ? `<div class="plan-badge">Most popular</div>` : ""}
      ${isCurrent ? `<div class="plan-badge plan-badge-current">Your plan</div>` : ""}
      <div class="plan-name">${escapeHtml(plan.name)}</div>
      <div class="plan-price">${priceLabel}<span class="plan-price-suffix">/mo</span></div>
      <p class="plan-tagline muted small">${escapeHtml(plan.tagline || "")}</p>
      <ul class="plan-features">
        <li><strong>${formatNumber(plan.included_tokens)}</strong> tokens / mo</li>
        <li><strong>${formatNumber(plan.rpm)}</strong> requests / min</li>
        ${plan.overage_per_1k > 0
          ? `<li>$${(plan.overage_per_1k / 100).toFixed(2)} per 1K overage</li>`
          : `<li>No overage charges</li>`}
        ${seats ? `<li>${escapeHtml(seats)}</li>` : ""}
      </ul>
      <button type="button"
              class="plan-cta ${isCurrent ? "is-current" : ""} ${featured ? "is-featured" : ""}"
              data-plan="${escapeHtml(plan.code)}"
              ${ctaDisabled ? "disabled" : ""}>
        ${escapeHtml(ctaLabel)}
      </button>
    </article>
  `;
}

function wireCardButtons(grid) {
  grid.querySelectorAll(".plan-cta:not([disabled])").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const planCode = btn.dataset.plan;
      if (!planCode) return;
      const originalLabel = btn.textContent;
      btn.disabled = true;
      btn.textContent = "Opening Stripe…";
      try {
        await invoke("mc_open_checkout_url", { planCode });
        // Stripe opens in the user's default browser. They pay there
        // and return to /welcome?plan=<code>. We DON'T navigate away
        // from this page — the user may want to keep the pricing
        // context open while paying.
      } catch (err) {
        // Restore button so the user can retry.
        btn.disabled = false;
        btn.textContent = originalLabel;
        // Surface the error visibly. Pricing is high-intent, so a
        // silent failure is worse than a noisy one.
        alert(`Couldn't open Stripe checkout: ${err}`);
      }
    });
  });
}