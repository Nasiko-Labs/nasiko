use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "public_holidays",
                "description": "Get public holidays for a country and year. Returns holiday dates, names, and whether they are fixed or regional.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "country_code": {
                            "type": "string",
                            "description": "2-letter ISO country code (e.g. \"US\", \"DE\", \"JP\")"
                        },
                        "year": {
                            "type": "integer",
                            "description": "Year to get holidays for (e.g. 2025)"
                        }
                    },
                    "required": ["country_code", "year"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "working_days",
                "description": "Calculate the number of working/business days between two dates, excluding weekends and public holidays for the given country.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "start_date": {
                            "type": "string",
                            "description": "Start date in YYYY-MM-DD format"
                        },
                        "end_date": {
                            "type": "string",
                            "description": "End date in YYYY-MM-DD format"
                        },
                        "country_code": {
                            "type": "string",
                            "description": "2-letter ISO country code for public holidays (e.g. \"US\", \"DE\", \"JP\")"
                        }
                    },
                    "required": ["start_date", "end_date", "country_code"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "world_clock",
                "description": "Get the current time in a specific timezone. Returns datetime, UTC offset, day of week, week number, and DST status.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "timezone": {
                            "type": "string",
                            "description": "IANA timezone name (e.g. \"America/New_York\", \"Europe/London\", \"Asia/Tokyo\")"
                        }
                    },
                    "required": ["timezone"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "country_info",
                "description": "Get country information including capital, region, income level, and latest population (World Bank data). Useful for understanding labor markets and regional context.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Country name (e.g. \"Germany\", \"Japan\", \"Brazil\")"
                        }
                    },
                    "required": ["name"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "public_holidays" => public_holidays(arguments).await,
        "working_days" => working_days(arguments).await,
        "world_clock" => world_clock(arguments).await,
        "country_info" => country_info(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

async fn public_holidays(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let country_code = args["country_code"].as_str().ok_or("missing 'country_code'")?;
    let year = args["year"].as_u64().ok_or("missing 'year'")?;

    let url = format!(
        "https://date.nager.at/api/v3/PublicHolidays/{}/{}",
        year,
        urlencode(country_code),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let holidays = resp.as_array().ok_or("unexpected response format")?;

    if holidays.is_empty() {
        return Ok(format!("No public holidays found for {country_code} in {year}."));
    }

    let mut results = Vec::new();
    for holiday in holidays {
        let date = holiday["date"].as_str().unwrap_or("unknown");
        let name = holiday["localName"].as_str().unwrap_or("");
        let intl_name = holiday["name"].as_str().unwrap_or("");
        let fixed = holiday["fixed"].as_bool().unwrap_or(false);
        let counties = holiday["counties"].as_array();

        let mut entry = format!("  {date}  {intl_name}");
        if !name.is_empty() && name != intl_name {
            entry.push_str(&format!(" ({name})"));
        }
        if fixed {
            entry.push_str("  [fixed date]");
        }
        if let Some(regions) = counties {
            let region_list: Vec<&str> = regions.iter().filter_map(|r| r.as_str()).collect();
            if !region_list.is_empty() {
                entry.push_str(&format!("  [regional: {}]", region_list.join(", ")));
            }
        }
        results.push(entry);
    }

    Ok(format!(
        "Public holidays for {} in {}:\n\n{}",
        country_code.to_uppercase(),
        year,
        results.join("\n")
    ))
}

async fn working_days(arguments: &str) -> Result<String, String> {
    use chrono::NaiveDate;
    use chrono::Datelike;

    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let start_str = args["start_date"].as_str().ok_or("missing 'start_date'")?;
    let end_str = args["end_date"].as_str().ok_or("missing 'end_date'")?;
    let country_code = args["country_code"].as_str().ok_or("missing 'country_code'")?;

    let start = NaiveDate::parse_from_str(start_str, "%Y-%m-%d")
        .map_err(|e| format!("invalid start_date: {e}"))?;
    let end = NaiveDate::parse_from_str(end_str, "%Y-%m-%d")
        .map_err(|e| format!("invalid end_date: {e}"))?;

    if end < start {
        return Err("end_date must be on or after start_date".into());
    }

    // Collect all years in the range to fetch holidays
    let start_year = start.year();
    let end_year = end.year();

    let mut holiday_dates: std::collections::HashSet<NaiveDate> = std::collections::HashSet::new();

    for year in start_year..=end_year {
        let url = format!(
            "https://date.nager.at/api/v3/PublicHolidays/{}/{}",
            year,
            urlencode(country_code),
        );

        if let Ok(resp) = reqwest::get(&url).await {
            if let Ok(holidays) = resp.json::<serde_json::Value>().await {
                if let Some(arr) = holidays.as_array() {
                    for holiday in arr {
                        if let Some(date_str) = holiday["date"].as_str() {
                            if let Ok(d) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                                holiday_dates.insert(d);
                            }
                        }
                    }
                }
            }
        }
    }

    // Iterate through the date range and count
    let mut total_days = 0i64;
    let mut weekends = 0u32;
    let mut holidays_in_range = 0u32;
    let mut working = 0u32;

    let mut current = start;
    while current <= end {
        total_days += 1;
        let weekday = current.weekday();
        let is_weekend =
            weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun;
        let is_holiday = holiday_dates.contains(&current);

        if is_weekend {
            weekends += 1;
        } else if is_holiday {
            holidays_in_range += 1;
        } else {
            working += 1;
        }

        current = current.succ_opt().unwrap_or(current);
        if current == end.succ_opt().unwrap_or(end) && current != end {
            break;
        }
    }

    Ok(format!(
        "Working days between {} and {} ({}):\n\n\
         Total calendar days: {}\n\
         Weekend days:        {}\n\
         Public holidays:     {} (on weekdays within range)\n\
         Net working days:    {}",
        start_str,
        end_str,
        country_code.to_uppercase(),
        total_days,
        weekends,
        holidays_in_range,
        working,
    ))
}

async fn world_clock(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let timezone = args["timezone"].as_str().ok_or("missing 'timezone'")?;

    let url = format!(
        "http://worldtimeapi.org/api/timezone/{}",
        urlencode(timezone),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if resp.get("error").is_some() {
        return Err(format!(
            "Invalid timezone '{}'. Use IANA format like 'America/New_York'.",
            timezone
        ));
    }

    let tz = resp["timezone"].as_str().unwrap_or(timezone);
    let datetime = resp["datetime"].as_str().unwrap_or("unknown");
    let utc_offset = resp["utc_offset"].as_str().unwrap_or("unknown");
    let day_of_week = resp["day_of_week"].as_u64().unwrap_or(0);
    let week_number = resp["week_number"].as_u64().unwrap_or(0);
    let dst = resp["dst"].as_bool().unwrap_or(false);

    let day_name = match day_of_week {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        7 => "Sunday",
        _ => "Unknown",
    };

    Ok(format!(
        "Current time in {}:\n\n\
         Datetime:    {}\n\
         UTC offset:  {}\n\
         Day of week: {} ({})\n\
         Week number: {}\n\
         DST active:  {}",
        tz,
        datetime,
        utc_offset,
        day_name,
        day_of_week,
        week_number,
        if dst { "Yes" } else { "No" },
    ))
}

async fn country_info(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let name = args["name"].as_str().ok_or("missing 'name'")?;

    // restcountries.com retired its free API (v1–v4 return deprecation
    // errors) — use the World Bank API instead: country metadata plus the
    // latest population indicator, no key required.
    let list = reqwest::get("https://api.worldbank.org/v2/country?format=json&per_page=400")
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let countries = list
        .get(1)
        .and_then(|v| v.as_array())
        .ok_or("unexpected country list response")?;

    let query = name.trim().to_lowercase();
    // Real countries only — the list also carries regional aggregates.
    let real = || {
        countries.iter().filter(|c| {
            c["region"]["value"]
                .as_str()
                .is_some_and(|r| r.trim() != "Aggregates")
        })
    };
    let country = real()
        .find(|c| {
            c["name"].as_str().unwrap_or("").eq_ignore_ascii_case(&query)
                || c["iso2Code"].as_str().unwrap_or("").eq_ignore_ascii_case(&query)
                || c["id"].as_str().unwrap_or("").eq_ignore_ascii_case(&query)
        })
        .or_else(|| {
            // Fuzzy pass: "Venezuela" should match "Venezuela, RB".
            real().find(|c| {
                let n = c["name"].as_str().unwrap_or("").to_lowercase();
                n.contains(&query) || query.contains(n.trim_end_matches(", rb"))
            })
        });

    let Some(country) = country else {
        return Ok(format!("No country found matching '{name}'."));
    };

    let code = country["id"].as_str().unwrap_or("");
    let official_name = country["name"].as_str().unwrap_or(name);
    let capital = country["capitalCity"].as_str().filter(|s| !s.is_empty()).unwrap_or("N/A");
    let region = country["region"]["value"].as_str().unwrap_or("unknown").trim();
    let income = country["incomeLevel"]["value"].as_str().unwrap_or("unknown");

    // Latest available total population (mrnev=1 = most recent non-empty value).
    let pop_url = format!(
        "https://api.worldbank.org/v2/country/{code}/indicator/SP.POP.TOTL?format=json&mrnev=1"
    );
    let pop_resp = reqwest::get(&pop_url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let pop_entry = pop_resp.get(1).and_then(|v| v.get(0));
    let population = pop_entry
        .and_then(|e| e["value"].as_u64())
        .map(format_population)
        .unwrap_or_else(|| "N/A".into());
    let pop_year = pop_entry
        .and_then(|e| e["date"].as_str())
        .unwrap_or("latest");

    Ok(format!(
        "Country: {official_name} ({code})\n\n\
         Capital:      {capital}\n\
         Region:       {region}\n\
         Income level: {income}\n\
         Population:   {population} ({pop_year}, World Bank)",
    ))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            b' ' => "+".into(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}

fn format_population(pop: u64) -> String {
    if pop >= 1_000_000_000 {
        format!("{:.2}B", pop as f64 / 1_000_000_000.0)
    } else if pop >= 1_000_000 {
        format!("{:.2}M", pop as f64 / 1_000_000.0)
    } else if pop >= 1_000 {
        format!("{:.1}K", pop as f64 / 1_000.0)
    } else {
        pop.to_string()
    }
}
