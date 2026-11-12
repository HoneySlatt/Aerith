// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The figures a run's readings give, and the table that compares runs. Pure functions over the
//! readings, so that they are tested without a player.

use serde_json::{Value, json};

use crate::process::TICKS_PER_SECOND;
use crate::sampler::Sample;

/// The largest value, ignoring missing ones.
fn largest(values: impl Iterator<Item = f64>) -> Option<f64> {
    values.fold(None, |best, value| {
        Some(best.map_or(value, |b: f64| b.max(value)))
    })
}

/// The mean, ignoring missing ones.
pub fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0_u32), |(sum, count), value| (sum + value, count + 1));
    (count > 0).then(|| sum / f64::from(count))
}

/// The `fraction` quantile (0.99 for the 99th percentile) by the nearest-rank method.
pub fn quantile(mut values: Vec<f64>, fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let rank = (fraction * values.len() as f64).ceil() as usize;
    Some(values[rank.clamp(1, values.len()) - 1])
}

/// The memory, CPU and power figures of the samples taken while the clip played.
pub fn machine_figures(samples: &[Sample], idle_gpu_microwatts: Option<f64>) -> Value {
    let pss = samples.iter().map(|sample| sample.pss_kib as f64 / 1024.0);
    let cpu = match (samples.first(), samples.last()) {
        (Some(first), Some(last)) if last.at > first.at => {
            let wall = last.at - first.at;
            let seconds = last.cpu_ticks.saturating_sub(first.cpu_ticks) as f64 / TICKS_PER_SECOND;
            // What the rest of the machine used meanwhile, in logical CPUs (hardware threads): a
            // build running beside the run shows here, and the run's figures are to be taken again.
            let others =
                first
                    .machine_busy_ticks
                    .zip(last.machine_busy_ticks)
                    .map(|(start, end)| {
                        let machine = end.saturating_sub(start) as f64 / TICKS_PER_SECOND;
                        round(((machine - seconds) / wall).max(0.0), 2)
                    });
            json!({
                "seconds": round(seconds, 2),
                "percent_of_one_core": round(100.0 * seconds / wall, 1),
                "others_cores": others,
            })
        }
        _ => Value::Null,
    };
    let playing = mean(
        samples
            .iter()
            .filter_map(|sample| sample.gpu_microwatts)
            .map(|uw| uw as f64),
    );
    json!({
        "memory_mib": {
            "peak_pss": largest(pss.clone()).map(|v| round(v, 1)),
            "mean_pss": mean(pss).map(|v| round(v, 1)),
        },
        "cpu": cpu,
        "gpu_power_w": {
            "idle_mean": idle_gpu_microwatts.map(|uw| round(uw / 1e6, 2)),
            "playing_mean": playing.map(|uw| round(uw / 1e6, 2)),
            "playing_minus_idle": playing.zip(idle_gpu_microwatts).map(|(p, i)| round((p - i) / 1e6, 2)),
        },
    })
}

/// The audio-video difference over a run, from mpv's `avsync` readings in seconds: the largest
/// and the 99th percentile of its absolute value, in milliseconds.
pub fn sync_figures(avsync_seconds: &[f64]) -> Value {
    if avsync_seconds.is_empty() {
        return Value::Null;
    }
    let absolute: Vec<f64> = avsync_seconds.iter().map(|s| (s * 1000.0).abs()).collect();
    json!({
        "max_abs_ms": largest(absolute.iter().copied()).map(|v| round(v, 2)),
        "p99_abs_ms": quantile(absolute.clone(), 0.99).map(|v| round(v, 2)),
        "readings": absolute.len(),
    })
}

/// Rounds to `places` decimal places, for readable reports.
pub fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

/// The header of the table that compares runs, in Markdown.
pub const TABLE_HEADER: &str = "| Rival | Clip | First frame | Dropped | Decoder dropped | Delayed | Mistimed | Off cadence / presented | Scanned out | Sync, max / p99 | Memory, peak PSS | CPU, one core | Other programs | GPU power over idle |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|";

/// One row of the table, from a run's result.
pub fn table_row(result: &Value) -> String {
    let text = |value: &Value, unit: &str| match value {
        Value::Null => "–".to_owned(),
        Value::Number(number) => format!("{number}{unit}"),
        other => other.to_string(),
    };
    let frames = &result["frames"];
    let sync = &result["sync"];
    let sync = if sync.is_null() {
        "–".to_owned()
    } else {
        format!(
            "{} / {}",
            text(&sync["max_abs_ms"], " ms"),
            text(&sync["p99_abs_ms"], " ms")
        )
    };
    let presentation = &result["presentation"];
    let cadence = if presentation.is_null() {
        "–".to_owned()
    } else {
        format!(
            "{} / {}",
            text(&presentation["refresh_gaps"]["off_cadence"], ""),
            text(&presentation["presented"], "")
        )
    };
    let rival = match (result["rival"].as_str(), result["order"].as_str()) {
        (Some(rival), Some(order)) => format!("{rival}, {order}"),
        (rival, _) => rival.unwrap_or("?").to_owned(),
    };
    format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        rival,
        result["clip"].as_str().unwrap_or("?"),
        text(&result["first_frame_ms"], " ms"),
        text(&frames["dropped"], ""),
        text(&frames["decoder_dropped"], ""),
        text(&frames["delayed"], ""),
        text(&frames["mistimed"], ""),
        cadence,
        text(&presentation["zero_copy"], ""),
        sync,
        text(&result["memory_mib"]["peak_pss"], " MiB"),
        text(&result["cpu"]["percent_of_one_core"], " %"),
        text(&result["cpu"]["others_cores"], " threads"),
        text(&result["gpu_power_w"]["playing_minus_idle"], " W"),
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm), for the run's date in UTC
/// without a date library.
pub fn civil_date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A Unix time in seconds as an ISO 8601 date and time in UTC.
pub fn iso_utc(unix_seconds: u64) -> String {
    let (days, seconds) = ((unix_seconds / 86_400) as i64, unix_seconds % 86_400);
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: f64, pss_kib: u64, cpu_ticks: u64, gpu_microwatts: Option<u64>) -> Sample {
        Sample {
            machine_busy_ticks: Some(cpu_ticks * 3),
            at,
            pss_kib,
            cpu_ticks,
            gpu_microwatts,
            processes: 1,
        }
    }

    #[test]
    fn quantiles_take_the_nearest_rank() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(quantile(values.clone(), 0.99), Some(99.0));
        assert_eq!(quantile(values, 1.0), Some(100.0));
        assert_eq!(quantile(vec![5.0], 0.99), Some(5.0));
        assert_eq!(quantile(Vec::new(), 0.99), None);
    }

    #[test]
    fn machine_figures_come_from_the_samples() {
        let samples = [
            sample(0.0, 102_400, 1000, Some(30_000_000)),
            sample(10.0, 204_800, 1500, Some(40_000_000)),
        ];
        let figures = machine_figures(&samples, Some(20_000_000.0));
        assert_eq!(figures["memory_mib"]["peak_pss"], json!(200.0));
        assert_eq!(figures["memory_mib"]["mean_pss"], json!(150.0));
        // 500 ticks are 5 s of CPU over 10 s.
        assert_eq!(figures["cpu"]["seconds"], json!(5.0));
        assert_eq!(figures["cpu"]["percent_of_one_core"], json!(50.0));
        // The machine was busy 1500 ticks, the rival 500: 10 s of others' CPU over 10 s.
        assert_eq!(figures["cpu"]["others_cores"], json!(1.0));
        assert_eq!(figures["gpu_power_w"]["playing_mean"], json!(35.0));
        assert_eq!(figures["gpu_power_w"]["playing_minus_idle"], json!(15.0));
    }

    #[test]
    fn machine_figures_without_readings_are_null() {
        let figures = machine_figures(&[], None);
        assert!(figures["cpu"].is_null());
        assert!(figures["memory_mib"]["peak_pss"].is_null());
        assert!(figures["gpu_power_w"]["playing_minus_idle"].is_null());
    }

    #[test]
    fn sync_is_measured_in_absolute_milliseconds() {
        let figures = sync_figures(&[0.001, -0.012, 0.004]);
        assert_eq!(figures["max_abs_ms"], json!(12.0));
        assert_eq!(figures["readings"], json!(3));
        assert!(sync_figures(&[]).is_null());
    }

    #[test]
    fn a_row_shows_missing_figures_as_a_dash() {
        let result = json!({
            "rival": "kodi", "clip": "hdr10", "first_frame_ms": 412.0,
            "frames": {"dropped": 3, "decoder_dropped": null, "delayed": null, "mistimed": null},
            "sync": null, "memory_mib": {"peak_pss": 300.5}, "cpu": {"percent_of_one_core": 12.5},
            "gpu_power_w": {"playing_minus_idle": null},
        });
        assert_eq!(
            table_row(&result),
            "| kodi | hdr10 | 412.0 ms | 3 | – | – | – | – | – | – | 300.5 MiB | 12.5 % | – | – |"
        );
        let aerith = json!({
            "rival": "aerith", "order": "above", "clip": "cadence-23.976", "first_frame_ms": 70.0,
            "frames": {"dropped": 0, "decoder_dropped": 0, "delayed": 0, "mistimed": 1},
            "presentation": {"presented": 1440, "zero_copy": 1440, "refresh_gaps": {"off_cadence": 2}},
            "sync": null, "memory_mib": {"peak_pss": null}, "cpu": {"percent_of_one_core": null},
            "gpu_power_w": {"playing_minus_idle": null},
        });
        assert_eq!(
            table_row(&aerith),
            "| aerith, above | cadence-23.976 | 70.0 ms | 0 | 0 | 0 | 1 | 2 / 1440 | 1440 | – | – | – | – | – |"
        );
        assert_eq!(
            TABLE_HEADER.lines().next().unwrap().matches('|').count(),
            table_row(&result).matches('|').count()
        );
    }

    #[test]
    fn dates_are_civil_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_794_369_600), "2026-11-11T04:00:00Z");
    }
}
