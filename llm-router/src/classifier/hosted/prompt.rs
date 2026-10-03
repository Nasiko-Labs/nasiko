//! Prompt construction for hosted LLM classification.
//!
//! This module provides the [`PromptBuilder`] which constructs structured prompts
//! for LLM-based request classification. It supports both zero-shot and few-shot
//! prompting strategies.
//!
//! # Examples
//!
//! ```
//! use nasiko_llm_router::classifier::hosted::prompt::{PromptBuilder, PromptMode};
//! use nasiko_llm_router::classifier::InferenceContext;
//! use std::collections::HashMap;
//! use std::time::Instant;
//!
//! let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
//! let context = InferenceContext {
//!     request_text: "Write a function to sort an array".into(),
//!     user_context: None,
//!     conversation_history: None,
//!     metadata: HashMap::new(),
//!     received_at: Instant::now(),
//! };
//!
//! let prompt = builder.build_prompt(&context).unwrap();
//! assert!(prompt.contains("RequestType"));
//! ```

use crate::classifier::types::{ClassifierError, InferenceContext};
use crate::routing::RequestType;
use serde::{Deserialize, Serialize};

/// Prompting strategy for classification.
///
/// Determines whether the LLM receives only instructions (zero-shot)
/// or also example classifications (few-shot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptMode {
    /// Zero-shot prompting: LLM receives only instructions and the request to classify.
    ///
    /// This mode relies on the LLM's pre-trained knowledge and is faster since
    /// prompts are shorter. Best for well-defined classification tasks.
    ZeroShot,

    /// Few-shot prompting: LLM receives instructions plus example classifications.
    ///
    /// This mode provides demonstrations of correct classifications, which can
    /// improve accuracy at the cost of longer prompts and slightly higher latency.
    FewShot,
}

/// A single example for few-shot prompting.
///
/// Demonstrates a request text, its correct classification, and an explanation
/// of why that classification is appropriate.
///
/// # Examples
///
/// ```
/// use nasiko_llm_router::classifier::hosted::prompt::FewShotExample;
/// use nasiko_llm_router::routing::RequestType;
///
/// let example = FewShotExample {
///     request_text: "Create a REST API endpoint for user authentication".into(),
///     request_type: RequestType::CodeGeneration,
///     explanation: "Request asks to create new code functionality".into(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FewShotExample {
    /// The example request text.
    pub request_text: String,

    /// The correct classification for this request.
    pub request_type: RequestType,

    /// Explanation of why this classification is correct.
    ///
    /// Should be concise and focus on the key features that determined
    /// the classification.
    pub explanation: String,
}

/// Builds classification prompts with configurable strategies.
///
/// The `PromptBuilder` constructs structured prompts for LLM-based classification.
/// It handles:
/// - System instructions and classification task description
/// - RequestType enum documentation
/// - Few-shot examples (when in FewShot mode)
/// - Conversation history context (limited to last 5 turns)
/// - Output format specification
///
/// # Examples
///
/// ## Zero-shot classification
///
/// ```
/// use nasiko_llm_router::classifier::hosted::prompt::{PromptBuilder, PromptMode};
/// use nasiko_llm_router::classifier::InferenceContext;
/// use std::collections::HashMap;
/// use std::time::Instant;
///
/// let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
///
/// let context = InferenceContext {
///     request_text: "Explain how binary search works".into(),
///     user_context: None,
///     conversation_history: None,
///     metadata: HashMap::new(),
///     received_at: Instant::now(),
/// };
///
/// let prompt = builder.build_prompt(&context).unwrap();
/// assert!(prompt.contains("Explain how binary search works"));
/// ```
///
/// ## Few-shot classification
///
/// ```
/// use nasiko_llm_router::classifier::hosted::prompt::{
///     PromptBuilder, PromptMode, FewShotExample
/// };
/// use nasiko_llm_router::classifier::InferenceContext;
/// use nasiko_llm_router::routing::RequestType;
/// use std::collections::HashMap;
/// use std::time::Instant;
///
/// let examples = vec![
///     FewShotExample {
///         request_text: "Write a function to parse JSON".into(),
///         request_type: RequestType::CodeGeneration,
///         explanation: "Asks to create new code".into(),
///     },
/// ];
///
/// let builder = PromptBuilder::new(PromptMode::FewShot, examples);
///
/// let context = InferenceContext {
///     request_text: "Create a sorting algorithm".into(),
///     user_context: None,
///     conversation_history: None,
///     metadata: HashMap::new(),
///     received_at: Instant::now(),
/// };
///
/// let prompt = builder.build_prompt(&context).unwrap();
/// assert!(prompt.contains("Examples"));
/// ```
pub struct PromptBuilder {
    mode: PromptMode,
    examples: Vec<FewShotExample>,
}

impl PromptBuilder {
    /// Create a new `PromptBuilder` with the specified mode and examples.
    ///
    /// # Arguments
    ///
    /// * `mode` - The prompting strategy (zero-shot or few-shot)
    /// * `examples` - Few-shot examples (ignored in zero-shot mode)
    ///
    /// # Examples
    ///
    /// ```
    /// use nasiko_llm_router::classifier::hosted::prompt::{PromptBuilder, PromptMode};
    ///
    /// let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
    /// ```
    pub fn new(mode: PromptMode, examples: Vec<FewShotExample>) -> Self {
        Self { mode, examples }
    }

    /// Build a classification prompt for the given inference context.
    ///
    /// Constructs a complete prompt including:
    /// 1. System instructions
    /// 2. RequestType documentation
    /// 3. Few-shot examples (if in FewShot mode, limited to 5)
    /// 4. Output format specification
    /// 5. The actual request to classify
    /// 6. Conversation history (if present, limited to last 5 turns)
    ///
    /// # Arguments
    ///
    /// * `context` - The inference context containing the request to classify
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - The constructed prompt
    /// * `Err(ClassifierError)` - If prompt construction fails
    ///
    /// # Examples
    ///
    /// ```
    /// use nasiko_llm_router::classifier::hosted::prompt::{PromptBuilder, PromptMode};
    /// use nasiko_llm_router::classifier::InferenceContext;
    /// use std::collections::HashMap;
    /// use std::time::Instant;
    ///
    /// let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
    ///
    /// let context = InferenceContext {
    ///     request_text: "Debug this memory leak".into(),
    ///     user_context: None,
    ///     conversation_history: None,
    ///     metadata: HashMap::new(),
    ///     received_at: Instant::now(),
    /// };
    ///
    /// let prompt = builder.build_prompt(&context).unwrap();
    /// assert!(prompt.contains("Debug this memory leak"));
    /// ```
    pub fn build_prompt(&self, context: &InferenceContext) -> Result<String, ClassifierError> {
        let mut prompt = String::new();

        // System instructions
        prompt.push_str(
            "You are a request classifier. Your task is to categorize incoming requests \
             into one of the predefined RequestType categories.\n\n",
        );

        // Add RequestType documentation
        prompt.push_str("# RequestType Categories\n\n");
        prompt.push_str(&self.get_request_type_documentation());
        prompt.push_str("\n\n");

        // Add few-shot examples if in few-shot mode
        if matches!(self.mode, PromptMode::FewShot) && !self.examples.is_empty() {
            prompt.push_str("# Examples\n\n");

            // Limit to 5 examples as per requirement 2.5
            let examples_to_use = self.examples.iter().take(5);

            for (i, example) in examples_to_use.enumerate() {
                prompt.push_str(&format!(
                    "Example {}:\n\
                     Request: {}\n\
                     Type: {}\n\
                     Reason: {}\n\n",
                    i + 1,
                    example.request_text,
                    example.request_type.as_str(),
                    example.explanation
                ));
            }
        }

        // Add classification instructions
        prompt.push_str("# Classification Task\n\n");
        prompt.push_str("Classify the following request and respond in this exact format:\n");
        prompt.push_str("TYPE: <RequestType>\n");
        prompt.push_str("CONFIDENCE: <0.0-1.0>\n");
        prompt.push_str("REASONING: <brief explanation>\n\n");

        // Add conversation history if available (limit to last 5 turns as per requirement 1.6)
        if let Some(history) = &context.conversation_history {
            if !history.is_empty() {
                prompt.push_str("# Conversation History\n\n");
                prompt.push_str(
                    "The following conversation history provides context for the request:\n\n",
                );

                // Take last 5 turns
                let turns_to_include = if history.len() > 5 {
                    &history[history.len() - 5..]
                } else {
                    history.as_slice()
                };

                for turn in turns_to_include {
                    prompt.push_str(&format!("{}: {}\n", turn.role, turn.content));
                }
                prompt.push_str("\n");
            }
        }

        // Add the actual request to classify
        prompt.push_str("# Request to Classify\n\n");
        prompt.push_str(&context.request_text);
        prompt.push_str("\n");

        Ok(prompt)
    }

    /// Get documentation for RequestType enum variants.
    ///
    /// Returns a formatted string describing each RequestType variant with
    /// examples of when it should be used.
    ///
    /// This documentation is included in every prompt to help the LLM understand
    /// the classification categories.
    fn get_request_type_documentation(&self) -> String {
        // Detailed documentation for each RequestType variant
        // Based on the enum definition in routing/classifier.rs
        r#"The following RequestType categories are available:

1. **CodeGeneration**: Requests to create, write, or generate new code
   - Examples: "Write a function to...", "Create a class for...", "Generate code that..."
   - Includes: implementing new features, writing scripts, creating modules

2. **CodeUnderstanding**: Requests to explain, analyze, or understand existing code
   - Examples: "Explain this function", "What does this code do?", "How does this work?"
   - Includes: code reviews, documentation requests, tracing execution flow

3. **TechnicalDesign**: Requests for architecture, design patterns, or system planning
   - Examples: "Design a system for...", "What's the best architecture for...", "How should I structure..."
   - Includes: API design, database schema design, system architecture

4. **AnalyticalReasoning**: Requests requiring complex analysis, problem-solving, or algorithmic thinking
   - Examples: "How do I optimize...", "What's the most efficient way to...", "Analyze the complexity of..."
   - Includes: algorithm selection, performance optimization, complexity analysis

5. **Writing**: Requests to write or edit non-code text content
   - Examples: "Write documentation for...", "Create a README", "Draft an email about..."
   - Includes: documentation, comments, technical writing, communication

6. **FactualLookup**: Requests for factual information, definitions, or reference material
   - Examples: "What is...", "Define...", "Look up the syntax for..."
   - Includes: API references, language syntax, library documentation lookups

7. **General**: Catch-all for requests that don't fit other categories
   - Examples: General questions, conversational requests, unclear intents
   - Includes: greetings, clarification requests, off-topic questions
"#
        .trim()
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::types::ConversationTurn;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::time::Instant;

    fn create_test_context(request_text: &str) -> InferenceContext {
        InferenceContext {
            request_text: request_text.to_string(),
            user_context: None,
            conversation_history: None,
            metadata: HashMap::new(),
            received_at: Instant::now(),
        }
    }

    #[test]
    fn test_zero_shot_prompt_contains_system_instructions() {
        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let context = create_test_context("Test request");

        let prompt = builder.build_prompt(&context).unwrap();

        assert!(prompt.contains("You are a request classifier"));
        assert!(prompt.contains("RequestType Categories"));
        assert!(prompt.contains("Classification Task"));
    }

    #[test]
    fn test_zero_shot_prompt_contains_request_text() {
        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let context = create_test_context("Write a function to sort an array");

        let prompt = builder.build_prompt(&context).unwrap();

        assert!(prompt.contains("Write a function to sort an array"));
    }

    #[test]
    fn test_zero_shot_prompt_does_not_contain_examples() {
        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let context = create_test_context("Test request");

        let prompt = builder.build_prompt(&context).unwrap();

        assert!(!prompt.contains("Examples"));
    }

    #[test]
    fn test_few_shot_prompt_contains_examples() {
        let examples = vec![FewShotExample {
            request_text: "Create a REST API".into(),
            request_type: RequestType::CodeGeneration,
            explanation: "Requests new code creation".into(),
        }];

        let builder = PromptBuilder::new(PromptMode::FewShot, examples);
        let context = create_test_context("Test request");

        let prompt = builder.build_prompt(&context).unwrap();

        assert!(prompt.contains("Examples"));
        assert!(prompt.contains("Create a REST API"));
        assert!(prompt.contains("code_generation"));
        assert!(prompt.contains("Requests new code creation"));
    }

    #[test]
    fn test_few_shot_limits_to_five_examples() {
        // Create 7 examples
        let examples: Vec<FewShotExample> = (0..7)
            .map(|i| FewShotExample {
                request_text: format!("Example request {}", i),
                request_type: RequestType::CodeGeneration,
                explanation: format!("Explanation {}", i),
            })
            .collect();

        let builder = PromptBuilder::new(PromptMode::FewShot, examples);
        let context = create_test_context("Test request");

        let prompt = builder.build_prompt(&context).unwrap();

        // Should contain Examples 0-4 but not 5-6
        assert!(prompt.contains("Example request 0"));
        assert!(prompt.contains("Example request 4"));
        assert!(!prompt.contains("Example request 5"));
        assert!(!prompt.contains("Example request 6"));

        // Count occurrences of "Example " to verify exactly 5 examples
        let example_count = prompt.matches("Example 1").count()
            + prompt.matches("Example 2").count()
            + prompt.matches("Example 3").count()
            + prompt.matches("Example 4").count()
            + prompt.matches("Example 5").count();
        assert_eq!(example_count, 5);
    }

    #[test]
    fn test_prompt_includes_conversation_history() {
        let history = vec![
            ConversationTurn {
                role: "user".into(),
                content: "How do I sort an array?".into(),
                timestamp: Utc::now(),
            },
            ConversationTurn {
                role: "assistant".into(),
                content: "You can use the sort method".into(),
                timestamp: Utc::now(),
            },
        ];

        let context = InferenceContext {
            request_text: "Show me an example".into(),
            user_context: None,
            conversation_history: Some(history),
            metadata: HashMap::new(),
            received_at: Instant::now(),
        };

        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let prompt = builder.build_prompt(&context).unwrap();

        assert!(prompt.contains("Conversation History"));
        assert!(prompt.contains("How do I sort an array?"));
        assert!(prompt.contains("You can use the sort method"));
    }

    #[test]
    fn test_conversation_history_limits_to_five_turns() {
        // Create 7 conversation turns
        let history: Vec<ConversationTurn> = (0..7)
            .map(|i| ConversationTurn {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("Turn {}", i),
                timestamp: Utc::now(),
            })
            .collect();

        let context = InferenceContext {
            request_text: "Test request".into(),
            user_context: None,
            conversation_history: Some(history),
            metadata: HashMap::new(),
            received_at: Instant::now(),
        };

        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let prompt = builder.build_prompt(&context).unwrap();

        // Should contain the last 5 turns (Turn 2-6) but not the first 2 (Turn 0-1)
        assert!(!prompt.contains("Turn 0"));
        assert!(!prompt.contains("Turn 1"));
        assert!(prompt.contains("Turn 2"));
        assert!(prompt.contains("Turn 6"));
    }

    #[test]
    fn test_prompt_contains_output_format_specification() {
        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let context = create_test_context("Test request");

        let prompt = builder.build_prompt(&context).unwrap();

        assert!(prompt.contains("TYPE:"));
        assert!(prompt.contains("CONFIDENCE:"));
        assert!(prompt.contains("REASONING:"));
    }

    #[test]
    fn test_request_type_documentation_includes_all_variants() {
        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let docs = builder.get_request_type_documentation();

        // Verify all RequestType variants are documented
        assert!(docs.contains("CodeGeneration"));
        assert!(docs.contains("CodeUnderstanding"));
        assert!(docs.contains("TechnicalDesign"));
        assert!(docs.contains("AnalyticalReasoning"));
        assert!(docs.contains("Writing"));
        assert!(docs.contains("FactualLookup"));
        assert!(docs.contains("General"));
    }

    #[test]
    fn test_empty_conversation_history_not_included() {
        let context = InferenceContext {
            request_text: "Test request".into(),
            user_context: None,
            conversation_history: Some(vec![]),
            metadata: HashMap::new(),
            received_at: Instant::now(),
        };

        let builder = PromptBuilder::new(PromptMode::ZeroShot, vec![]);
        let prompt = builder.build_prompt(&context).unwrap();

        assert!(!prompt.contains("Conversation History"));
    }

    #[test]
    fn test_prompt_mode_equality() {
        assert_eq!(PromptMode::ZeroShot, PromptMode::ZeroShot);
        assert_eq!(PromptMode::FewShot, PromptMode::FewShot);
        assert_ne!(PromptMode::ZeroShot, PromptMode::FewShot);
    }

    #[test]
    fn test_few_shot_example_creation() {
        let example = FewShotExample {
            request_text: "Test".into(),
            request_type: RequestType::General,
            explanation: "Test explanation".into(),
        };

        assert_eq!(example.request_text, "Test");
        assert_eq!(example.request_type, RequestType::General);
        assert_eq!(example.explanation, "Test explanation");
    }
}
