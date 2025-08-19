use serde::Serialize;
use std::collections::HashMap;
use regex::Regex;

#[derive(Debug, Serialize)]
pub struct MetricsData {
    pub latest_fixed_metrics: HashMap<String, String>,
    pub historical_metrics: HashMap<String, Vec<(i64, f64)>>,
}

// A list of metrics that should only show the latest value, not historical data.
const FIXED_METRICS: &[&str] = &[
    "Computation",
    "Mean action noise std",
    "Mean value_function loss",
    "Mean surrogate loss",
    "Mean entropy loss",
    "Mean reward",
    "Mean episode length",
    "Total timesteps",
    "Iteration time",
    "Time elapsed",
    "ETA",
];

const EXCLUDED_METRICS: &[&str] = &[
    "physics step-size",
    "rendering step-size",
    "environment step-size",
    "active action terms",
    "environment seed",
    "environment spacing",
    "setting seed",
    "number of environments",
];

pub fn parse_log_file(content: &str) -> MetricsData {
    let mut latest_fixed_metrics = HashMap::new();
    let mut historical_metrics: HashMap<String, Vec<(i64, f64)>> = HashMap::new();

    let block_separator = "################################################################################";
    let iteration_regex = Regex::new(r"Learning iteration (\d+)/\d+").unwrap();
    let metric_regex = Regex::new(r"^\s*([^:]+):\s+(.+)").unwrap();

    let mut current_iteration = 0;

    for block in content.split(block_separator).filter(|s| !s.trim().is_empty()) {
        if let Some(captures) = iteration_regex.captures(block) {
            if let Ok(iteration_num) = captures[1].parse::<i64>() {
                current_iteration = iteration_num;
            }
        }

        for line in block.lines() {
            if let Some(captures) = metric_regex.captures(line.trim()) {
                let key = captures[1].trim().to_string();
                let lower_key = key.to_lowercase();

                if EXCLUDED_METRICS.iter().any(|&excluded| lower_key.contains(excluded)) {
                    continue;
                }

                if let Ok(value) = captures[2].parse::<f64>() {
                    if FIXED_METRICS.contains(&key.as_str()) {
                        latest_fixed_metrics.insert(key, captures[2].to_string());
                    } else {
                        historical_metrics
                            .entry(key)
                            .or_default()
                            .push((current_iteration, value));
                    }
                } else if FIXED_METRICS.contains(&key.as_str()) {
                    // For metrics like ETA, which are not f64
                     latest_fixed_metrics.insert(key, captures[2].to_string());
                }
            }
        }
    }

    MetricsData {
        latest_fixed_metrics,
        historical_metrics,
    }
}
