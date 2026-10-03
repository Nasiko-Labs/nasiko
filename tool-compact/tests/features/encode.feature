Feature: Encode a tool schema into a compact signature

  Scenario: The calendar signature keeps required fields, optional fields, and the enum
    Given the sample tool "create_calendar_event"
    When the tools are encoded
    Then the signature contains "title:str"
    And the signature contains "start:datetime"
    And the signature contains "duration_min?:int"
    And the signature contains "attendees?:[str]"
    And the signature contains "visibility?:public|private"
    And the signature contains "To call a tool, emit: <<call name {json args}>>"
    And the signature ends with " - Create an event in the user's calendar."
    And the encoded tools accessor lists "create_calendar_event"
    When the compact text is decoded back into schemas
    Then schema decoding returns 1 tools
    And decoded tool 1 is named "create_calendar_event"
    And decoded tool 1 description is "Create an event in the user's calendar."
    And decoded tool 1 requires "title"
    And decoded tool 1 requires "start"
    And decoded tool 1 property "start" format is "date-time"
    And decoded tool 1 property "title" has type "string"
    And decoded tool 1 property "duration_min" has type "integer"
    And decoded tool 1 property "visibility" enum is "public,private"
    And decoded tool 1 property "attendees" items are "string"

  Scenario: Two tools keep both names
    Given the sample tool "create_calendar_event"
    And the sample tool "send_email"
    When the tools are encoded
    And the compact text is decoded back into schemas
    Then schema decoding returns 2 tools
    And decoded tool 1 is named "create_calendar_event"
    And decoded tool 2 is named "send_email"
    And decoded tool 2 requires "to"
    And decoded tool 2 requires "subject"
    And decoded tool 2 requires "body"

  Scenario: A tool with no parameters still states how to call it
    Given a tool named "ping" with no parameters
    When the tools are encoded
    Then the signature contains "ping() - Check liveness"
    When the compact text is decoded back into schemas
    Then decoded tool 1 is named "ping"
    And decoded tool 1 has no required fields

  Scenario: A missing description stays missing after a round trip
    Given a tool named "ping" with no description and no parameters
    When the tools are encoded
    Then the signature contains "ping() - "
    When the compact text is decoded back into schemas
    Then decoded tool 1 has no description

  Scenario: Encoding no tools still states the call format
    Given no tools
    When the tools are encoded
    Then the signature is the call instruction
    When the compact text is decoded back into schemas
    Then schema decoding returns 0 tools

  Scenario: A signature that is not the grammar is refused
    Given the sample tool "create_calendar_event"
    When the tools are encoded
    And the compact text is replaced with:
      """
      create_calendar_event title:str - Create an event.
      """
    And the compact text is decoded back into schemas
    Then schema decoding fails as malformed

  Scenario: A parameter without a type mark is refused
    Given the sample tool "create_calendar_event"
    When the tools are encoded
    And the compact text is replaced with:
      """
      create_calendar_event(title str) - Create an event.
      """
    And the compact text is decoded back into schemas
    Then schema decoding fails as malformed

  Scenario: An object schema with no properties is an empty parameter list
    Given a tool named "ping" described as "Check liveness" with this schema:
      """
      {"type":"object","required":["unused"]}
      """
    When the tools are encoded
    Then the signature contains "ping() - Check liveness"

  Scenario: A signature that never closes is refused
    Given the sample tool "create_calendar_event"
    When the tools are encoded
    And the compact text is replaced with:
      """
      create_calendar_event(title:str - Create an event.
      """
    And the compact text is decoded back into schemas
    Then schema decoding fails as malformed

  Scenario: An unknown type name is refused
    Given the sample tool "create_calendar_event"
    When the tools are encoded
    And the compact text is replaced with:
      """
      create_calendar_event(title:uuid) - Create an event.
      """
    And the compact text is decoded back into schemas
    Then schema decoding fails as malformed
