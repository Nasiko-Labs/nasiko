Feature: Decode compact calls back into JSON arguments

  Background:
    Given the sample tool "create_calendar_event"
    And the sample tool "send_email"

  Scenario: The design review call decodes to one tool call
    When the model replies:
      """
      <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>
      """
    Then there are 1 calls
    And call 1 is named "create_calendar_event"
    And call 1 argument "title" is "Design review"
    And call 1 argument "start" is "2026-10-05T15:00:00+05:30"
    And call 1 argument "attendees" json is:
      """
      ["riya@example.com"]
      """

  Scenario: Argument key order does not matter
    When the model replies:
      """
      <<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","title":"Design review"}>>
      """
    Then call 1 argument "title" is "Design review"
    And call 1 argument "start" is "2026-10-05T15:00:00+05:30"

  Scenario: Prose around a call is ignored
    When the model replies:
      """
      Sure.
      <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>
      Done.
      """
    Then there are 1 calls
    And call 1 is named "create_calendar_event"

  Scenario: Two calls come back in source order
    When the model replies:
      """
      <<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>> then <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>
      """
    Then there are 2 calls
    And call 1 is named "send_email"
    And call 2 is named "create_calendar_event"
    And call 1 argument "subject" is "Build status"

  Scenario: A plain answer has no calls and is not an error
    When the model replies:
      """
      What's the weather?
      """
    Then there are 0 calls

  Scenario: Greater-than inside a string stays in the title
    When the model replies:
      """
      <<call create_calendar_event {"title":"meet >> review","start":"2026-10-05T15:00:00+05:30"}>> trailing
      """
    Then call 1 argument "title" is "meet >> review"

  Scenario: An escaped quote does not end the JSON string
    When the model replies:
      """
      <<call create_calendar_event {"title":"say \"hi\"","start":"2026-10-05T15:00:00+05:30"}>>
      """
    Then call 1 argument "title" is:
      """
      say "hi"
      """

  Scenario: An unknown tool is rejected
    When the model replies:
      """
      <<call weather {"city":"Pune"}>>
      """
    Then decoding fails with unknown tool "weather"

  Scenario: A missing required title is rejected
    When the model replies:
      """
      <<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>
      """
    Then decoding fails with missing field "title"

  Scenario: A string duration is the wrong type
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","duration_min":"30"}>>
      """
    Then decoding fails with wrong type for "duration_min"

  Scenario: A fractional duration is the wrong type
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","duration_min":1.5}>>
      """
    Then decoding fails with wrong type for "duration_min"

  Scenario: An integer above the signed 64-bit range is still an integer
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","duration_min":9223372036854775808}>>
      """
    Then there are 1 calls
    And call 1 argument "duration_min" json is "9223372036854775808"

  Scenario: A visibility outside the enum is rejected
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>
      """
    Then decoding fails with a bad enum for "visibility"

  Scenario: A non-string enum value is rejected
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":1}>>
      """
    Then decoding fails with wrong type for "visibility"

  Scenario: Attendees sent as one string are rejected
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","attendees":"riya@example.com"}>>
      """
    Then decoding fails with wrong type for "attendees"

  Scenario: A date-time sent as a number is rejected
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro","start":1}>>
      """
    Then decoding fails with wrong type for "start"

  Scenario: A truncated marker is malformed
    When the model replies:
      """
      <<call create_calendar_event {"title":"Retro"}
      """
    Then decoding fails as malformed

  Scenario: A marker that never opens its arguments is malformed
    When the model replies:
      """
      <<call create_calendar_event>>
      """
    Then decoding fails as malformed

  Scenario: Arguments that are not an object are malformed
    When the model replies:
      """
      <<call create_calendar_event ["nope"]>>
      """
    Then decoding fails as malformed

  Scenario: Arguments that are not JSON are malformed
    When the model replies:
      """
      <<call create_calendar_event {nope}>>
      """
    Then decoding fails as malformed

  Scenario: A dangling call marker is malformed
    When the model replies:
      """
      not a call <<call
      """
    Then decoding fails as malformed

  Scenario: schema valid calls only
    When invalid replies are decoded against the calendar tool
    Then every invalid reply is rejected

  Scenario: A string schema cannot accept an object call
    Given a tool named "label" described as "A label" with this schema:
      """
      {"type":"string"}
      """
    When the model replies:
      """
      <<call label {"x":1}>>
      """
    Then decoding fails with wrong type
