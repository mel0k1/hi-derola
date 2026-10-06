//! Session usage ledger — one [`UsageRow`] per api request, aggregated per
//! model for `/usage`.
//!
//! Rows live with the session (`StoredSession.usage`), so the breakdown
//! survives a resume. Cost per row is the same `models::cost_cached` math the
//! status bars use (cached tokens at the provider's cache discount), recorded
//! at request time with the model that actually served it — switching models
//! mid-session keeps each request priced correctly.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageRow {
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cost: f64,
}

pub struct ModelTotals {
    pub model: String,
    pub requests: usize,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cost: f64,
}

/// per-model totals in first-seen order
pub fn aggregate(rows: &[UsageRow]) -> Vec<ModelTotals> {
    let mut out: Vec<ModelTotals> = Vec::new();
    for r in rows {
        match out.iter_mut().find(|t| t.model == r.model) {
            Some(t) => {
                t.requests += 1;
                t.input += r.input;
                t.output += r.output;
                t.cached += r.cached;
                t.cost += r.cost;
            }
            None => out.push(ModelTotals {
                model: r.model.clone(),
                requests: 1,
                input: r.input,
                output: r.output,
                cached: r.cached,
                cost: r.cost,
            }),
        }
    }
    out
}

/// (requests, tokens in, tokens out, tokens cached, cost) over all rows
pub fn totals(rows: &[UsageRow]) -> (usize, u64, u64, u64, f64) {
    let mut t = (0usize, 0u64, 0u64, 0u64, 0.0f64);
    for r in rows {
        t.0 += 1;
        t.1 += r.input;
        t.2 += r.output;
        t.3 += r.cached;
        t.4 += r.cost;
    }
    t
}

/// context window used by the last request (prompt + completion)
pub fn last_ctx_used(rows: &[UsageRow]) -> u64 {
    rows.last().map(|r| r.input + r.output).unwrap_or(0)
}

pub fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

pub fn fmt_cost(c: f64) -> String {
    if c >= 1.0 {
        format!("${c:.2}")
    } else {
        format!("${c:.4}")
    }
}

/// The `/usage` report shared by the TUI and the GUI.
///
/// `ctx_limit` is the effective context window (config override or the model
/// catalog window); 0 hides the context line.
pub fn render(rows: &[UsageRow], model: &str, kind: &str, ctx_limit: u64) -> String {
    let (reqs, tin, tout, tcached, cost) = totals(rows);
    if reqs == 0 {
        return "usage: no api requests this session".into();
    }
    let disc = if kind == "anthropic" { 0.1 } else { 0.5 };
    let disc_pct = format!("{}%", (disc * 100.0) as u64);

    let mut out = String::new();
    out.push_str(&format!(
        "usage — {} request{} · {} in · {} out ({} cached) · {}\n",
        reqs,
        if reqs == 1 { "" } else { "s" },
        fmt_tokens(tin),
        fmt_tokens(tout),
        fmt_tokens(tcached),
        fmt_cost(cost)
    ));
    out.push_str("\n  model                       req   in        out       cached    cost");
    for t in aggregate(rows) {
        out.push_str(&format!(
            "\n  {:<26} {:>3}   {:<9} {:<9} {:<9} {}",
            truncate(&t.model, 26),
            t.requests,
            fmt_tokens(t.input),
            fmt_tokens(t.output),
            fmt_tokens(t.cached),
            fmt_cost(t.cost)
        ));
    }
    let mi = crate::models::lookup(model);
    out.push_str(&format!(
        "\n\nprices ({}): ${:.2}/M in · ${:.2}/M out · cached reads {}\n",
        truncate(model, 40),
        mi.input,
        mi.output,
        disc_pct
    ));
    if ctx_limit > 0 {
        let used = last_ctx_used(rows);
        let pct = (used as f64 / ctx_limit as f64 * 100.0).round() as u64;
        out.push_str(&format!(
            "context: last request {} / {} tokens ({}%) — /compact shrinks it",
            fmt_tokens(used),
            fmt_tokens(ctx_limit),
            pct
        ));
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(model: &str, input: u64, output: u64, cached: u64, cost: f64) -> UsageRow {
        UsageRow {
            model: model.into(),
            input,
            output,
            cached,
            cost,
        }
    }

    #[test]
    fn aggregate_keeps_first_seen_order_and_sums() {
        let rows = vec![
            row("a", 100, 10, 0, 0.5),
            row("b", 200, 20, 50, 1.0),
            row("a", 300, 30, 25, 0.25),
        ];
        let agg = aggregate(&rows);
        assert_eq!(agg.len(), 2);
        assert_eq!(agg[0].model, "a");
        assert_eq!(agg[0].requests, 2);
        assert_eq!(agg[0].input, 400);
        assert_eq!(agg[0].output, 40);
        assert_eq!(agg[0].cached, 25);
        assert!((agg[0].cost - 0.75).abs() < 1e-9);
        assert_eq!(agg[1].model, "b");
        assert_eq!(agg[1].requests, 1);
    }

    #[test]
    fn totals_and_empty_rows() {
        assert_eq!(totals(&[]), (0, 0, 0, 0, 0.0));
        assert_eq!(aggregate(&[]).len(), 0);
        assert_eq!(last_ctx_used(&[]), 0);
        let rows = vec![row("a", 1000, 100, 0, 0.1), row("a", 2000, 200, 500, 0.2)];
        let t = totals(&rows);
        assert_eq!((t.0, t.1, t.2, t.3), (2, 3000, 300, 500));
        assert!((t.4 - 0.3).abs() < 1e-9);
        assert_eq!(last_ctx_used(&rows), 2200);
    }

    #[test]
    fn render_empty_session() {
        assert_eq!(
            render(&[], "gpt-5", "openai", 400_000),
            "usage: no api requests this session"
        );
    }

    #[test]
    fn render_shows_breakdown_prices_context() {
        let rows = vec![
            row("gpt-5", 150_000, 2_000, 100_000, 0.1912),
            row("gpt-5", 50_000, 1_000, 0, 0.0725),
        ];
        let s = render(&rows, "gpt-5", "openai", 400_000);
        assert!(s.contains("usage — 2 requests · 200.0k in · 3.0k out (100.0k cached) · $0.2637"));
        assert!(s.contains("gpt-5"));
        assert!(s.contains("prices (gpt-5): $1.25/M in · $10.00/M out · cached reads 50%"));
        assert!(s.contains("context: last request 51.0k / 400.0k tokens (13%)"));
        // anthropic discount is 10%
        let s2 = render(&rows, "claude-sonnet-4-5", "anthropic", 200_000);
        assert!(s2.contains("cached reads 10%"));
    }
}
