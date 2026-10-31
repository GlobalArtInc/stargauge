use chrono::NaiveDate;
use console::style;
use std::io::{self, Write};

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const AXIS_RESERVE: usize = 10;

pub fn bars(
    out: &mut dyn Write,
    title: &str,
    rows: &[(String, u64)],
    width: usize,
) -> io::Result<()> {
    writeln!(out, "{}", style(title).bold())?;
    if rows.is_empty() {
        return writeln!(out, "(no data)");
    }
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let number_width = rows
        .iter()
        .map(|(_, n)| n.to_string().len())
        .max()
        .unwrap_or(1);
    let max = rows.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let avail = width.saturating_sub(label_width + number_width + 3);
    for (label, n) in rows {
        let len = bar_len(*n, max, avail);
        writeln!(out, "{label:<label_width$}  {} {n}", "█".repeat(len))?;
    }
    Ok(())
}

fn bar_len(value: u64, max: u64, avail: usize) -> usize {
    if value == 0 || max == 0 || avail == 0 {
        return 0;
    }
    ((value as f64 / max as f64 * avail as f64).round() as usize).clamp(1, avail)
}

pub fn columns(
    out: &mut dyn Write,
    title: &str,
    series: &[(NaiveDate, u32)],
    width: usize,
    height: usize,
) -> io::Result<()> {
    writeln!(out, "{}", style(title).bold())?;
    let (Some(first), Some(last)) = (series.first(), series.last()) else {
        return writeln!(out, "(no data)");
    };
    let cols = width.saturating_sub(AXIS_RESERVE).max(1);
    let height = height.max(1);
    let buckets = bucket(series, cols);
    let top = buckets.iter().copied().max().unwrap_or(0);
    let top_label = top.to_string();
    let label_width = top_label.len();
    let scale = (height * 8) as f64;
    let levels: Vec<usize> = buckets
        .iter()
        .map(|&v| {
            if top == 0 {
                0
            } else {
                (v as f64 * scale / top as f64).round() as usize
            }
        })
        .collect();

    for row in (0..height).rev() {
        let base = row * 8;
        let label = if row == height - 1 {
            top_label.clone()
        } else if row == 0 {
            "0".to_string()
        } else {
            String::new()
        };
        let cells: String = levels
            .iter()
            .map(|&level| match level.saturating_sub(base).min(8) {
                0 => ' ',
                fill => BLOCKS[fill - 1],
            })
            .collect();
        writeln!(out, "{label:>label_width$} │{cells}")?;
    }

    writeln!(out, "{:>label_width$} └{}", "", "─".repeat(buckets.len()))?;

    let first = first.0.format("%Y-%m-%d").to_string();
    let last = last.0.format("%Y-%m-%d").to_string();
    let gap = buckets
        .len()
        .saturating_sub(first.len() + last.len())
        .max(1);
    writeln!(
        out,
        "{}{first}{}{last}",
        " ".repeat(label_width + 2),
        " ".repeat(gap)
    )
}

fn bucket(series: &[(NaiveDate, u32)], cols: usize) -> Vec<u64> {
    let n = series.len();
    if n <= cols {
        return series.iter().map(|&(_, v)| u64::from(v)).collect();
    }
    (0..cols)
        .map(|i| {
            let start = i * n / cols;
            let end = (i + 1) * n / cols;
            series[start..end]
                .iter()
                .map(|&(_, v)| u64::from(v))
                .sum::<u64>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_render_three_known_points() {
        let series = [
            (NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), 2),
            (NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(), 4),
            (NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(), 8),
        ];
        let mut buf = Vec::new();
        columns(&mut buf, "ALL", &series, 13, 2).unwrap();
        let text = console::strip_ansi_codes(&String::from_utf8(buf).unwrap()).to_string();
        assert_eq!(
            text,
            "ALL\n8 │  █\n0 │▄██\n  └───\n   2026-01-01 2026-01-03\n"
        );
    }
}
