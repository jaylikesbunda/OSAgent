use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub struct WeatherTool {
    client: Client,
}

impl WeatherTool {
    pub fn new(_config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("OSAgent/0.1")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    fn build_url(location: Option<&str>) -> String {
        match location.map(str::trim).filter(|value| !value.is_empty()) {
            Some(location) => format!("https://wttr.in/{}?format=j1", urlencoding::encode(location)),
            None => "https://wttr.in/?format=j1".to_string(),
        }
    }

    fn forecast_lines(value: &Value, units: &str, days: usize) -> Vec<String> {
        let Some(entries) = value["weather"].as_array() else {
            return Vec::new();
        };

        entries
            .iter()
            .take(days)
            .map(|entry| {
                let date = entry["date"].as_str().unwrap_or("unknown date");
                let condition = entry["hourly"]
                    .as_array()
                    .and_then(|hourly| hourly.first())
                    .and_then(|hour| hour["weatherDesc"].as_array())
                    .and_then(|desc| desc.first())
                    .and_then(|desc| desc["value"].as_str())
                    .unwrap_or("Unknown");
                let (max_temp, min_temp, wind) = if units == "imperial" {
                    (
                        entry["maxtempF"].as_str().unwrap_or("?"),
                        entry["mintempF"].as_str().unwrap_or("?"),
                        entry["hourly"]
                            .as_array()
                            .and_then(|hourly| hourly.first())
                            .and_then(|hour| hour["windspeedMiles"].as_str())
                            .unwrap_or("?"),
                    )
                } else {
                    (
                        entry["maxtempC"].as_str().unwrap_or("?"),
                        entry["mintempC"].as_str().unwrap_or("?"),
                        entry["hourly"]
                            .as_array()
                            .and_then(|hourly| hourly.first())
                            .and_then(|hour| hour["windspeedKmph"].as_str())
                            .unwrap_or("?"),
                    )
                };
                let temp_unit = if units == "imperial" { "F" } else { "C" };
                let wind_unit = if units == "imperial" { "mph" } else { "km/h" };
                format!(
                    "{}: {} (high {}{}, low {}{}, wind {} {})",
                    date, condition, max_temp, temp_unit, min_temp, temp_unit, wind, wind_unit
                )
            })
            .collect()
    }

    fn render_report(value: &Value, location: Option<&str>, units: &str, days: usize) -> String {
        let resolved_location = value["nearest_area"]
            .as_array()
            .and_then(|areas| areas.first())
            .and_then(|area| area["areaName"].as_array())
            .and_then(|names| names.first())
            .and_then(|name| name["value"].as_str())
            .map(ToString::to_string)
            .or_else(|| location.map(ToString::to_string))
            .unwrap_or_else(|| "current location".to_string());
        let current = value["current_condition"]
            .as_array()
            .and_then(|entries| entries.first());
        let condition = current
            .and_then(|entry| entry["weatherDesc"].as_array())
            .and_then(|descriptions| descriptions.first())
            .and_then(|description| description["value"].as_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| "Unknown".to_string());
        let humidity = current
            .and_then(|entry| entry["humidity"].as_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| "?".to_string());

        let (temp, feels_like, wind) = if units == "imperial" {
            (
                current.and_then(|entry| entry["temp_F"].as_str()).map(ToString::to_string),
                current
                    .and_then(|entry| entry["FeelsLikeF"].as_str())
                    .map(ToString::to_string),
                current
                    .and_then(|entry| entry["windspeedMiles"].as_str())
                    .map(ToString::to_string),
            )
        } else {
            (
                current.and_then(|entry| entry["temp_C"].as_str()).map(ToString::to_string),
                current
                    .and_then(|entry| entry["FeelsLikeC"].as_str())
                    .map(ToString::to_string),
                current
                    .and_then(|entry| entry["windspeedKmph"].as_str())
                    .map(ToString::to_string),
            )
        };

        let temp_unit = if units == "imperial" { "F" } else { "C" };
        let wind_unit = if units == "imperial" { "mph" } else { "km/h" };

        let mut lines = vec![
            format!("Weather for {}", resolved_location),
            format!("Current: {}{}, {}", temp.unwrap_or_else(|| "?".to_string()), temp_unit, condition),
            format!("Feels like: {}{}", feels_like.unwrap_or_else(|| "?".to_string()), temp_unit),
            format!("Humidity: {}%", humidity),
            format!("Wind: {} {}", wind.unwrap_or_else(|| "?".to_string()), wind_unit),
        ];

        let forecast = Self::forecast_lines(value, units, days.clamp(1, 3));
        if !forecast.is_empty() {
            lines.push("Forecast:".to_string());
            lines.extend(forecast);
        }

        lines.join("\n")
    }
}

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "Fetch current weather and a short forecast for a city, region, or your current location"
    }

    fn when_to_use(&self) -> &str {
        "Use for current conditions, temperature, and short forecast lookups that need live web data"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when offline or when you need historical climate analysis"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Check a city forecast".to_string(),
                input: json!({"location": "Boston", "days": 2}),
            },
            ToolExample {
                description: "Use imperial units".to_string(),
                input: json!({"location": "Austin", "units": "imperial"}),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "Optional location such as 'Boston', 'Paris', or '94107'"
                },
                "units": {
                    "type": "string",
                    "enum": ["metric", "imperial"],
                    "description": "Temperature and wind units"
                },
                "days": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 3,
                    "description": "Number of forecast days to include"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let location = args["location"].as_str();
        let units = args["units"].as_str().unwrap_or("metric");
        let days = args["days"].as_u64().unwrap_or(1) as usize;
        let url = Self::build_url(location);

        let response = self.client.get(url).send().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to fetch weather data: {}", e))
        })?;
        if !response.status().is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "Weather service returned status {}",
                response.status()
            )));
        }

        let payload = response.json::<Value>().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to parse weather response: {}", e))
        })?;

        Ok(Self::render_report(&payload, location, units, days))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_location_url() {
        assert_eq!(WeatherTool::build_url(Some("New York")), "https://wttr.in/New%20York?format=j1");
    }

    #[test]
    fn renders_weather_report() {
        let payload = json!({
            "nearest_area": [{"areaName": [{"value": "Boston"}]}],
            "current_condition": [{
                "temp_C": "12",
                "temp_F": "54",
                "FeelsLikeC": "10",
                "FeelsLikeF": "50",
                "humidity": "65",
                "windspeedKmph": "18",
                "windspeedMiles": "11",
                "weatherDesc": [{"value": "Partly cloudy"}]
            }],
            "weather": [{
                "date": "2026-04-04",
                "maxtempC": "14",
                "mintempC": "8",
                "maxtempF": "57",
                "mintempF": "46",
                "hourly": [{
                    "windspeedKmph": "20",
                    "windspeedMiles": "12",
                    "weatherDesc": [{"value": "Sunny intervals"}]
                }]
            }]
        });

        let report = WeatherTool::render_report(&payload, Some("Boston"), "metric", 1);
        assert!(report.contains("Weather for Boston"));
        assert!(report.contains("Current: 12C"));
        assert!(report.contains("Forecast:"));
    }
}
