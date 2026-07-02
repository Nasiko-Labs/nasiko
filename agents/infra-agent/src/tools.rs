use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "terraform_modules",
                "description": "Search the Terraform Registry for modules. Returns module names, versions, descriptions, and download counts. Use for finding reusable infrastructure modules.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (e.g. 'vpc', 'eks', 's3-bucket', 'lambda')"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "terraform_provider",
                "description": "Get information about a Terraform provider from the registry. Returns version, description, source repository, downloads, and tier (official/partner/community).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Provider namespace (e.g. 'hashicorp', 'digitalocean', 'cloudflare')"
                        },
                        "name": {
                            "type": "string",
                            "description": "Provider name (e.g. 'aws', 'google', 'kubernetes', 'azurerm')"
                        }
                    },
                    "required": ["namespace", "name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "dns_lookup",
                "description": "Resolve DNS records for a domain using Google's DNS-over-HTTPS. Returns answer records with TTL and data. Useful for diagnosing DNS issues or verifying configurations.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "domain": {
                            "type": "string",
                            "description": "Domain name to resolve (e.g. 'example.com', 'api.github.com')"
                        },
                        "record_type": {
                            "type": "string",
                            "enum": ["A", "AAAA", "CNAME", "MX", "TXT", "NS"],
                            "description": "DNS record type to query (default: A)",
                            "default": "A"
                        }
                    },
                    "required": ["domain"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ip_info",
                "description": "Get geolocation and network information for an IP address. Returns country, region, city, timezone, ISP, organization, and AS number.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "ip": {
                            "type": "string",
                            "description": "IP address to look up (e.g. '8.8.8.8', '1.1.1.1')"
                        }
                    },
                    "required": ["ip"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "terraform_modules" => terraform_modules(arguments).await,
        "terraform_provider" => terraform_provider(arguments).await,
        "dns_lookup" => dns_lookup(arguments).await,
        "ip_info" => ip_info(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

async fn terraform_modules(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;

    let url = format!(
        "https://registry.terraform.io/v1/modules?q={}&limit=5",
        urlencode(query),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let modules = resp["modules"]
        .as_array()
        .ok_or("no modules in response")?;

    if modules.is_empty() {
        return Ok(format!("No Terraform modules found for '{query}'."));
    }

    let mut results = Vec::new();
    for module in modules {
        let namespace = module["namespace"].as_str().unwrap_or("unknown");
        let name = module["name"].as_str().unwrap_or("unknown");
        let provider = module["provider"].as_str().unwrap_or("unknown");
        let version = module["version"].as_str().unwrap_or("unknown");
        let description = module["description"].as_str().unwrap_or("No description");
        let downloads = module["downloads"].as_u64().unwrap_or(0);
        let published_at = module["published_at"].as_str().unwrap_or("unknown");

        results.push(format!(
            "**{namespace}/{name}/{provider}** v{version}\n\
             Description: {description}\n\
             Downloads: {downloads}\n\
             Published: {published_at}\n\
             Registry: https://registry.terraform.io/modules/{namespace}/{name}/{provider}",
        ));
    }

    Ok(format!(
        "Found {} modules:\n\n{}",
        results.len(),
        results.join("\n\n---\n\n")
    ))
}

async fn terraform_provider(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let namespace = args["namespace"].as_str().ok_or("missing 'namespace'")?;
    let name = args["name"].as_str().ok_or("missing 'name'")?;

    let url = format!(
        "https://registry.terraform.io/v1/providers/{}/{}",
        urlencode(namespace),
        urlencode(name),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if resp.get("errors").is_some() {
        return Err(format!(
            "Provider not found: {namespace}/{name}"
        ));
    }

    let full_name = resp["full-name"]
        .as_str()
        .or_else(|| resp["id"].as_str())
        .unwrap_or("unknown");
    let version = resp["version"].as_str().unwrap_or("unknown");
    let description = resp["description"].as_str().unwrap_or("No description");
    let source = resp["source"].as_str().unwrap_or("unknown");
    let downloads = resp["downloads"].as_u64().unwrap_or(0);
    let tier = resp["tier"].as_str().unwrap_or("community");

    Ok(format!(
        "**{full_name}** v{version}\n\
         Tier: {tier}\n\
         Description: {description}\n\
         Source: {source}\n\
         Downloads: {downloads}\n\
         Registry: https://registry.terraform.io/providers/{namespace}/{name}",
    ))
}

async fn dns_lookup(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let domain = args["domain"].as_str().ok_or("missing 'domain'")?;
    let record_type = args["record_type"].as_str().unwrap_or("A");

    let url = format!(
        "https://dns.google/resolve?name={}&type={}",
        urlencode(domain),
        urlencode(record_type),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let status = resp["Status"].as_u64().unwrap_or(99);
    let status_text = match status {
        0 => "NOERROR",
        1 => "FORMERR",
        2 => "SERVFAIL",
        3 => "NXDOMAIN",
        5 => "REFUSED",
        _ => "UNKNOWN",
    };

    let question_name = resp["Question"]
        .as_array()
        .and_then(|q| q.first())
        .and_then(|q| q["name"].as_str())
        .unwrap_or(domain);

    let mut output = format!(
        "DNS Lookup: {question_name} (type {record_type})\nStatus: {status_text} ({status})\n"
    );

    if let Some(answers) = resp["Answer"].as_array() {
        if answers.is_empty() {
            output.push_str("\nNo records found.");
        } else {
            output.push_str(&format!("\nRecords ({}):\n", answers.len()));
            for answer in answers {
                let name = answer["name"].as_str().unwrap_or("?");
                let rtype = answer["type"].as_u64().unwrap_or(0);
                let ttl = answer["TTL"].as_u64().unwrap_or(0);
                let data = answer["data"].as_str().unwrap_or("?");

                let type_name = match rtype {
                    1 => "A",
                    5 => "CNAME",
                    15 => "MX",
                    16 => "TXT",
                    2 => "NS",
                    28 => "AAAA",
                    _ => "OTHER",
                };

                output.push_str(&format!(
                    "  {name} {type_name} TTL={ttl} -> {data}\n"
                ));
            }
        }
    } else {
        output.push_str("\nNo answer section in response.");
    }

    Ok(output)
}

async fn ip_info(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let ip = args["ip"].as_str().ok_or("missing 'ip'")?;

    let url = format!(
        "http://ip-api.com/json/{}?fields=status,message,country,regionName,city,zip,lat,lon,timezone,isp,org,as,query",
        urlencode(ip),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let status = resp["status"].as_str().unwrap_or("unknown");
    if status == "fail" {
        let message = resp["message"].as_str().unwrap_or("unknown error");
        return Err(format!("IP lookup failed: {message}"));
    }

    let query = resp["query"].as_str().unwrap_or(ip);
    let country = resp["country"].as_str().unwrap_or("unknown");
    let region = resp["regionName"].as_str().unwrap_or("unknown");
    let city = resp["city"].as_str().unwrap_or("unknown");
    let zip = resp["zip"].as_str().unwrap_or("unknown");
    let lat = resp["lat"].as_f64().unwrap_or(0.0);
    let lon = resp["lon"].as_f64().unwrap_or(0.0);
    let timezone = resp["timezone"].as_str().unwrap_or("unknown");
    let isp = resp["isp"].as_str().unwrap_or("unknown");
    let org = resp["org"].as_str().unwrap_or("unknown");
    let as_number = resp["as"].as_str().unwrap_or("unknown");

    Ok(format!(
        "**IP: {query}**\n\
         Country: {country}\n\
         Region: {region}\n\
         City: {city}\n\
         ZIP: {zip}\n\
         Coordinates: {lat}, {lon}\n\
         Timezone: {timezone}\n\
         ISP: {isp}\n\
         Organization: {org}\n\
         AS: {as_number}",
    ))
}

// --- Helpers -----------------------------------------------------------------

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
