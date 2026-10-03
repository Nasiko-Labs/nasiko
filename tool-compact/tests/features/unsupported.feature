Feature: Schemas the grammar cannot carry are refused as a whole batch

  Scenario: A $ref inside a property is not simplified
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"id":{"$ref":"#/$defs/Id"}}}
      """
    When the tools are encoded
    Then encoding fails because of "$ref"

  Scenario: One unsupported tool rejects the whole batch
    Given the sample tool "create_calendar_event"
    And a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"id":{"$ref":"#/$defs/Id"}}}
      """
    When the tools are encoded
    Then encoding fails because of "$ref"

  Scenario: oneOf is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","oneOf":[{"type":"object"}]}
      """
    When the tools are encoded
    Then encoding fails because of "oneOf"

  Scenario: anyOf is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","anyOf":[{"type":"object"}]}
      """
    When the tools are encoded
    Then encoding fails because of "anyOf"

  Scenario: allOf is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","allOf":[{"type":"object"}]}
      """
    When the tools are encoded
    Then encoding fails because of "allOf"

  Scenario: not is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","not":{"type":"string"}}
      """
    When the tools are encoded
    Then encoding fails because of "not"

  Scenario: prefixItems is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","prefixItems":[{"type":"string"}]}
      """
    When the tools are encoded
    Then encoding fails because of "prefixItems"

  Scenario: A schema object for additionalProperties is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","additionalProperties":{"type":"string"}}
      """
    When the tools are encoded
    Then encoding fails because of "additionalProperties"

  Scenario: Tuple items are unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"pair":{"type":"array","items":[{"type":"string"},{"type":"integer"}]}}}
      """
    When the tools are encoded
    Then encoding fails because of "tuple items"

  Scenario: A format other than date-time is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"email":{"type":"string","format":"email"}}}
      """
    When the tools are encoded
    Then encoding fails because of "format"

  Scenario: A non-string enum is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"n":{"enum":[1,2]}}}
      """
    When the tools are encoded
    Then encoding fails because of "enum"

  Scenario: An enum that is not a list is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"n":{"enum":{"a":"b"}}}}
      """
    When the tools are encoded
    Then encoding fails because of "enum"

  Scenario: An array without items is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"tags":{"type":"array"}}}
      """
    When the tools are encoded
    Then encoding fails because of "items"

  Scenario: A property schema with no type is unsupported
    Given a tool named "lookup" described as "Find a row" with this schema:
      """
      {"type":"object","properties":{"loose":{}}}
      """
    When the tools are encoded
    Then encoding fails because of "type"

  Scenario: A root schema that is not an object is unsupported
    Given a tool named "label" described as "A label" with this schema:
      """
      {"type":"string"}
      """
    When the tools are encoded
    Then encoding fails because of "type"

  Scenario: Parameters that are not a schema object are unsupported
    Given a tool named "label" described as "A label" with this schema:
      """
      "nope"
      """
    When the tools are encoded
    Then encoding fails because of "non-object schema"
