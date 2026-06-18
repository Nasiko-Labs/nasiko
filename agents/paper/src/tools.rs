use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "arxiv_search",
                "description": "Search arXiv for academic papers. Returns titles, authors, abstracts, and links. Use for finding research papers on any scientific or technical topic.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (supports arXiv query syntax: ti:, au:, abs:, cat:)"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Number of results to return (1-10)",
                            "default": 5
                        },
                        "sort_by": {
                            "type": "string",
                            "enum": ["relevance", "lastUpdatedDate", "submittedDate"],
                            "default": "relevance"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "semantic_scholar",
                "description": "Search Semantic Scholar for papers with citation counts, influential citations, and related work. Better for finding highly-cited or influential papers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for papers"
                        },
                        "year": {
                            "type": "string",
                            "description": "Filter by year or range (e.g. '2024' or '2022-2024')"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Number of results (1-10)",
                            "default": 5
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "semantic_scholar_paper",
                "description": "Get detailed info about a specific paper by its Semantic Scholar ID, arXiv ID, or DOI. Returns abstract, citations, references, and TL;DR.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "paper_id": {
                            "type": "string",
                            "description": "Paper identifier: Semantic Scholar ID, 'arXiv:2301.00001', or 'DOI:10.xxx/yyy'"
                        }
                    },
                    "required": ["paper_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "wikipedia_summary",
                "description": "Get a Wikipedia summary for a topic. Useful for providing context, definitions, or background on concepts mentioned in papers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "Topic to look up"
                        }
                    },
                    "required": ["topic"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "arxiv_search" => arxiv_search(arguments).await,
        "semantic_scholar" => semantic_scholar_search(arguments).await,
        "semantic_scholar_paper" => semantic_scholar_paper(arguments).await,
        "wikipedia_summary" => wikipedia_summary(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

async fn arxiv_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let max_results = args["max_results"].as_u64().unwrap_or(5).min(10);
    let sort_by = args["sort_by"].as_str().unwrap_or("relevance");

    let url = format!(
        "http://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}&sortBy={}&sortOrder=descending",
        urlencode(query),
        max_results,
        sort_by,
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read failed: {e}"))?;

    // Parse the Atom XML response
    let mut results = Vec::new();
    let mut in_entry = false;
    let mut current_title = String::new();
    let mut current_summary = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_id = String::new();
    let mut current_published = String::new();
    let mut current_tag = String::new();

    for line in resp.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<entry>") {
            in_entry = true;
            current_title.clear();
            current_summary.clear();
            current_authors.clear();
            current_id.clear();
            current_published.clear();
        } else if trimmed.starts_with("</entry>") && in_entry {
            in_entry = false;
            let abstract_short = if current_summary.len() > 300 {
                format!("{}...", &current_summary[..300])
            } else {
                current_summary.clone()
            };
            results.push(format!(
                "**{}**\nAuthors: {}\nDate: {}\nLink: {}\nAbstract: {}\n",
                current_title.trim(),
                current_authors.join(", "),
                &current_published[..10.min(current_published.len())],
                current_id.trim(),
                abstract_short.trim(),
            ));
        } else if in_entry {
            if trimmed.starts_with("<id>") {
                current_id = extract_tag_content(trimmed, "id");
            } else if trimmed.starts_with("<published>") {
                current_published = extract_tag_content(trimmed, "published");
            } else if trimmed.starts_with("<title>") {
                current_tag = "title".into();
                current_title = extract_tag_content(trimmed, "title");
            } else if trimmed.starts_with("<summary>") {
                current_tag = "summary".into();
                current_summary = extract_tag_content(trimmed, "summary");
            } else if trimmed.starts_with("<name>") {
                current_authors.push(extract_tag_content(trimmed, "name"));
            } else if trimmed.starts_with("</title>") {
                current_tag.clear();
            } else if trimmed.starts_with("</summary>") {
                current_tag.clear();
            } else if current_tag == "title" {
                current_title.push(' ');
                current_title.push_str(trimmed);
            } else if current_tag == "summary" {
                current_summary.push(' ');
                current_summary.push_str(trimmed);
            }
        }
    }

    if results.is_empty() {
        Ok(format!("No arXiv papers found for '{query}'."))
    } else {
        Ok(format!("Found {} papers:\n\n{}", results.len(), results.join("\n---\n")))
    }
}

async fn semantic_scholar_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let limit = args["limit"].as_u64().unwrap_or(5).min(10);
    let year = args["year"].as_str();

    let mut url = format!(
        "https://api.semanticscholar.org/graph/v1/paper/search?query={}&limit={}&fields=title,authors,year,citationCount,influentialCitationCount,tldr,externalIds",
        urlencode(query),
        limit,
    );

    if let Some(y) = year {
        url.push_str(&format!("&year={}", y));
    }

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let papers = resp["data"].as_array().ok_or("no results")?;

    if papers.is_empty() {
        return Ok(format!("No papers found for '{query}'."));
    }

    let mut results = Vec::new();
    for paper in papers {
        let title = paper["title"].as_str().unwrap_or("Untitled");
        let year = paper["year"].as_u64().map(|y| y.to_string()).unwrap_or_default();
        let citations = paper["citationCount"].as_u64().unwrap_or(0);
        let influential = paper["influentialCitationCount"].as_u64().unwrap_or(0);
        let paper_id = paper["paperId"].as_str().unwrap_or("");
        let tldr = paper["tldr"]["text"].as_str().unwrap_or("");

        let authors: Vec<&str> = paper["authors"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
            .unwrap_or_default();

        let arxiv_id = paper["externalIds"]["ArXiv"].as_str();

        let mut entry = format!(
            "**{title}** ({year})\nAuthors: {}\nCitations: {citations} ({influential} influential)\nID: {paper_id}",
            authors.join(", ")
        );

        if let Some(aid) = arxiv_id {
            entry.push_str(&format!("\narXiv: https://arxiv.org/abs/{aid}"));
        }

        if !tldr.is_empty() {
            entry.push_str(&format!("\nTL;DR: {tldr}"));
        }

        results.push(entry);
    }

    Ok(format!("Found {} papers:\n\n{}", results.len(), results.join("\n\n---\n\n")))
}

async fn semantic_scholar_paper(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let paper_id = args["paper_id"].as_str().ok_or("missing 'paper_id'")?;

    let url = format!(
        "https://api.semanticscholar.org/graph/v1/paper/{}?fields=title,authors,year,abstract,citationCount,influentialCitationCount,references.title,references.year,tldr,externalIds,venue",
        urlencode(paper_id),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if resp.get("error").is_some() {
        return Err(format!("Paper not found: {}", resp["error"]));
    }

    let title = resp["title"].as_str().unwrap_or("Untitled");
    let year = resp["year"].as_u64().map(|y| y.to_string()).unwrap_or_default();
    let venue = resp["venue"].as_str().unwrap_or("");
    let abstract_text = resp["abstract"].as_str().unwrap_or("No abstract available.");
    let citations = resp["citationCount"].as_u64().unwrap_or(0);
    let influential = resp["influentialCitationCount"].as_u64().unwrap_or(0);
    let tldr = resp["tldr"]["text"].as_str().unwrap_or("");

    let authors: Vec<&str> = resp["authors"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
        .unwrap_or_default();

    let references: Vec<String> = resp["references"]
        .as_array()
        .map(|refs| {
            refs.iter()
                .take(10)
                .filter_map(|r| {
                    let t = r["title"].as_str()?;
                    let y = r["year"].as_u64().map(|y| format!(" ({y})")).unwrap_or_default();
                    Some(format!("  - {t}{y}"))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut output = format!(
        "**{title}** ({year})\nVenue: {venue}\nAuthors: {}\nCitations: {citations} ({influential} influential)",
        authors.join(", ")
    );

    if !tldr.is_empty() {
        output.push_str(&format!("\n\nTL;DR: {tldr}"));
    }

    output.push_str(&format!("\n\nAbstract:\n{abstract_text}"));

    if !references.is_empty() {
        output.push_str(&format!("\n\nKey References:\n{}", references.join("\n")));
    }

    Ok(output)
}

async fn wikipedia_summary(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let topic = args["topic"].as_str().ok_or("missing 'topic'")?;

    let url = format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
        urlencode(topic),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if resp["type"].as_str() == Some("https://mediawiki.org/wiki/HyperSwitch/errors/not_found") {
        return Ok(format!("No Wikipedia article found for '{topic}'."));
    }

    let title = resp["title"].as_str().unwrap_or(topic);
    let extract = resp["extract"].as_str().unwrap_or("No summary available.");
    let page_url = resp["content_urls"]["desktop"]["page"].as_str().unwrap_or("");

    Ok(format!("**{title}**\n\n{extract}\n\nSource: {page_url}"))
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

fn extract_tag_content(line: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = line.find(&open) {
        let after_open = start + open.len();
        if let Some(end) = line[after_open..].find(&close) {
            return line[after_open..after_open + end].to_string();
        }
        return line[after_open..].to_string();
    }
    String::new()
}
