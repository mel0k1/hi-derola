pub struct Row {
    pub tag: u8, // 0 ctx, 1 add, 2 del
    pub text: String,
}

const CONTEXT: usize = 2;
const MAX_ROWS: usize = 40;
const MAX_CELLS: usize = 2_000_000;

fn ellipsis(mut rows: Vec<Row>) -> Vec<Row> {
    if rows.len() > MAX_ROWS {
        rows.truncate(MAX_ROWS);
        rows.push(Row {
            tag: 0,
            text: "...".into(),
        });
    }
    rows
}

pub fn lines_diff(a: &str, b: &str) -> Vec<Row> {
    let av: Vec<&str> = a.lines().collect();
    let bv: Vec<&str> = b.lines().collect();
    let (n, m) = (av.len(), bv.len());
    let mut ops: Vec<(u8, &str)> = Vec::new();
    if n.saturating_mul(m) <= MAX_CELLS {
        let stride = m + 1;
        let mut dp = vec![0u32; (n + 1) * stride];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * stride + j] = if av[i] == bv[j] {
                    dp[(i + 1) * stride + j + 1] + 1
                } else {
                    dp[(i + 1) * stride + j].max(dp[i * stride + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < n && j < m {
            if av[i] == bv[j] {
                ops.push((0, av[i]));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * stride + j] >= dp[i * stride + j + 1] {
                ops.push((2, av[i]));
                i += 1;
            } else {
                ops.push((1, bv[j]));
                j += 1;
            }
        }
        while i < n {
            ops.push((2, av[i]));
            i += 1;
        }
        while j < m {
            ops.push((1, bv[j]));
            j += 1;
        }
    } else {
        for l in &av {
            ops.push((2, l));
        }
        for l in &bv {
            ops.push((1, l));
        }
    }
    let len = ops.len();
    let mut keep = vec![false; len];
    for k in 0..len {
        if ops[k].0 != 0 {
            let lo = k.saturating_sub(CONTEXT);
            let hi = (k + CONTEXT + 1).min(len);
            for t in lo..hi {
                keep[t] = true;
            }
        }
    }
    let mut rows = Vec::new();
    let mut skipping = false;
    for k in 0..len {
        if keep[k] {
            skipping = false;
            rows.push(Row {
                tag: ops[k].0,
                text: ops[k].1.to_string(),
            });
        } else if !skipping {
            skipping = true;
            rows.push(Row {
                tag: 0,
                text: "...".into(),
            });
        }
    }
    ellipsis(rows)
}

pub fn preview_write(old: Option<&str>, new: &str) -> Vec<Row> {
    match old {
        Some(o) => lines_diff(o, new),
        None => ellipsis(
            new.lines()
                .map(|l| Row {
                    tag: 1,
                    text: l.to_string(),
                })
                .collect(),
        ),
    }
}
