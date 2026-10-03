//! Response parsing for hosted LLM classification.
//!
//! This module provides parsing of LLM responses in the expected format:
//! ```text
//! TYPE: <RequestType>
//! CONFIDENCE: <0.0-1.0>
//! REASONING: <explanation>
//! ```

use crate::classifier::ClassifierError;
use crate::routing::RequestType;
use regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref TYPE_REGEX: Regex = Regex::new(r"TYPE:\s*(\w+)").unwrap();
    static ref CONFIDENCE_REGEX: Regex = Regex::new(r"CONFIDENCE:\s*([\d.]+)").unwrap();
    static ref REASONING_REGEX: Regex = Regex::new(r"REASONING:\s*(.+?)(?:\n\n|\z)").unwrap();
}

/// Parse LLM classification response into structured data.
///
/// # Arguments
///
/// * `response` - The raw LLM response text
///
/// # Returns
///
/// * `Ok((RequestType, f32, Option<String>))` - Parsed type, confidence, and reasoning
/// * `Err(ClassifierError)` - Parsing failed
///
/// # Examples
///
/// ```ignore
/// let response = "TYPE: CodeGeneration\nCONFIDENCE: 0.95\nREASONING: Request asks to create code";
/// let (req_type, confidence, reasoning) = parse_classification_response(response)?;
/// assert_eq!(confidence, 0.95);
/// ```
pub fn parse_classification_response(
    response: &str,
) -> Result<(RequestType, f32, Option<String>), ClassifierError> {
    // Extract TYPE field
    let type_str = TYPE_REGEX
        .captures(response)
        .and_then(|c| c.get(1))
        .ok_or_else(|| ClassifierError::ParseError("Missing TYPE field".into()))?
        .as_str();

    // Parse RequestType - handle various formats
    let request_type = match type_str.to_lowercase().replace("_", "").as_str() {
        "codegeneration" => RequestType::CodeGeneration,
        "codeunderstanding" => RequestType::CodeUnderstanding,
        "technicaldesign" => RequestType::TechnicalDesign,
        "analyticalreasoning" => RequestType::AnalyticalReasoning,
        "writing" => RequestType::Writing,
        "factuallookup" => RequestType::FactualLookup,
        "general" => RequestType::General,
        _ => return Err(ClassifierError::ParseError(
            format!("Invalid RequestType: {}. Must be one of: CodeGeneration, CodeUnderstanding, TechnicalDesign, AnalyticalReasoning, Writing, FactualLookup, General", type_str)
        )),
    };

    // Extract CONFIDENCE field
    let confidence_str = CONFIDENCE_REGEX
        .captures(response)
        .and_then(|c| c.get(1))
        .ok_or_else(|| ClassifierError::ParseError("Missing CONFIDENCE field".into()))?
        .as_str();

    let confidence: f32 = confidence_str
        .parse()
        .map_err(|e| ClassifierError::ParseError(format!("Invalid confidence number: {}", e)))?;

    // Validate confidence range
    if !(0.0..=1.0).contains(&confidence) {
        return Err(ClassifierError::ParseError(
            format!("Confidence {} out of range [0.0, 1.0]", confidence)
        ));
    }

    // Extract REASONING field (optional)
    let reasoning = REASONING_REGEX
        .captures(response)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string());

    Ok((request_type, confidence, reasoning))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_response() {
        let response = "TYPE: CodeGeneration\nCONFIDENCE: 0.95\nREASONING: Request asks to create code";
        let (req_type, confidence, reasoning) = parse_classification_response(response).unwrap();
        
        assert!(matches!(req_type, RequestType::CodeGeneration));
        assert_eq!(confidence, 0.95);
        assert!(reasoning.is_some());
    }

    #[test]
    fn test_parse_with_underscore() {
        let response = "TYPE: code_generation\nCONFIDENCE: 0.90\nREASONING: Test";
        let (req_type, _, _) = parse_classification_response(response).unwrap();
        assert!(matches!(req_type, RequestType::CodeGeneration));
    }

    #[test]
    fn test_parse_missing_type() {
        let response = "CONFIDENCE: 0.95\nREASONING: Missing type";
        let result = parse_classification_response(response);
        assert!(matches!(result, Err(ClassifierError::ParseError(_))));
    }

    #[test]
    fn test_parse_missing_confidence() {
        let response = "TYPE: CodeGeneration\nREASONING: Missing confidence";
        let result = parse_classification_response(response);
        assert!(matches!(result, Err(ClassifierError::ParseError(_))));
    }

    #[test]
    fn test_parse_invalid_confidence_range() {
        let response = "TYPE: CodeGeneration\nCONFIDENCE: 1.5\nREASONING: Out of range";
        let result = parse_classification_response(response);
        assert!(matches!(result, Err(ClassifierError::ParseError(_))));
    }

    #[test]
    fn test_parse_invalid_request_type() {
        let response = "TYPE: InvalidType\nCONFIDENCE: 0.9\nREASONING: Test";
        let result = parse_classification_response(response);
        assert!(matches!(result, Err(ClassifierError::ParseError(_))));
    }

    #[test]
    fn test_parse_without_reasoning() {
        let response = "TYPE: General\nCONFIDENCE: 0.5";
        let (_, _, reasoning) = parse_classification_response(response).unwrap();
        assert!(reasoning.is_none());
    }
}
