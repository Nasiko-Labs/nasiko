use pingora_http::RequestHeader;

/// Request/response translation layer.
/// Handles protocol-level transformations as requests pass through the gateway.
pub struct Translator {
    rules: Vec<TranslationRule>,
}

#[derive(Debug, Clone)]
pub enum TranslationRule {
    /// Add a header to the upstream request
    AddHeader { name: String, value: String },
    /// Remove a header before forwarding
    RemoveHeader { name: String },
    /// Rewrite Content-Type header
    RewriteContentType { from: String, to: String },
}

impl Translator {
    pub fn new(rules: Vec<TranslationRule>) -> Self {
        Self { rules }
    }

    /// Default translator that injects gateway-standard headers.
    pub fn default_translator() -> Self {
        Self::new(vec![
            TranslationRule::AddHeader {
                name: "X-Forwarded-By".into(),
                value: "nasiko-gateway".into(),
            },
            TranslationRule::RemoveHeader {
                name: "x-internal-only".into(),
            },
        ])
    }

    /// Apply translation rules to a request header before proxying upstream.
    pub fn translate_request(&self, header: &mut RequestHeader, client_ip: Option<&str>) {
        // Always add X-Forwarded-For
        if let Some(ip) = client_ip {
            let _ = header.append_header("X-Forwarded-For", ip);
        }

        // Always add X-Request-Id if missing
        if header.headers.get("x-request-id").is_none() {
            let id = simple_request_id();
            let _ = header.append_header("X-Request-Id", id);
        }

        for rule in &self.rules {
            match rule {
                TranslationRule::AddHeader { name, value } => {
                    let name = name.clone();
                    let value = value.clone();
                    let _ = header.append_header(name, value);
                }
                TranslationRule::RemoveHeader { name } => {
                    let _ = header.remove_header(name);
                }
                TranslationRule::RewriteContentType { from, to } => {
                    let rewrite = header
                        .headers
                        .get("content-type")
                        .and_then(|ct| ct.to_str().ok())
                        .is_some_and(|ct| ct == from.as_str());
                    if rewrite {
                        let to = to.clone();
                        let _ = header.insert_header("content-type", to);
                    }
                }
            }
        }
    }
}

fn simple_request_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("gw-{:x}", ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_forwarded_for() {
        let translator = Translator::new(vec![]);
        let mut header = RequestHeader::build("GET", b"/test", None).unwrap();

        translator.translate_request(&mut header, Some("1.2.3.4"));

        assert_eq!(
            header.headers.get("x-forwarded-for").unwrap().to_str().unwrap(),
            "1.2.3.4"
        );
    }

    #[test]
    fn adds_request_id_when_missing() {
        let translator = Translator::new(vec![]);
        let mut header = RequestHeader::build("GET", b"/test", None).unwrap();

        translator.translate_request(&mut header, None);

        let id = header.headers.get("x-request-id").unwrap().to_str().unwrap();
        assert!(id.starts_with("gw-"));
    }

    #[test]
    fn preserves_existing_request_id() {
        let translator = Translator::new(vec![]);
        let mut header = RequestHeader::build("GET", b"/test", None).unwrap();
        header.insert_header("x-request-id", "existing-id".to_string()).unwrap();

        translator.translate_request(&mut header, None);

        assert_eq!(
            header.headers.get("x-request-id").unwrap().to_str().unwrap(),
            "existing-id"
        );
    }

    #[test]
    fn applies_add_header_rule() {
        let translator = Translator::new(vec![TranslationRule::AddHeader {
            name: "X-Custom".into(),
            value: "hello".into(),
        }]);
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();

        translator.translate_request(&mut header, None);

        assert_eq!(
            header.headers.get("x-custom").unwrap().to_str().unwrap(),
            "hello"
        );
    }

    #[test]
    fn applies_remove_header_rule() {
        let translator = Translator::new(vec![TranslationRule::RemoveHeader {
            name: "x-secret".into(),
        }]);
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();
        header.insert_header("x-secret", "should-be-removed".to_string()).unwrap();

        translator.translate_request(&mut header, None);

        assert!(header.headers.get("x-secret").is_none());
    }

    #[test]
    fn rewrites_content_type() {
        let translator = Translator::new(vec![TranslationRule::RewriteContentType {
            from: "text/plain".into(),
            to: "application/json".into(),
        }]);
        let mut header = RequestHeader::build("POST", b"/", None).unwrap();
        header.insert_header("content-type", "text/plain".to_string()).unwrap();

        translator.translate_request(&mut header, None);

        assert_eq!(
            header.headers.get("content-type").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    #[test]
    fn does_not_rewrite_non_matching_content_type() {
        let translator = Translator::new(vec![TranslationRule::RewriteContentType {
            from: "text/plain".into(),
            to: "application/json".into(),
        }]);
        let mut header = RequestHeader::build("POST", b"/", None).unwrap();
        header.insert_header("content-type", "application/xml".to_string()).unwrap();

        translator.translate_request(&mut header, None);

        assert_eq!(
            header.headers.get("content-type").unwrap().to_str().unwrap(),
            "application/xml"
        );
    }

    #[test]
    fn default_translator_adds_forwarded_by() {
        let translator = Translator::default_translator();
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();

        translator.translate_request(&mut header, None);

        assert_eq!(
            header.headers.get("x-forwarded-by").unwrap().to_str().unwrap(),
            "nasiko-gateway"
        );
    }
}
