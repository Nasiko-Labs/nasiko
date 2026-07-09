use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "sec_filing_search",
                "description": "Full-text search of SEC EDGAR filings. Search for keywords across all SEC filings including 10-K, 10-Q, 8-K, and more. Useful for finding disclosures, risk factors, financial statements, and regulatory filings.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for full-text search across SEC filings"
                        },
                        "form_type": {
                            "type": "string",
                            "description": "Filter by form type (e.g. '10-K', '10-Q', '8-K', 'S-1', 'DEF 14A')"
                        },
                        "start_date": {
                            "type": "string",
                            "description": "Start date filter in YYYY-MM-DD format"
                        },
                        "end_date": {
                            "type": "string",
                            "description": "End date filter in YYYY-MM-DD format"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "sec_company_filings",
                "description": "Get recent SEC filings for a specific company by CIK number. Returns the company's most recent filings including form type, date, and description. CIK numbers can be found on the SEC EDGAR website.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cik": {
                            "type": "string",
                            "description": "Company CIK number (will be zero-padded to 10 digits automatically)"
                        }
                    },
                    "required": ["cik"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "sec_company_search",
                "description": "Look up a public company's SEC CIK number by name or ticker. Use this before sec_company_filings, which requires a CIK rather than a company name.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Company name or ticker symbol to search for (e.g. 'Apple', 'AAPL')"
                        }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "federal_register",
                "description": "Search the U.S. Federal Register for regulations, proposed rules, notices, and presidential documents. The Federal Register is the official daily publication for rules, proposed rules, and notices of federal agencies.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for Federal Register documents"
                        },
                        "document_type": {
                            "type": "string",
                            "enum": ["RULE", "PRRULE", "NOTICE", "PRESDOCU"],
                            "description": "Filter by document type: RULE (final rules), PRRULE (proposed rules), NOTICE (notices), PRESDOCU (presidential documents)"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "case_law_search",
                "description": "Search for court opinions and case law via CourtListener. Find judicial opinions from federal and state courts including the Supreme Court, Circuit Courts, and District Courts.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for case law (supports natural language and legal citations)"
                        },
                        "court": {
                            "type": "string",
                            "description": "Filter by court identifier (e.g. 'scotus' for Supreme Court, 'ca1'-'ca11' for Circuit Courts)"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "sec_filing_search" => sec_filing_search(arguments).await,
        "sec_company_filings" => sec_company_filings(arguments).await,
        "sec_company_search" => sec_company_search(arguments).await,
        "federal_register" => federal_register(arguments).await,
        "case_law_search" => case_law_search(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("nasiko-legal-agent/1.0 (research@nasiko.io)")
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))
}

async fn sec_filing_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let form_type = args["form_type"].as_str();
    let start_date = args["start_date"].as_str();
    let end_date = args["end_date"].as_str();

    let mut url = format!(
        "https://efts.sec.gov/LATEST/search-index?q={}",
        urlencode(query),
    );

    if let Some(form) = form_type {
        url.push_str(&format!("&forms={}", urlencode(form)));
    }

    if start_date.is_some() || end_date.is_some() {
        url.push_str("&dateRange=custom");
        if let Some(start) = start_date {
            url.push_str(&format!("&startdt={}", urlencode(start)));
        }
        if let Some(end) = end_date {
            url.push_str(&format!("&enddt={}", urlencode(end)));
        }
    }

    let client = build_client()?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let hits = resp["hits"]["hits"]
        .as_array()
        .ok_or("no results in response")?;

    if hits.is_empty() {
        return Ok(format!("No SEC filings found for '{query}'."));
    }

    let mut results = Vec::new();
    for hit in hits.iter().take(10) {
        let source = &hit["_source"];
        let filing_date = source["file_date"].as_str().unwrap_or("unknown date");
        let form = source["form_type"].as_str().unwrap_or("unknown");
        let company = source["entity_name"].as_str().unwrap_or("Unknown Company");
        let description = source["file_description"]
            .as_str()
            .or_else(|| source["display_names"].as_array().and_then(|a| a.first()?.as_str()))
            .unwrap_or("");
        let file_num = source["file_num"].as_str().unwrap_or("");
        let accession = hit["_id"].as_str().unwrap_or("");

        let link = if !accession.is_empty() {
            let clean = accession.replace('-', "");
            format!(
                "https://www.sec.gov/Archives/edgar/data/{}/{}",
                file_num.split('-').next().unwrap_or(""),
                clean
            )
        } else {
            String::new()
        };

        let mut entry = format!(
            "**{company}** - Form {form}\nFiled: {filing_date}",
        );

        if !description.is_empty() {
            entry.push_str(&format!("\nDescription: {description}"));
        }

        if !link.is_empty() {
            entry.push_str(&format!("\nLink: {link}"));
        }

        results.push(entry);
    }

    Ok(format!(
        "Found {} SEC filings:\n\n{}",
        results.len(),
        results.join("\n\n---\n\n")
    ))
}

async fn sec_company_filings(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let cik = args["cik"].as_str().ok_or("missing 'cik'")?;

    // Zero-pad CIK to 10 digits
    let cik_numeric: u64 = cik
        .trim()
        .parse()
        .map_err(|_| format!("invalid CIK number: '{cik}'"))?;
    let padded_cik = format!("{:010}", cik_numeric);

    let url = format!("https://data.sec.gov/submissions/CIK{padded_cik}.json");

    let client = build_client()?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let company_name = resp["name"].as_str().unwrap_or("Unknown Company");
    let cik_display = resp["cik"].as_str().unwrap_or(&padded_cik);
    let tickers = resp["tickers"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let recent = &resp["filings"]["recent"];
    let forms = recent["form"].as_array();
    let dates = recent["filingDate"].as_array();
    let descriptions = recent["primaryDocDescription"].as_array();
    let accession_numbers = recent["accessionNumber"].as_array();

    let mut filings = Vec::new();

    if let (Some(forms), Some(dates)) = (forms, dates) {
        let count = forms.len().min(10);
        for i in 0..count {
            let form = forms[i].as_str().unwrap_or("?");
            let date = dates[i].as_str().unwrap_or("?");
            let desc = descriptions
                .and_then(|d| d.get(i))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let accession = accession_numbers
                .and_then(|a| a.get(i))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let link = if !accession.is_empty() {
                format!(
                    "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={}&type={}&dateb=&owner=include&count=1",
                    cik_display, urlencode(form)
                )
            } else {
                String::new()
            };

            let mut entry = format!("  {form} | {date}");
            if !desc.is_empty() {
                entry.push_str(&format!(" | {desc}"));
            }
            if !link.is_empty() {
                entry.push_str(&format!("\n    {link}"));
            }
            filings.push(entry);
        }
    }

    let mut output = format!("**{company_name}**\nCIK: {cik_display}");
    if !tickers.is_empty() {
        output.push_str(&format!("\nTickers: {tickers}"));
    }

    if filings.is_empty() {
        output.push_str("\n\nNo recent filings found.");
    } else {
        output.push_str(&format!(
            "\n\nRecent Filings ({}):\n{}",
            filings.len(),
            filings.join("\n")
        ));
    }

    Ok(output)
}

async fn sec_company_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let name = args["name"].as_str().ok_or("missing 'name'")?;
    let needle = name.to_lowercase();

    // SEC publishes a single static JSON file mapping every registered ticker
    // to its CIK — no search API needed, just fetch once and filter locally.
    let client = build_client()?;
    let resp = client
        .get("https://www.sec.gov/files/company_tickers.json")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let entries = resp
        .as_object()
        .ok_or("unexpected response shape")?
        .values();

    let mut matches: Vec<(String, String, String)> = Vec::new();
    for entry in entries {
        let title = entry["title"].as_str().unwrap_or("");
        let ticker = entry["ticker"].as_str().unwrap_or("");
        if title.to_lowercase().contains(&needle) || ticker.to_lowercase() == needle {
            let cik = entry["cik_str"].as_u64().map(|c| format!("{c:010}")).unwrap_or_default();
            if !cik.is_empty() {
                matches.push((title.to_string(), ticker.to_string(), cik));
            }
        }
        if matches.len() >= 10 {
            break;
        }
    }

    if matches.is_empty() {
        return Ok(format!("No SEC-registered company found matching '{name}'."));
    }

    let results: Vec<String> = matches
        .iter()
        .map(|(title, ticker, cik)| format!("**{title}** ({ticker}) — CIK: {cik}"))
        .collect();

    Ok(format!(
        "Found {} matching companies:\n\n{}",
        results.len(),
        results.join("\n")
    ))
}

async fn federal_register(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let document_type = args["document_type"].as_str();

    let mut url = format!(
        "https://www.federalregister.gov/api/v1/documents.json?conditions[term]={}&per_page=5&order=newest",
        urlencode(query),
    );

    if let Some(doc_type) = document_type {
        url.push_str(&format!("&conditions[type][]={}", urlencode(doc_type)));
    }

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let results_arr = resp["results"]
        .as_array()
        .ok_or("no results in response")?;

    if results_arr.is_empty() {
        return Ok(format!("No Federal Register documents found for '{query}'."));
    }

    let mut results = Vec::new();
    for doc in results_arr {
        let title = doc["title"].as_str().unwrap_or("Untitled");
        let doc_type = doc["type"].as_str().unwrap_or("unknown");
        let pub_date = doc["publication_date"].as_str().unwrap_or("unknown date");
        let abstract_text = doc["abstract"].as_str().unwrap_or("");
        let html_url = doc["html_url"].as_str().unwrap_or("");

        let agencies: Vec<&str> = doc["agencies"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
            .unwrap_or_default();

        let abstract_display = if abstract_text.len() > 300 {
            format!("{}...", &abstract_text[..300])
        } else {
            abstract_text.to_string()
        };

        let mut entry = format!(
            "**{title}**\nType: {doc_type} | Published: {pub_date}",
        );

        if !agencies.is_empty() {
            entry.push_str(&format!("\nAgencies: {}", agencies.join(", ")));
        }

        if !abstract_display.is_empty() {
            entry.push_str(&format!("\nAbstract: {abstract_display}"));
        }

        if !html_url.is_empty() {
            entry.push_str(&format!("\nURL: {html_url}"));
        }

        results.push(entry);
    }

    Ok(format!(
        "Found {} Federal Register documents:\n\n{}",
        results.len(),
        results.join("\n\n---\n\n")
    ))
}

async fn case_law_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let court = args["court"].as_str();

    let mut url = format!(
        "https://www.courtlistener.com/api/rest/v3/search/?q={}&type=o&order_by=score+desc",
        urlencode(query),
    );

    if let Some(court_id) = court {
        url.push_str(&format!("&court={}", urlencode(court_id)));
    }

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let results_arr = resp["results"]
        .as_array()
        .ok_or("no results in response")?;

    if results_arr.is_empty() {
        return Ok(format!("No case law found for '{query}'."));
    }

    let mut results = Vec::new();
    for case in results_arr.iter().take(10) {
        let case_name = case["caseName"]
            .as_str()
            .or_else(|| case["case_name"].as_str())
            .unwrap_or("Unknown Case");
        let court_name = case["court_citation_string"]
            .as_str()
            .or_else(|| case["court"].as_str())
            .unwrap_or("unknown court");
        let date_filed = case["dateFiled"]
            .as_str()
            .or_else(|| case["date_filed"].as_str())
            .unwrap_or("unknown date");
        let citation = case["citation"]
            .as_array()
            .and_then(|c| c.first())
            .and_then(|v| v.as_str())
            .or_else(|| case["citation"].as_str())
            .unwrap_or("");
        let snippet = case["snippet"]
            .as_str()
            .or_else(|| case["text"].as_str())
            .unwrap_or("");

        // Clean HTML tags from snippet
        let snippet_clean = snippet
            .replace("<mark>", "")
            .replace("</mark>", "")
            .replace("<em>", "")
            .replace("</em>", "");

        let snippet_display = if snippet_clean.len() > 300 {
            format!("{}...", &snippet_clean[..300])
        } else {
            snippet_clean
        };

        let absolute_url = case["absolute_url"].as_str().unwrap_or("");
        let link = if !absolute_url.is_empty() {
            format!("https://www.courtlistener.com{absolute_url}")
        } else {
            String::new()
        };

        let mut entry = format!(
            "**{case_name}**\nCourt: {court_name} | Date Filed: {date_filed}",
        );

        if !citation.is_empty() {
            entry.push_str(&format!("\nCitation: {citation}"));
        }

        if !snippet_display.is_empty() {
            entry.push_str(&format!("\nSnippet: {snippet_display}"));
        }

        if !link.is_empty() {
            entry.push_str(&format!("\nLink: {link}"));
        }

        results.push(entry);
    }

    Ok(format!(
        "Found {} cases:\n\n{}",
        results.len(),
        results.join("\n\n---\n\n")
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
