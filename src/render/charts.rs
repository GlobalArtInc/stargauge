use crate::model::Stats;
use askama::Template;
use chrono::NaiveDate;
use serde_json::{Value, json};

pub static ECHARTS_JS: &[u8] = include_bytes!("../../assets/echarts.min.js");

const STAR: &str = "path://M10.00,0.00 L12.47,6.60 L19.51,6.91 L13.99,11.30 L15.88,18.09 L10.00,14.20 L4.12,18.09 L6.01,11.30 L0.49,6.91 L7.53,6.60 Z";
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

#[derive(Template)]
#[template(path = "graphs.html")]
struct GraphsPage<'a> {
    repo: &'a str,
    has_hour: bool,
    data: String,
}

#[derive(Template)]
#[template(path = "gone_graph.html")]
struct GoneGraphPage<'a> {
    repo: &'a str,
    data: String,
}

pub fn daily_chart(repo: &str, daily: &[(NaiveDate, u32)], mean_per_day: f64) -> Value {
    daily_option(
        format!("Graph N1. Dynamics userstars, repository '{repo}'"),
        daily,
        "lime",
        "green",
        Some(mean_per_day),
    )
}

fn gone_chart(repo: &str, gone: &[(NaiveDate, u32)]) -> Value {
    daily_option(
        format!("Graph. Gone stars (calculations), repository '{repo}'"),
        gone,
        "red",
        "#a73c3c",
        None,
    )
}

pub fn cumulative_chart(repo: &str, cumulative: &[(NaiveDate, u64)]) -> Value {
    let points: Vec<Value> = cumulative
        .iter()
        .map(|(day, stars)| date_point(*day, *stars))
        .collect();
    json!({
        "title": {"text": format!("Graph N2. Cumulative growth of stars, repository '{repo}'")},
        "tooltip": tooltip(),
        "toolbox": toolbox(),
        "dataZoom": zoom(),
        "grid": {"containLabel": true, "top": 115, "bottom": 90},
        "xAxis": time_axis("Date"),
        "yAxis": {"type": "value", "name": "Quantity stars"},
        "series": [{
            "type": "line",
            "smooth": true,
            "symbol": STAR,
            "symbolSize": 6,
            "lineStyle": {"width": 5},
            "data": points,
        }],
    })
}

pub fn year_chart(repo: &str, by_year: &[(i32, u64)], mean_per_year: Option<f64>) -> Value {
    let total: u64 = by_year.iter().map(|(_, stars)| stars).sum();
    let labels: Vec<String> = by_year.iter().map(|(year, _)| year.to_string()).collect();
    let points: Vec<Value> = by_year
        .iter()
        .map(|(year, stars)| labelled(breakdown("Year", &year.to_string(), *stars, total), *stars))
        .collect();
    let mut bars = json!({
        "type": "bar",
        "itemStyle": {"color": "#ff9200", "borderColor": "black", "borderWidth": 4},
        "data": points,
    });
    if let Some(mean) = mean_per_year {
        bars["markLine"] = mean_line(format!("mean = {mean:.1} stars / year"), mean);
    }
    json!({
        "title": {"text": format!("Histogram N1. Cumulative set of stars by year, repository '{repo}'")},
        "tooltip": tooltip(),
        "toolbox": toolbox(),
        "grid": {"containLabel": true, "top": 115, "right": 70},
        "xAxis": {"type": "category", "name": "Year", "nameLocation": "middle", "nameGap": 30, "data": labels, "axisLabel": {"hideOverlap": true}},
        "yAxis": {"type": "value", "name": "Quantity stars"},
        "series": [bars],
    })
}

pub fn month_chart(repo: &str, by_month: &[(u32, u64)]) -> Value {
    let total: u64 = by_month.iter().map(|(_, stars)| stars).sum();
    let labels: Vec<&str> = by_month
        .iter()
        .map(|(month, _)| month_name(*month))
        .collect();
    let points: Vec<Value> = by_month
        .iter()
        .map(|(month, stars)| {
            labelled(
                breakdown("Month", month_name(*month), *stars, total),
                *stars,
            )
        })
        .collect();
    json!({
        "title": {"text": format!("Histogram N2. Cumulative set of stars by month, repository '{repo}'")},
        "tooltip": tooltip(),
        "toolbox": toolbox(),
        "grid": {"containLabel": true, "top": 115, "right": 70},
        "xAxis": {"type": "category", "name": "Month", "nameLocation": "middle", "nameGap": 30, "data": labels, "axisLabel": {"hideOverlap": true}},
        "yAxis": {"type": "value", "name": "Quantity stars"},
        "series": [{
            "type": "bar",
            "itemStyle": {"color": "orange", "borderColor": "grey", "borderWidth": 2},
            "data": points,
        }],
    })
}

pub fn weekday_chart(repo: &str, by_weekday: &[u64; 7]) -> Value {
    let total: u64 = by_weekday.iter().sum();
    let points: Vec<Value> = by_weekday
        .iter()
        .zip(WEEKDAYS)
        .map(|(stars, day)| labelled(breakdown("Day of the week", day, *stars, total), *stars))
        .collect();
    json!({
        "title": {"text": format!("Histogram N3. Star Day (distribution of stars by day of the week), repository '{repo}'")},
        "tooltip": tooltip(),
        "toolbox": toolbox(),
        "xAxis": {"type": "value", "name": "Quantity stars", "nameLocation": "middle", "nameGap": 30},
        "grid": {"containLabel": true, "top": 115, "right": 70},
        "yAxis": {"type": "category", "name": "day of the week", "inverse": true, "data": WEEKDAYS, "axisLabel": {"hideOverlap": true}},
        "series": [{
            "type": "bar",
            "itemStyle": {"color": "#6800ff"},
            "data": points,
        }],
    })
}

pub fn hour_chart(repo: &str, by_hour: &[u64; 24], tz_label: &str) -> Value {
    let total: u64 = by_hour.iter().sum();
    let points: Vec<Value> = by_hour
        .iter()
        .enumerate()
        .map(|(hour, stars)| {
            labelled(
                breakdown("Hours", &format!("{hour:02}:00"), *stars, total),
                *stars,
            )
        })
        .collect();
    let hours_24: Vec<String> = (0..24).map(|h| format!("{h:02}:00")).collect();
    let hours_12: Vec<String> = (0..24)
        .map(|h| {
            let twelve = match h % 12 {
                0 => 12,
                n => n,
            };
            let period = if h < 12 { "AM" } else { "PM" };
            format!("{twelve:02}:00 {period}")
        })
        .collect();
    json!({
        "title": {"text": format!("Histogram N4. Star Hour (distribution of stars by hour, {tz_label} time zones), repository '{repo}'")},
        "tooltip": tooltip(),
        "toolbox": toolbox(),
        "xAxis": {"type": "value", "name": "Quantity stars", "nameLocation": "middle", "nameGap": 30},
        "grid": {"containLabel": true, "top": 115, "right": 70},
        "yAxis": [
            {"type": "category", "name": "24 hour time format", "inverse": true, "data": hours_24, "axisLabel": {"hideOverlap": true}},
            {
                "type": "category",
                "name": "12 hour time format",
                "position": "right",
                "inverse": true,
                "splitLine": {"show": false},
                "data": hours_12,
                "axisLabel": {"hideOverlap": true},
            },
        ],
        "series": [
            {"type": "bar", "itemStyle": {"color": "blue"}, "data": points},
            {"type": "scatter", "yAxisIndex": 1, "silent": true, "data": []},
        ],
    })
}

pub fn graphs_page(repo: &str, stats: &Stats, tz_label: &str) -> String {
    let mut data = json!({
        "daily": daily_chart(repo, &stats.daily, stats.mean_per_day),
        "cumulative": cumulative_chart(repo, &stats.cumulative),
        "year": year_chart(repo, &stats.by_year, stats.mean_per_year),
        "month": month_chart(repo, &stats.by_month),
        "weekday": weekday_chart(repo, &stats.by_weekday),
    });
    if let Some(by_hour) = &stats.by_hour {
        data["hour"] = hour_chart(repo, by_hour, tz_label);
    }
    GraphsPage {
        repo,
        has_hour: stats.by_hour.is_some(),
        data: json_for_script(&data),
    }
    .render()
    .expect("rendering a template into a String does not fail")
}

pub fn gone_graph_page(repo: &str, gone: &[(NaiveDate, u32)]) -> String {
    let data = json!({"gone": gone_chart(repo, gone)});
    GoneGraphPage {
        repo,
        data: json_for_script(&data),
    }
    .render()
    .expect("rendering a template into a String does not fail")
}

pub fn json_for_script(v: &Value) -> String {
    serde_json::to_string(v)
        .expect("a JSON value serializes")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn daily_option(
    title: String,
    days: &[(NaiveDate, u32)],
    bar_color: &str,
    marker_color: &str,
    mean: Option<f64>,
) -> Value {
    let points: Vec<Value> = days
        .iter()
        .filter(|(_, stars)| *stars > 0)
        .map(|(day, stars)| date_point(*day, u64::from(*stars)))
        .collect();
    let bars = json!({
        "type": "bar",
        "barWidth": 1,
        "silent": true,
        "itemStyle": {"color": bar_color},
        "data": points.clone(),
    });
    let mut scatter = json!({
        "type": "scatter",
        "symbol": STAR,
        "symbolSize": 8,
        "itemStyle": {"color": marker_color},
        "data": points,
    });
    if let Some(mean) = mean {
        scatter["markLine"] = mean_line(format!("mean = {mean:.1} stars / days"), mean);
    }
    json!({
        "title": {"text": title},
        "tooltip": tooltip(),
        "toolbox": toolbox(),
        "dataZoom": zoom(),
        "grid": {"containLabel": true, "top": 115, "bottom": 90},
        "xAxis": time_axis("Date"),
        "yAxis": {"type": "value", "name": "Quantity stars", "min": 0, "minInterval": 1},
        "series": [bars, scatter],
    })
}

fn date_point(day: NaiveDate, stars: u64) -> Value {
    json!({
        "name": format!("Date: {day}<br>Stars: {stars}"),
        "value": [day.to_string(), stars],
    })
}

fn labelled(name: String, stars: u64) -> Value {
    json!({"name": name, "value": stars})
}

fn breakdown(head: &str, label: &str, stars: u64, total: u64) -> String {
    format!(
        "{head}: {label}<br>Stars: {stars} ({:.1}%)",
        share(stars, total)
    )
}

fn share(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

fn month_name(month: u32) -> &'static str {
    month
        .checked_sub(1)
        .and_then(|i| MONTHS.get(i as usize))
        .copied()
        .unwrap_or("")
}

fn mean_line(label: String, value: f64) -> Value {
    json!({
        "silent": true,
        "symbol": "none",
        "lineStyle": {"color": "red", "width": 2},
        "label": {"formatter": label, "position": "insideEndTop"},
        "data": [{"yAxis": value}],
    })
}

fn tooltip() -> Value {
    json!({"trigger": "item", "formatter": "{b}"})
}

fn toolbox() -> Value {
    json!({"feature": {"saveAsImage": {}}})
}

fn zoom() -> Value {
    json!([{"type": "inside"}, {"type": "slider"}])
}

fn time_axis(name: &str) -> Value {
    json!({
        "type": "time",
        "name": name,
        "nameLocation": "middle",
        "nameGap": 30,
        "minInterval": 86_400_000,
        "axisLabel": {"formatter": "{yyyy}-{MM}-{dd}", "hideOverlap": true},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Marketing;

    fn sample_stats() -> Stats {
        let day = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        Stats {
            age_days: 30,
            daily: vec![(day, 3)],
            cumulative: vec![(day, 3)],
            peak: Some((day, 3)),
            trend_per_day: 0,
            mean_per_day: 3.0,
            mean_per_year: None,
            last30: None,
            stdev: None,
            marketing: Marketing::None,
            by_weekday: [0, 0, 0, 0, 0, 0, 3],
            by_month: vec![(1, 3)],
            by_year: vec![(2026, 3)],
            by_hour: Some([3; 24]),
            best_weekday: None,
            worst_weekday: None,
            best_month: None,
            worst_month: None,
            month_quartiles: None,
            year_quartiles: None,
            fetched_stars: 3,
            private_stars: 0,
            longest_gap: None,
        }
    }

    #[test]
    fn json_for_script_escapes_closing_tag() {
        let out = json_for_script(&json!("</script>"));
        assert!(!out.contains("</script>"));
        assert!(out.contains("\\u003c/script\\u003e"));
    }

    fn page_data(page: &str) -> Value {
        let marker = "id=\"data\">";
        let start = page.find(marker).unwrap() + marker.len();
        let end = start + page[start..].find("</script>").unwrap();
        serde_json::from_str(&page[start..end]).unwrap()
    }

    fn sorted_keys(data: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = data
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn graphs_page_has_relative_echarts_and_six_charts() {
        let page = graphs_page("owner/repo", &sample_stats(), "UTC");
        assert!(!page.contains("file://"));
        assert!(page.contains("../../../assets/echarts.min.js"));
        assert!(page.contains("id=\"hour\""));
        let data = page_data(&page);
        assert_eq!(
            sorted_keys(&data),
            ["cumulative", "daily", "hour", "month", "weekday", "year"]
        );
    }

    #[test]
    fn graphs_page_without_hours_has_five_charts() {
        let mut stats = sample_stats();
        stats.by_hour = None;
        let page = graphs_page("owner/repo", &stats, "UTC");
        assert!(!page.contains("id=\"hour\""));
        let data = page_data(&page);
        assert_eq!(
            sorted_keys(&data),
            ["cumulative", "daily", "month", "weekday", "year"]
        );
    }

    #[test]
    fn weekday_chart_has_seven_categories() {
        let chart = weekday_chart("owner/repo", &[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(chart["yAxis"]["data"].as_array().unwrap().len(), 7);
    }

    #[test]
    fn daily_chart_time_axis_has_day_step_and_hides_overlap() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let chart = daily_chart("owner/repo", &[(day, 3)], 3.0);
        assert_eq!(chart["xAxis"]["minInterval"], 86_400_000);
        assert_eq!(chart["xAxis"]["axisLabel"]["hideOverlap"], true);
    }

    #[test]
    fn daily_and_gone_charts_leave_room_for_wrapped_title() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let daily = daily_chart("owner/repo", &[(day, 3)], 3.0);
        let gone = gone_chart("owner/repo", &[(day, 1)]);
        assert_eq!(daily["grid"]["top"], 115);
        assert_eq!(gone["grid"]["top"], 115);
    }

    #[test]
    fn daily_chart_x_axis_name_sits_centered_below_axis() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let chart = daily_chart("owner/repo", &[(day, 3)], 3.0);
        assert_eq!(chart["xAxis"]["nameLocation"], "middle");
        assert_eq!(chart["xAxis"]["nameGap"], 30);
    }
}
