use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "exchange_rates",
                "description": "Get current exchange rates for a base currency. Returns conversion rates against other currencies.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "base": {
                            "type": "string",
                            "description": "Base currency code (e.g. \"USD\", \"EUR\", \"GBP\")"
                        },
                        "targets": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional list of target currency codes (e.g. [\"EUR\", \"GBP\"]). If omitted, returns top 10 rates."
                        }
                    },
                    "required": ["base"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "crypto_price",
                "description": "Get current price and market data for a cryptocurrency. Use the CoinGecko coin ID (e.g. \"bitcoin\", \"ethereum\", \"solana\").",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "coin_id": {
                            "type": "string",
                            "description": "CoinGecko coin ID (e.g. \"bitcoin\", \"ethereum\", \"cardano\")"
                        }
                    },
                    "required": ["coin_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "crypto_search",
                "description": "Search for a cryptocurrency by name or symbol. Useful when you don't know the exact CoinGecko ID for a coin.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (name or symbol, e.g. \"bitcoin\" or \"BTC\")"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "crypto_market",
                "description": "Get top cryptocurrencies ranked by market cap. Returns prices, 24h changes, and market caps.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Number of results (5-20)",
                            "default": 10
                        }
                    }
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "exchange_rates" => exchange_rates(arguments).await,
        "crypto_price" => crypto_price(arguments).await,
        "crypto_search" => crypto_search(arguments).await,
        "crypto_market" => crypto_market(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

async fn exchange_rates(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let base = args["base"]
        .as_str()
        .ok_or("missing 'base'")?
        .to_uppercase();

    let targets: Option<Vec<String>> = args["targets"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_uppercase()))
            .collect()
    });

    let url = format!(
        "https://open.er-api.com/v6/latest/{}",
        urlencode(&base),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if resp["result"].as_str() != Some("success") {
        let error_type = resp["error-type"].as_str().unwrap_or("unknown error");
        return Err(format!("API error: {error_type}"));
    }

    let rates = resp["rates"]
        .as_object()
        .ok_or("missing rates in response")?;
    let last_update = resp["time_last_update_utc"]
        .as_str()
        .unwrap_or("unknown");

    let mut output = format!("**Exchange Rates for {base}**\nLast updated: {last_update}\n\n");

    match targets {
        Some(target_list) => {
            for target in &target_list {
                if let Some(rate) = rates.get(target.as_str()) {
                    output.push_str(&format!("  {base} -> {target}: {rate}\n"));
                } else {
                    output.push_str(&format!("  {base} -> {target}: not available\n"));
                }
            }
        }
        None => {
            // Show top 10 major currencies
            let major = [
                "USD", "EUR", "GBP", "JPY", "CHF", "CAD", "AUD", "CNY", "INR", "BRL",
            ];
            for currency in major {
                if currency == base {
                    continue;
                }
                if let Some(rate) = rates.get(currency) {
                    output.push_str(&format!("  {base} -> {currency}: {rate}\n"));
                }
            }
        }
    }

    Ok(output)
}

async fn crypto_price(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let coin_id = args["coin_id"]
        .as_str()
        .ok_or("missing 'coin_id'")?
        .to_lowercase();

    let url = format!(
        "https://api.coingecko.com/api/v3/coins/{}?localization=false&tickers=false&community_data=false&developer_data=false",
        urlencode(&coin_id),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if let Some(error) = resp["error"].as_str() {
        return Err(format!("CoinGecko error: {error}"));
    }

    let name = resp["name"].as_str().unwrap_or("Unknown");
    let symbol = resp["symbol"]
        .as_str()
        .unwrap_or("")
        .to_uppercase();
    let market = &resp["market_data"];

    let price_usd = market["current_price"]["usd"]
        .as_f64()
        .map(|p| format_price(p))
        .unwrap_or_else(|| "N/A".into());
    let price_eur = market["current_price"]["eur"]
        .as_f64()
        .map(|p| format_price(p))
        .unwrap_or_else(|| "N/A".into());
    let price_btc = market["current_price"]["btc"]
        .as_f64()
        .map(|p| format!("{:.8}", p))
        .unwrap_or_else(|| "N/A".into());

    let market_cap = market["market_cap"]["usd"]
        .as_f64()
        .map(|m| format_large_number(m))
        .unwrap_or_else(|| "N/A".into());

    let change_24h = market["price_change_percentage_24h"]
        .as_f64()
        .map(|c| format!("{:+.2}%", c))
        .unwrap_or_else(|| "N/A".into());

    let high_24h = market["high_24h"]["usd"]
        .as_f64()
        .map(|p| format_price(p))
        .unwrap_or_else(|| "N/A".into());
    let low_24h = market["low_24h"]["usd"]
        .as_f64()
        .map(|p| format_price(p))
        .unwrap_or_else(|| "N/A".into());

    let ath = market["ath"]["usd"]
        .as_f64()
        .map(|p| format_price(p))
        .unwrap_or_else(|| "N/A".into());

    let circulating = market["circulating_supply"]
        .as_f64()
        .map(|s| format_large_number(s))
        .unwrap_or_else(|| "N/A".into());

    Ok(format!(
        "**{name} ({symbol})**\n\n\
         Price (USD): ${price_usd}\n\
         Price (EUR): \u{20ac}{price_eur}\n\
         Price (BTC): {price_btc}\n\
         Market Cap:  ${market_cap}\n\
         24h Change:  {change_24h}\n\
         24h High:    ${high_24h}\n\
         24h Low:     ${low_24h}\n\
         ATH:         ${ath}\n\
         Circulating: {circulating}"
    ))
}

async fn crypto_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;

    let url = format!(
        "https://api.coingecko.com/api/v3/search?query={}",
        urlencode(query),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let coins = resp["coins"].as_array().ok_or("no results")?;

    if coins.is_empty() {
        return Ok(format!("No cryptocurrencies found for '{query}'."));
    }

    let mut results = Vec::new();
    for coin in coins.iter().take(5) {
        let id = coin["id"].as_str().unwrap_or("unknown");
        let name = coin["name"].as_str().unwrap_or("Unknown");
        let symbol = coin["symbol"]
            .as_str()
            .unwrap_or("")
            .to_uppercase();
        let rank = coin["market_cap_rank"]
            .as_u64()
            .map(|r| format!("#{r}"))
            .unwrap_or_else(|| "unranked".into());

        results.push(format!("  {name} ({symbol}) - ID: `{id}` - Rank: {rank}"));
    }

    Ok(format!(
        "**Search results for '{query}':**\n\n{}",
        results.join("\n")
    ))
}

async fn crypto_market(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let limit = args["limit"].as_u64().unwrap_or(10).clamp(5, 20);

    let url = format!(
        "https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&order=market_cap_desc&per_page={}&page=1&sparkline=false",
        limit,
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let coins = resp.as_array().ok_or("unexpected response format")?;

    if coins.is_empty() {
        return Ok("No market data available.".into());
    }

    let mut output = String::from("**Top Cryptocurrencies by Market Cap**\n\n");

    for coin in coins {
        let rank = coin["market_cap_rank"].as_u64().unwrap_or(0);
        let name = coin["name"].as_str().unwrap_or("Unknown");
        let symbol = coin["symbol"]
            .as_str()
            .unwrap_or("")
            .to_uppercase();
        let price = coin["current_price"]
            .as_f64()
            .map(|p| format_price(p))
            .unwrap_or_else(|| "N/A".into());
        let change_24h = coin["price_change_percentage_24h"]
            .as_f64()
            .map(|c| format!("{:+.2}%", c))
            .unwrap_or_else(|| "N/A".into());
        let market_cap = coin["market_cap"]
            .as_f64()
            .map(|m| format_large_number(m))
            .unwrap_or_else(|| "N/A".into());

        output.push_str(&format!(
            "  #{rank} {name} ({symbol}): ${price} ({change_24h}) - MCap: ${market_cap}\n"
        ));
    }

    Ok(output)
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

fn format_price(price: f64) -> String {
    if price >= 1.0 {
        format!("{:.2}", price)
    } else if price >= 0.01 {
        format!("{:.4}", price)
    } else {
        format!("{:.8}", price)
    }
}

fn format_large_number(n: f64) -> String {
    if n >= 1_000_000_000_000.0 {
        format!("{:.2}T", n / 1_000_000_000_000.0)
    } else if n >= 1_000_000_000.0 {
        format!("{:.2}B", n / 1_000_000_000.0)
    } else if n >= 1_000_000.0 {
        format!("{:.2}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.2}K", n / 1_000.0)
    } else {
        format!("{:.2}", n)
    }
}
